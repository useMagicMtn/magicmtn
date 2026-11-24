//! Full-duplex terminal transport. Keep input until the relay acknowledges its
//! sequence, and reconnect from the last output actually written to stdout.
use super::*;
use futures_util::{SinkExt, StreamExt};
use std::collections::VecDeque;
use tokio_tungstenite::{
    tungstenite::{self, protocol::Role, Message},
    WebSocketStream,
};

type Socket = WebSocketStream<reqwest::Upgraded>;

#[derive(Debug)]
struct Rejected(reqwest::StatusCode);
impl std::fmt::Display for Rejected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "terminal stream rejected: {}", self.0)
    }
}
impl std::error::Error for Rejected {}

async fn connect(client: &reqwest::Client, url: &reqwest::Url, token: &str) -> Result<Socket> {
    // Use reqwest's existing TLS roots (including MAGICMTN_CA_CERT), disabled
    // proxies and redirect policy for the upgrade as well as HTTP attachment.
    let key = tungstenite::handshake::client::generate_key();
    let response = client
        .get(url.clone())
        .version(reqwest::Version::HTTP_11)
        .bearer_auth(token)
        .header("Connection", "Upgrade")
        .header("Upgrade", "websocket")
        .header("Sec-WebSocket-Version", "13")
        .header("Sec-WebSocket-Key", &key)
        .timeout(Duration::from_secs(5))
        .send()
        .await?;
    if response.status() != reqwest::StatusCode::SWITCHING_PROTOCOLS {
        return Err(Rejected(response.status()).into());
    }
    let accept = tungstenite::handshake::derive_accept_key(key.as_bytes());
    ensure!(
        response
            .headers()
            .get("Sec-WebSocket-Accept")
            .and_then(|v| v.to_str().ok())
            == Some(&accept),
        "invalid terminal websocket handshake"
    );
    let config = tungstenite::protocol::WebSocketConfig {
        max_message_size: Some(65536),
        max_frame_size: Some(65536),
        ..Default::default()
    };
    Ok(
        WebSocketStream::from_raw_socket(response.upgrade().await?, Role::Client, Some(config))
            .await,
    )
}

async fn send<S>(sink: &mut S, frame: &Value) -> Result<()>
where
    S: futures_util::Sink<Message, Error = tungstenite::Error> + Unpin,
{
    send_message(sink, Message::Text(frame.to_string())).await
}

async fn send_message<S>(sink: &mut S, message: Message) -> Result<()>
where
    S: futures_util::Sink<Message, Error = tungstenite::Error> + Unpin,
{
    tokio::time::timeout(Duration::from_secs(5), sink.send(message)).await??;
    Ok(())
}

pub(super) async fn attach(
    client: reqwest::Client,
    mut url: reqwest::Url,
    mut poll_url: Option<reqwest::Url>,
    token: &str,
    client_id: &str,
    mut cursor: u64,
    mut receiver: tokio::sync::mpsc::Receiver<Vec<u8>>,
) -> Result<()> {
    url.query_pairs_mut().append_pair("client_id", client_id);
    let mut seq = 0_u64;
    let mut relay_id: Option<Uuid> = None;
    let mut pending = VecDeque::<Value>::new();
    let mut input_tail = Vec::new();
    let mut stdout = std::io::stdout();
    let mut last_response = Instant::now();
    'reconnect: loop {
        let socket = match connect(&client, &url, token).await {
            Ok(socket) => socket,
            Err(error) => {
                if let Some(Rejected(status)) = error.downcast_ref::<Rejected>() {
                    if status.is_client_error() && *status != reqwest::StatusCode::TOO_MANY_REQUESTS
                    {
                        return Err(error);
                    }
                }
                if last_response.elapsed() > Duration::from_secs(15) {
                    return Err(error).context("terminal connection lost; reconnect with /magic");
                }
                tokio::time::sleep(Duration::from_millis(250)).await;
                continue;
            }
        };
        let (mut sink, mut stream) = socket.split();
        let mut size = crossterm::terminal::size()?;
        // Sequence zero is an idempotent resume, never an input frame.
        let mut hello = json!({"client_id":client_id,"seq":0,"input":"","cursor":cursor,"cols":size.0,"rows":size.1});
        if let Some(epoch) = relay_id {
            hello["relay_id"] = json!(epoch);
        }
        let initial = async {
            send(&mut sink, &hello).await?;
            for frame in &pending {
                send(&mut sink, frame).await?;
            }
            Result::<()>::Ok(())
        }
        .await;
        if initial.is_err() {
            if last_response.elapsed() > Duration::from_secs(15) {
                initial.context("terminal connection lost; reconnect with /magic")?;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
            continue;
        }
        let mut resize = tokio::time::interval(Duration::from_millis(100));
        resize.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut heartbeat = tokio::time::interval(Duration::from_secs(5));
        heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            let mut input = None;
            tokio::select! {
                message = stream.next() => match message {
                    Some(Ok(Message::Text(text))) => {
                        let response: Value = serde_json::from_str(&text).context("invalid terminal stream response")?;
                        if let Some(code) = response["error"].as_u64() {
                            if code == 409 && relay_id.is_none() {
                                if let Some(poll) = poll_url.take() {
                                    // The gateway may still hold the previous client when
                                    // the API issues a new ticket. Polling registers the
                                    // ticket with the relay without sending any keystrokes.
                                    client
                                        .post(poll)
                                        .bearer_auth(token)
                                        .timeout(Duration::from_secs(5))
                                        .json(&hello)
                                        .send()
                                        .await
                                        .and_then(reqwest::Response::error_for_status)
                                        .context("could not register the workspace attachment")?;
                                    continue 'reconnect;
                                }
                            }
                            if code == 429 || code >= 500 { break; }
                            bail!("terminal disconnected: status {code}; reconnect with /magic");
                        }
                        let epoch: Uuid = serde_json::from_value(response["relay_id"].clone()).context("terminal relay identity missing")?;
                        ensure!(relay_id.is_none_or(|previous| previous == epoch), "gateway restarted; reconnect with /magic");
                        relay_id = Some(epoch);
                        let ack = response["seq"].as_u64().context("terminal acknowledgement missing")?;
                        ensure!(ack <= seq, "invalid terminal input acknowledgement");
                        while pending.front().is_some_and(|frame| frame["seq"].as_u64().unwrap() <= ack) { pending.pop_front(); }
                        let next_cursor = response["cursor"].as_u64().context("terminal cursor missing")?;
                        ensure!(next_cursor >= cursor, "terminal cursor moved backwards");
                        if response["lost"] == true { stdout.write_all(b"\r\n[Some earlier terminal output is no longer buffered.]\r\n")?; }
                        let output = B64.decode(response["output"].as_str().context("terminal output missing")?)?;
                        stdout.write_all(&output)?;
                        stdout.flush()?;
                        cursor = next_cursor;
                        last_response = Instant::now();
                        if let Some(code) = response["exit_code"].as_i64() {
                            ensure!(code == 0, "remote pi exited with status {code}");
                            return Ok(());
                        }
                    }
                    Some(Ok(Message::Ping(payload))) => {
                        if send_message(&mut sink, Message::Pong(payload)).await.is_err() { break; }
                    }
                    Some(Ok(Message::Pong(_))) => {},
                    Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                    _ => bail!("invalid terminal websocket message"),
                },
                bytes = receiver.recv(), if relay_id.is_some() && pending.len() < 8 => match bytes {
                    Some(bytes) if !bytes.is_empty() => input = Some(bytes),
                    _ => return Ok(()),
                },
                _ = resize.tick(), if relay_id.is_some() && pending.len() < 8 => {
                    let next_size = crossterm::terminal::size()?;
                    if next_size != size { size = next_size; input = Some(Vec::new()); }
                },
                _ = heartbeat.tick() => {
                    if last_response.elapsed() > Duration::from_secs(10) { break; }
                    if send_message(&mut sink, Message::Ping(Vec::new())).await.is_err() { break; }
                },
            }
            if let Some(input) = input {
                let mut inspected = std::mem::take(&mut input_tail);
                inspected.extend(&input);
                if detach_requested(&inspected) {
                    return Ok(());
                }
                input_tail = inspected[inspected.len().saturating_sub(32)..].to_vec();
                seq += 1;
                let frame = json!({"client_id":client_id,"seq":seq,"input":B64.encode(input),"cursor":cursor,"cols":size.0,"rows":size.1});
                pending.push_back(frame);
                if send(&mut sink, pending.back().unwrap()).await.is_err() {
                    break;
                }
            }
        }
        if last_response.elapsed() > Duration::from_secs(15) {
            bail!("terminal connection lost; reconnect with /magic");
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
        continue 'reconnect;
    }
}
