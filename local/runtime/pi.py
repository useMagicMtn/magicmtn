"""Local managed Pi: durable PTY and inference mailbox over bounded provider exec.

Only the sandbox's loopback proxy key enters Pi. Provider credentials stay in
its host. Detaching a terminal or restarting that host leaves this supervisor
and Pi running in the sandbox.
"""
import base64
import fcntl
import hashlib
import http.server
import json
import os
import pathlib
import pty
import secrets
import select
import signal
import sqlite3
import struct
import subprocess
import sys
import termios
import threading
import time
import uuid

LIMIT = 16 * 1024 * 1024
ROOT = pathlib.Path.home() / ".magicmtn" / "local-pi"


def database(directory):
    # WAL's first initialization needs its own lock: the supervisor and a host
    # polling a fresh sandbox can arrive together before SQLite sets WAL mode.
    with (directory / "schema.lock").open("a") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        db = sqlite3.connect(directory / "state.sqlite3", timeout=10)
        db.execute("PRAGMA journal_mode=WAL")
        db.execute("PRAGMA synchronous=FULL")
        db.executescript("""
        CREATE TABLE IF NOT EXISTS state (key TEXT PRIMARY KEY, value TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS output (start INTEGER PRIMARY KEY, data BLOB NOT NULL);
        CREATE TABLE IF NOT EXISTS input (id TEXT PRIMARY KEY, body TEXT NOT NULL, consumed INTEGER NOT NULL DEFAULT 0);
        CREATE TABLE IF NOT EXISTS requests (id TEXT PRIMARY KEY, request TEXT NOT NULL, response TEXT, acknowledged INTEGER NOT NULL DEFAULT 0);
        """)
    return db


def get(db, key, default=None):
    row = db.execute("SELECT value FROM state WHERE key=?", (key,)).fetchone()
    return json.loads(row[0]) if row else default


def put(db, key, value):
    db.execute("INSERT OR REPLACE INTO state VALUES (?,?)", (key, json.dumps(value)))


def atomic_json(path, value):
    tmp = path.with_suffix(".tmp")
    with tmp.open("w") as file:
        json.dump(value, file)
        file.flush()
        os.fsync(file.fileno())
    tmp.replace(path)


class Proxy(http.server.BaseHTTPRequestHandler):
    def log_message(self, *_args):
        pass

    def do_POST(self):
        if self.headers.get("Authorization") != "Bearer " + self.server.key:
            self.send_error(401)
            return
        length = int(self.headers.get("Content-Length", "0"))
        if not 0 < length <= LIMIT:
            self.send_error(413)
            return
        body = json.loads(self.rfile.read(length))
        if self.path.endswith("/errors"):
            self.send_response(204)
            self.end_headers()
            return
        if self.path != "/v1/harness/agent/chat/completions":
            self.send_error(404)
            return
        db = database(self.server.directory)
        request_id = str(uuid.uuid4())
        with db:
            db.execute("INSERT INTO requests(id,request) VALUES (?,?)", (request_id, json.dumps(body)))
        deadline = time.monotonic() + 600
        try:
            while time.monotonic() < deadline:
                row = db.execute("SELECT response FROM requests WHERE id=?", (request_id,)).fetchone()
                if row[0] is not None:
                    response = json.loads(row[0])
                    break
                time.sleep(0.1)
            else:
                response = {"error": {"message": "Local inference timed out", "type": "timeout"}}
            if "error" in response:
                encoded = json.dumps(response).encode()
                self.send_response(502)
                self.send_header("Content-Type", "application/json")
                self.end_headers()
                self.wfile.write(encoded)
            elif body.get("stream"):
                self.send_response(200)
                self.send_header("Content-Type", "text/event-stream")
                self.end_headers()
                choice = response["choices"][0]
                message = dict(choice["message"])
                for index, call in enumerate(message.get("tool_calls") or []):
                    call["index"] = index
                chunk = dict(response, object="chat.completion.chunk", choices=[{
                    "index": 0, "delta": message, "finish_reason": None}])
                self.wfile.write(("data: " + json.dumps(chunk) + "\n\n").encode())
                chunk["choices"] = [{"index": 0, "delta": {}, "finish_reason": choice["finish_reason"]}]
                self.wfile.write(("data: " + json.dumps(chunk) + "\n\ndata: [DONE]\n\n").encode())
            else:
                self.send_response(200)
                self.send_header("Content-Type", "application/json")
                self.end_headers()
                self.wfile.write(json.dumps(response).encode())
        finally:
            with db:
                # Keep the ID as a receipt but drop conversation content after delivery.
                db.execute("UPDATE requests SET acknowledged=1, request='{}',response=NULL WHERE id=?", (request_id,))
            db.close()


def serve(directory, config):
    lock = (directory / "supervisor.lock").open("a")
    try:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except BlockingIOError:
        lock.close()
        return
    db = database(directory)
    # A daemon loss may have left a child running: never start a second agent.
    old_pid = get(db, "pid")
    if old_pid:
        try:
            os.kill(old_pid, 0)
        except ProcessLookupError:
            pass
        else:
            with db:
                put(db, "status", "recovery_required")
            db.close()
            lock.close()
            return
    sys.path.insert(0, "/opt/magicmtn/runtime")
    from harnesses.pi import PiHarness
    import environment
    proxy = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Proxy)
    proxy.daemon_threads = True
    proxy.directory = directory
    proxy.key = secrets.token_urlsafe(32)
    threading.Thread(target=proxy.serve_forever, daemon=True).start()
    config.update(origin=f"http://127.0.0.1:{proxy.server_port}", token=proxy.key)
    config.setdefault("workspace", "/computer")
    harness = PiHarness(directory, config, config["session_id"], atomic_json)
    master, slave = pty.openpty()
    specification = environment.configure(directory, config, harness.environment(), atomic_json)
    values = dict(environment.apply(specification), TERM="xterm-256color")
    # This process is started without any host provider environment.
    agent = subprocess.Popen(harness.command(), cwd=config["workspace"], env=values,
                             stdin=slave, stdout=slave, stderr=slave, start_new_session=True,
                             preexec_fn=lambda: fcntl.ioctl(slave, termios.TIOCSCTTY, 0))
    os.close(slave)
    with db:
        put(db, "pid", agent.pid)
        put(db, "status", "running")
        put(db, "exit_code", None)
    try:
        while agent.poll() is None:
            if get(db, "desired", "running") == "stopped":
                os.killpg(agent.pid, signal.SIGTERM)
                try:
                    agent.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    os.killpg(agent.pid, signal.SIGKILL)
                break
            inputs = db.execute("SELECT id,body FROM input WHERE consumed=0 ORDER BY rowid LIMIT 16").fetchall()
            for identity, encoded in inputs:
                body = json.loads(encoded)
                with db:
                    db.execute("UPDATE input SET consumed=1,body='{}' WHERE id=?", (identity,))
                if "cols" in body:
                    fcntl.ioctl(master, termios.TIOCSWINSZ, struct.pack("HHHH", body["rows"], body["cols"], 0, 0))
                if body.get("input"):
                    data = base64.b64decode(body["input"], validate=True)
                    while data:
                        data = data[os.write(master, data):]
            if select.select([master], [], [], 0.05)[0]:
                try:
                    data = os.read(master, 8192)
                except OSError:
                    break
                if not data:
                    break
                with db:
                    cursor = get(db, "cursor", 0)
                    db.execute("INSERT INTO output VALUES (?,?)", (cursor, data))
                    put(db, "cursor", cursor + len(data))
                    db.execute("DELETE FROM output WHERE start < ?", (cursor - 1024 * 1024,))
        code = agent.wait()
        with db:
            put(db, "pid", None)
            put(db, "exit_code", code)
            put(db, "status", "stopped" if get(db, "desired") == "stopped" else "exited")
    finally:
        os.close(master)
        proxy.shutdown()
        proxy.server_close()
        db.close()
        lock.close()


def exchange(directory, body):
    db = database(directory)
    with db:
        if "desired" in body:
            put(db, "desired", body["desired"])
            if body["desired"] == "stopped":
                db.execute("UPDATE input SET consumed=1,body='{}' WHERE consumed=0")
        if "input_id" in body:
            db.execute("INSERT OR IGNORE INTO input(id,body) VALUES (?,?)", (body["input_id"], json.dumps(body)))
        if "response_id" in body:
            value = (directory / (body["response_id"] + ".response")).read_text()
            json.loads(value)
            db.execute("UPDATE requests SET response=? WHERE id=? AND acknowledged=0", (value, body["response_id"]))
        cursor = body.get("cursor", 0)
        first = db.execute("SELECT min(start) FROM output").fetchone()[0]
        lost = first is not None and cursor < first
        cursor = max(cursor, first or 0)
        output = bytearray()
        for start, data in db.execute("SELECT start,data FROM output WHERE start+length(data)>? ORDER BY start LIMIT 4", (cursor,)):
            output.extend(data[max(cursor - start, 0):])
            if len(output) >= 4096:
                break
        output = output[:4096]
        request = db.execute("SELECT id,length(CAST(request AS BLOB)) FROM requests WHERE acknowledged=0 AND response IS NULL ORDER BY rowid LIMIT 1").fetchone()
        result = {"status": get(db, "status", "starting"), "output": base64.b64encode(output).decode(),
                  "cursor": cursor + len(output), "lost": lost, "exit_code": get(db, "exit_code"),
                  "request": {"id": request[0], "bytes": request[1]} if request else None}
    db.close()
    return result


def main():
    os.umask(0o077)
    command, session_id = sys.argv[1:3]
    uuid.UUID(session_id)
    directory = ROOT / session_id
    directory.mkdir(parents=True, exist_ok=True, mode=0o700)
    if command == "serve":
        serve(directory, json.loads((directory / "config.json").read_text()))
    elif command == "start":
        atomic_json(directory / "config.json", json.loads(base64.b64decode(sys.argv[3])))
        with database(directory) as db:
            put(db, "desired", "running")
            if get(db, "status") == "running":
                pid = get(db, "pid")
                if pid:
                    try:
                        os.kill(pid, 0)
                    except ProcessLookupError:
                        pass
                    else:
                        print('{}')
                        return
        # A stopped supervisor can still be shutting down its HTTP thread.
        lock = (directory / "supervisor.lock").open("a")
        deadline = time.monotonic() + 3
        while True:
            try:
                fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
                break
            except BlockingIOError:
                if time.monotonic() > deadline:
                    raise RuntimeError("supervisor is still stopping")
                time.sleep(0.05)
        lock.close()
        with database(directory) as db:
            put(db, "status", "starting")
            put(db, "exit_code", None)
            db.execute("UPDATE input SET consumed=1,body='{}' WHERE consumed=0")
        subprocess.Popen([sys.executable, __file__, "serve", session_id], start_new_session=True,
                         stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=(directory / "supervisor.log").open("a"))
        print('{}')
    elif command == "exchange":
        print(json.dumps(exchange(directory, json.loads(base64.b64decode(sys.argv[3])))))
    elif command == "read":
        request_id = str(uuid.UUID(sys.argv[3]))
        offset = int(sys.argv[4])
        with database(directory) as db:
            value = db.execute("SELECT request FROM requests WHERE id=? AND acknowledged=0", (request_id,)).fetchone()[0].encode()
        print(json.dumps({"data": base64.b64encode(value[offset:offset + 4096]).decode()}))


if __name__ == "__main__":
    main()
