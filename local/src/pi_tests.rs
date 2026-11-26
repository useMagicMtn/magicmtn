//! End-to-end local orchestration using a real guest supervisor and a fake
//! provider boundary. No network service, account credential or VM is needed.
use super::*;
use axum::{
    body::{to_bytes, Body},
    http::Request,
};
use magicmtn_provider_framework::host::Release;
use tower::ServiceExt;

async fn request(app: &Router, method: &str, path: &str, body: Value) -> (StatusCode, Value) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 2 * 1024 * 1024)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes)
            .unwrap_or_else(|_| json!({"error":String::from_utf8_lossy(&bytes)})),
    )
}

async fn fixture() -> Result<(tempfile::TempDir, Service)> {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir()?;
    let guest = dir.path().join("guest");
    std::fs::create_dir_all(guest.join("home"))?;
    std::fs::create_dir_all(guest.join("bin"))?;
    let pi = guest.join("bin/pi");
    std::fs::write(
        &pi,
        r#"#!/usr/bin/env python3
import json,os,pathlib,sys,urllib.request
root=pathlib.Path(os.environ['PI_CODING_AGENT_DIR'])
model=json.loads((root/'models.json').read_text())['providers']['magicmtn']
print('PI_READY',flush=True)
for line in sys.stdin:
    if line.strip()=='infer':
        body={'model':model['models'][0]['id'],'messages':[{'role':'user','content':'hello'}],'stream':True,'max_tokens':32}
        request=urllib.request.Request(model['baseUrl']+'/chat/completions',json.dumps(body).encode(),{'Authorization':'Bearer '+model['apiKey'],'Content-Type':'application/json'})
        with urllib.request.urlopen(request,timeout=20) as response:
            print('INFERENCE '+response.read().decode(),flush=True)
    else:
        print('INPUT '+line.strip(),flush=True)
"#,
    )?;
    std::fs::set_permissions(&pi, std::fs::Permissions::from_mode(0o700))?;
    let caps = json!({"inference_tasks":["chat"],"inference_models":[{"id":"model-alpha","name":"Alpha","context_window":32768,"max_tokens":4096},{"id":"model-beta","name":"Beta","context_window":65536,"max_tokens":8192}],"inference_deployment_operations":["create","status","delete"],"sandbox_operations":["prepare_image","create","status","exec_bounded","terminate"],"editable_surfaces":[],"observe":false,"evaluate":false,"deploy":false});
    let manifest = json!({"manifest_version":1,"provider_version":"0.1.0","capabilities":caps});
    let script = dir.path().join("provider");
    let source = format!(
        r#"#!/usr/bin/env python3
import base64,json,os,pathlib,signal,sqlite3,subprocess,sys
manifest=json.loads({manifest})
guest={guest}
for line in sys.stdin:
    rpc=json.loads(line); op=rpc['operation']; name=op['name']
    with open({events},'a') as log: log.write(json.dumps(op)+'\n')
    op=op.get('arguments',{{}})
    if name=='describe': result={{'kind':'description','value':manifest}}
    elif name=='inference_deployment_create':
        assert len(op['spec']['name'])<=64
        result={{'kind':'inference_deployment','value':{{'resource':{{'provider_id':'endpoint-test'}},'model':op['spec']['model'],'inference_model':'endpoint:test','state':'ready','ready_replicas':1}}}}
    elif name=='infer':
        assert op['request']['model']['model']=='endpoint:test'
        assert op['request']['model']['parameters']=={{'max_tokens':32}}
        assert set(op['request']['input'])=={{'messages'}}
        result={{'kind':'inference','value':{{'model':'model-beta','output':{{'message':{{'role':'assistant','content':'ANSWER_FROM_SELECTED_ENDPOINT'}},'usage':{{'prompt_tokens':2,'completion_tokens':3,'total_tokens':5}},'finish_reason':'stop'}}}}}}
    elif name=='sandbox_create': result={{'kind':'sandbox','value':{{'provider_id':'test-guest','status':'running','create_time_ms':1,'metadata':{{}}}}}}
    elif name=='sandbox_status': result={{'kind':'sandbox_status','value':'running'}}
    elif name=='sandbox_exec_bounded':
        assert len(op['args'])<=32 and all(len(a.encode())<=2048 for a in op['args'])
        args=[a.replace('/computer',guest) for a in op['args']]
        if len(args)==4 and args[1]=='start':
            config=json.loads(base64.b64decode(args[3])); config['workspace']=guest
            args[3]=base64.b64encode(json.dumps(config).encode()).decode()
        env=dict(os.environ,HOME=guest+'/home',PATH=guest+'/bin:'+os.environ['PATH'],PYTHONPATH={runtime})
        proc=subprocess.run([op['command']]+args,capture_output=True,text=True,env=env,timeout=10)
        if proc.returncode: print(proc.stderr,file=sys.stderr)
        result={{'kind':'exec','value':{{'stdout':proc.stdout,'stderr':proc.stderr,'exit_code':proc.returncode,'elapsed_ms':1,'timed_out':False,'truncated':False}}}}
    elif name=='sandbox_terminate':
        for state in pathlib.Path(guest+'/home/.magicmtn/local-pi').glob('*/state.sqlite3'):
            with sqlite3.connect(state) as db:
                db.execute("INSERT OR REPLACE INTO state VALUES ('desired','\"stopped\"')")
        result={{'kind':'done'}}
    else: result={{'kind':'done'}}
    print(json.dumps({{'id':rpc['id'],'protocol_version':1,'provider':rpc['provider'],'outcome':{{'ok':result}}}}),flush=True)
    if name=='shutdown': break
"#,
        manifest = serde_json::to_string(&manifest.to_string())?,
        guest = serde_json::to_string(&guest.to_string_lossy())?,
        events = serde_json::to_string(&dir.path().join("events").to_string_lossy())?,
        runtime = serde_json::to_string(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../runtime")
                .to_string_lossy()
        )?
    );
    std::fs::write(&script, &source)?;
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700))?;
    let release: Release = serde_json::from_value(
        json!({"package":{"name":"test-local","version":"0.1.0","protocol_version":1,"os":std::env::consts::OS,"arch":std::env::consts::ARCH,"sha256":hex::encode(Sha256::digest(source.as_bytes()))},"manifest":manifest,"active":true,"executable":script}),
    )?;
    let registry = Registry::from_releases(vec![release])?;
    let db = Database::open(&dir.path().join("local.sqlite3")).await?;
    db.bootstrap(&registry).await?;
    magicmtn_engine::sandbox_images::seed(&db).await?;
    let binding = db.binding(None, None, "test-local").await?;
    let images = magicmtn_engine::sandbox_images::manifests(&db, "canonical").await?;
    magicmtn_engine::sandbox_images::save(
        &db,
        "canonical",
        &binding,
        &magicmtn_sdk::image_registry::PreparedImage {
            image: images["linux/arm64"].clone(),
            platform: "linux/arm64".into(),
            artifact: "prepared-test".into(),
            configuration: Default::default(),
        },
    )
    .await?;
    Ok((dir, Service::new(db, registry, 3600)))
}

async fn wait_tick(service: &Service, id: Uuid, expected: &str) -> Result<()> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        service.tick(id).await?;
        let chunks: Vec<Vec<u8>> = sqlx::query_scalar(
            "SELECT data FROM local_pi_output WHERE session_id=? ORDER BY start",
        )
        .bind(id.to_string())
        .fetch_all(&service.db.pool)
        .await?;
        let output = String::from_utf8_lossy(&chunks.concat()).into_owned();
        if output.contains(expected) {
            return Ok(());
        }
        ensure!(
            tokio::time::Instant::now() < deadline,
            "missing terminal output: {expected}; got {output}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test]
async fn selected_model_endpoint_and_pi_survive_detach_and_host_restart() -> Result<()> {
    let (dir, service) = fixture().await?;
    let app = service.clone().router();
    let (_, catalog) = request(&app, "GET", "/v1/pi/catalog", json!(null)).await;
    assert!(catalog["defaults"]["model_id"].is_null());
    let selected = &catalog["models"][1]["id"];
    let (status, result) = request(
        &app,
        "POST",
        "/v1/pi/defaults",
        json!({"model_id":selected}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{result}");
    assert_eq!(result["deployment"]["model"], "model-beta");
    let id = Uuid::new_v4();
    let create = json!({"provider":"test-local","idempotency_key":id});
    assert_eq!(
        request(&app, "POST", "/v1/pi/workspaces", create.clone())
            .await
            .0,
        StatusCode::OK
    );
    let saved = service.db.get(id).await?.unwrap();
    assert_eq!(saved["metadata"]["_magicmtn_image_selection"], "canonical");
    wait_tick(&service, id, "PI_READY").await?;
    // Payloads cross both per-argument and per-command transport boundaries.
    let payload: Vec<u8> = (0..70_000).map(|index| (index % 251) as u8).collect();
    service.upload(id, "~/large-payload", &payload).await?;
    assert_eq!(
        std::fs::read(dir.path().join("guest/home/large-payload"))?,
        payload
    );
    let guest = dir
        .path()
        .join(format!("guest/home/.magicmtn/local-pi/{id}"));
    let pid: Value = serde_json::from_str(&std::fs::read_to_string(guest.join("config.json"))?)?;
    assert!(
        pid["token"].is_null(),
        "host configuration must not contain an account credential"
    );
    let (_, attached) = request(
        &app,
        "POST",
        &format!("/v1/pi/workspaces/{id}/terminal"),
        json!({"cols":100,"rows":30}),
    )
    .await;
    let input = json!({"client_id":attached["client_id"],"seq":1,"input":B64.encode("infer\n"),"cursor":0,"cols":100,"rows":30});
    let poll_path = format!("/v1/pi/workspaces/{id}/terminal/poll");
    assert_eq!(
        request(&app, "POST", &poll_path, input.clone()).await.0,
        StatusCode::OK
    );
    service.tick(id).await?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        service.tick(id).await?;
        let snapshot: Value =
            sqlx::query_scalar("SELECT snapshot FROM local_pi_sessions WHERE id=?")
                .bind(id.to_string())
                .fetch_one(&service.db.pool)
                .await?;
        if snapshot["request"].is_object() {
            break;
        }
        ensure!(
            tokio::time::Instant::now() < deadline,
            "guest did not request inference"
        );
    }
    service.relay(id).await?;
    wait_tick(&service, id, "ANSWER_FROM_SELECTED_ENDPOINT").await?;
    // Simulate losing a poll response after delivery. It cannot type the turn twice.
    request(&app, "POST", &poll_path, input).await;
    let queued: i64 = sqlx::query_scalar("SELECT count(*) FROM local_pi_inputs WHERE session_id=?")
        .bind(id.to_string())
        .fetch_one(&service.db.pool)
        .await?;
    assert_eq!(queued, 0);
    // Restart the host assembly, retaining its SQLite state and the live guest.
    let reopened = Database::open(&dir.path().join("local.sqlite3")).await?;
    reopened.bootstrap(&service.registry).await?;
    let recovered = Service::new(reopened, service.registry.clone(), 3600);
    let app = recovered.clone().router();
    assert_eq!(
        request(
            &app,
            "POST",
            "/v1/pi/defaults",
            json!({"model_id":selected})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        request(&app, "POST", "/v1/pi/workspaces", create).await.0,
        StatusCode::OK
    );
    recovered.tick(id).await?;
    let events = std::fs::read_to_string(dir.path().join("events"))?;
    let events: Vec<Value> = events
        .lines()
        .map(serde_json::from_str)
        .collect::<std::result::Result<_, _>>()?;
    assert_eq!(
        events
            .iter()
            .filter(|v| v["name"] == "inference_deployment_create")
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|v| v["name"] == "sandbox_create")
            .count(),
        1
    );
    assert_eq!(events.iter().filter(|v| v["name"] == "infer").count(), 1);
    // At-most-once inference receipts fail an interrupted turn instead of replaying it.
    let interrupted = Uuid::new_v4().to_string();
    sqlx::query("INSERT INTO local_pi_inference(session_id,request_id) VALUES(?,?)")
        .bind(id.to_string())
        .bind(&interrupted)
        .execute(&recovered.db.pool)
        .await?;
    let uncertain = recovered.infer(id, &interrupted, 0).await?;
    assert_eq!(uncertain["error"]["type"], "interrupted");
    let (_, stopping) = request(
        &app,
        "POST",
        &format!("/v1/pi/workspaces/{id}/stop"),
        json!({}),
    )
    .await;
    assert_eq!(stopping["desired"], "stopped");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    loop {
        recovered.tick(id).await?;
        if recovered.session(id).await?["status"] == "stopped" {
            break;
        }
        ensure!(tokio::time::Instant::now() < deadline, "guest did not stop");
    }
    assert_eq!(
        request(
            &app,
            "POST",
            &format!("/v1/pi/workspaces/{id}/restart"),
            json!({})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert!(recovered.session(id).await?["exit_code"].is_null());
    recovered.tick(id).await?;
    assert!(recovered.session(id).await?["exit_code"].is_null());
    let files: Vec<_> =
        std::fs::read_dir(guest.join("pi/sessions"))?.collect::<std::io::Result<_>>()?;
    assert_eq!(
        files.len(),
        1,
        "restart must retain the existing Pi conversation"
    );
    recovered.exchange(id, json!({"desired":"stopped"})).await?;
    recovered.registry.shutdown().await;
    Ok(())
}
