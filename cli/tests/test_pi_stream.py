"""Exercise the real CLI over HTTP attachment + WebSocket with a local PTY."""
import base64
import fcntl
import hashlib
import http.server
import json
import os
import pathlib
import pty
import select
import shutil
import socket
import struct
import subprocess
import tempfile
import termios
import threading
import time
import unittest
import uuid

try:
    from websockets.sync.server import serve
    from websockets.exceptions import ConnectionClosed
except ImportError:
    serve = None


@unittest.skipUnless(os.environ.get("MAGICMTN_TEST_MAGIC") and serve,
                     "set MAGICMTN_TEST_MAGIC and install runtime/requirements.txt")
class StreamTerminalTest(unittest.TestCase):
    def test_stream_input_resize_ack_loss_reconnect_and_terminal_restore(self):
        self.run_terminal(previous_client=False)

    def test_resume_claims_the_relay_from_the_previous_client(self):
        self.run_terminal(previous_client=True)

    @unittest.skipUnless(shutil.which("caddy") and shutil.which("openssl"),
                         "install Caddy and OpenSSL to test HTTPS protocol negotiation")
    def test_https_stream_negotiates_http1_with_http2_ingress(self):
        self.run_terminal(previous_client=True, https=True)

    def run_terminal(self, previous_client, https=False):
        binary = str(pathlib.Path(os.environ["MAGICMTN_TEST_MAGIC"]).resolve())
        with tempfile.TemporaryDirectory() as home:
            state = {"seq": 0, "output": b"remote stream ready\r\n", "input": bytearray(),
                     "frames": [], "connections": 0, "dropped": False}
            relay_id = str(uuid.uuid4())
            client_id = str(uuid.uuid4())
            state["relay_client"] = str(uuid.uuid4()) if previous_client else client_id
            claims = []

            def handle(connection):
                self.assertEqual(connection.request.headers["Authorization"], "Bearer test-cli-session")
                state["connections"] += 1
                cursor = None
                try:
                    for text in connection:
                        frame = json.loads(text)
                        state["frames"].append(frame)
                        if frame["client_id"] != state["relay_client"]:
                            connection.send(json.dumps({"error": 409}))
                            return
                        if cursor is None:
                            cursor = frame["cursor"]
                        data = base64.b64decode(frame["input"])
                        if frame["seq"] == state["seq"] + 1:
                            state["seq"] = frame["seq"]
                            state["input"].extend(data)
                            state["output"] += data
                        if data and not state["dropped"]:
                            # Accept input but lose the acknowledgement. The CLI
                            # must resume output and replay input without doubling it.
                            state["dropped"] = True
                            connection.close(code=1012)
                            return
                        output = state["output"][cursor:]
                        cursor += len(output)
                        connection.send(json.dumps({"seq": state["seq"], "relay_id": relay_id,
                            "cursor": cursor, "output": base64.b64encode(output).decode(), "lost": False, "exit_code": None}))
                except ConnectionClosed:
                    pass

            ws = serve(handle, "127.0.0.1", 0)
            ws_thread = threading.Thread(target=ws.serve_forever)
            ws_thread.start()
            stream_url = f"http://127.0.0.1:{ws.socket.getsockname()[1]}/stream"
            tls_env = {}
            if https:
                certificate = pathlib.Path(home) / "ca.pem"
                ca_key = pathlib.Path(home) / "ca-key.pem"
                key = pathlib.Path(home) / "key.pem"
                csr = pathlib.Path(home) / "localhost.csr"
                leaf = pathlib.Path(home) / "localhost.pem"
                extensions = pathlib.Path(home) / "localhost.ext"
                extensions.write_text("subjectAltName=DNS:localhost\nbasicConstraints=critical,CA:FALSE\n"
                                      "keyUsage=critical,digitalSignature,keyEncipherment\n"
                                      "extendedKeyUsage=serverAuth\n")
                for command in (
                    ["req", "-x509", "-newkey", "rsa:2048", "-nodes", "-keyout", str(ca_key),
                     "-out", str(certificate), "-days", "1", "-subj", "/CN=Terminal test CA",
                     "-addext", "basicConstraints=critical,CA:TRUE"],
                    ["req", "-new", "-newkey", "rsa:2048", "-nodes", "-keyout", str(key),
                     "-out", str(csr), "-subj", "/CN=localhost"],
                    ["x509", "-req", "-in", str(csr), "-CA", str(certificate), "-CAkey", str(ca_key),
                     "-CAcreateserial", "-out", str(leaf), "-days", "1", "-extfile", str(extensions)],
                ):
                    subprocess.run(["openssl", *command], check=True,
                                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
                with socket.socket() as port_socket:
                    port_socket.bind(("127.0.0.1", 0))
                    tls_port = port_socket.getsockname()[1]
                config = pathlib.Path(home) / "Caddyfile"
                config.write_text(f"{{\n admin off\n auto_https off\n}}\n"
                                  f"https://localhost:{tls_port} {{\n"
                                  f" tls {leaf} {key}\n"
                                  f" reverse_proxy 127.0.0.1:{ws.socket.getsockname()[1]}\n}}\n")
                proxy_log = open(pathlib.Path(home) / "caddy.log", "w+")
                self.addCleanup(proxy_log.close)
                proxy = subprocess.Popen(["caddy", "run", "--config", str(config), "--adapter", "caddyfile"],
                                         stdout=proxy_log, stderr=subprocess.STDOUT,
                                         env=dict(os.environ, XDG_DATA_HOME=home, XDG_CONFIG_HOME=home))
                def stop_proxy():
                    proxy.terminate()
                    proxy.wait(timeout=5)
                self.addCleanup(stop_proxy)
                for _ in range(100):
                    if proxy.poll() is not None:
                        proxy_log.seek(0)
                        self.fail(proxy_log.read())
                    try:
                        with socket.create_connection(("127.0.0.1", tls_port), timeout=0.1):
                            break
                    except OSError:
                        time.sleep(0.05)
                else:
                    self.fail("HTTPS proxy did not start")
                stream_url = f"https://localhost:{tls_port}/stream"
                tls_env["MAGICMTN_CA_CERT"] = str(certificate)

            class Handler(http.server.BaseHTTPRequestHandler):
                def log_message(self, *args):
                    pass

                def do_POST(request):
                    self.assertEqual(request.headers["Authorization"], "Bearer test-cli-session")
                    frame = json.loads(request.rfile.read(int(request.headers["Content-Length"])))
                    if request.path == "/poll":
                        claims.append(frame)
                        state["relay_client"] = frame["client_id"]
                        body = json.dumps({"cursor": len(state["output"]),
                                           "output": base64.b64encode(state["output"]).decode()}).encode()
                    else:
                        self.assertTrue(request.path.endswith("/terminal"))
                        body = json.dumps({"client_id": client_id, "cursor": 0, "token": "test-cli-session",
                                           "stream_url": stream_url, "poll_url": origin + "poll"}).encode()
                    request.send_response(200)
                    request.send_header("Content-Length", str(len(body)))
                    request.end_headers()
                    request.wfile.write(body)

            http_server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
            http_server.daemon_threads = True
            threading.Thread(target=http_server.serve_forever, daemon=True).start()
            origin = f"http://127.0.0.1:{http_server.server_port}/"
            credentials = pathlib.Path(home) / ".config/magicmtn/cli"
            credentials.mkdir(parents=True)
            (credentials / (hashlib.sha256(origin.encode()).hexdigest() + ".json")).write_text(
                json.dumps({"server": origin, "token": "test-cli-session"}))
            master, slave = pty.openpty()
            before = termios.tcgetattr(slave)
            fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 24, 80, 0, 0))
            process = subprocess.Popen([binary, "--server-url", origin, "pi", "attach", str(uuid.uuid4())],
                                       stdin=slave, stdout=slave, stderr=slave, env=dict(os.environ, HOME=home, **tls_env))
            output = bytearray()
            typed = False
            detached = False
            try:
                deadline = time.monotonic() + 20
                while process.poll() is None and time.monotonic() < deadline:
                    if select.select([master], [], [], 0.025)[0]:
                        output.extend(os.read(master, 8192))
                    if b"remote stream ready" in output and not typed:
                        os.write(master, "typed 雪\r".encode())
                        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 32, 100, 0, 0))
                        typed = True
                    if ("typed 雪\r".encode() in output and state["connections"] >= 2
                            and any(f["cols"] == 100 for f in state["frames"]) and not detached):
                        os.write(master, b"\x1b[93;5u")
                        detached = True
                self.assertIsNotNone(process.poll(), output.decode(errors="replace"))
                self.assertEqual(process.returncode, 0, output.decode(errors="replace"))
                self.assertTrue(typed and detached, output.decode(errors="replace"))
                self.assertEqual(state["input"], "typed 雪\r".encode())
                self.assertEqual(claims, [{"client_id": client_id, "seq": 0, "input": "",
                                           "cursor": 0, "cols": 80, "rows": 24}] if previous_client else [])
                self.assertEqual(output.count(b"remote stream ready"), 1)
                self.assertEqual(output.count("typed 雪\r".encode()), 1)
                self.assertEqual(termios.tcgetattr(slave), before)
            finally:
                if process.poll() is None:
                    process.kill()
                    process.wait()
                os.close(master)
                os.close(slave)
                http_server.shutdown()
                http_server.server_close()
                ws.shutdown()
                ws_thread.join(timeout=3)
