"""Credential-free loopback inference and terminal fixture for real Pi tests."""
import base64
import http.server
import json
import threading
import time


class Bridge(http.server.ThreadingHTTPServer):
    daemon_threads = True

    def __init__(self):
        self.output = bytearray()
        self.offset = 0
        self.input = []
        self.ack = 0
        self.exit_code = None
        self.requests = []
        self.prompt_sent = False
        self.ready_at = None
        self.tool_seen = threading.Event()
        self.finished = threading.Event()
        self.real_pi = False
        self.exit_after_answer = False
        super().__init__(("127.0.0.1", 0), Handler)
        self.thread = threading.Thread(target=self.serve_forever, daemon=True)
        self.thread.start()

    def frame(self, data):
        self.input.append({"seq": len(self.input) + 1, "data": base64.b64encode(data).decode()})


class Handler(http.server.BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def do_POST(self):
        if self.headers.get("Authorization") != "Bearer test-scoped-token":
            self.send_error(401)
            return
        body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        bridge = self.server
        if self.path.endswith("/poll"):
            data = base64.b64decode(body["output"])
            if body["offset"] == bridge.offset:
                bridge.output.extend(data)
                bridge.offset += len(data)
            bridge.ack = body["input_ack"]
            bridge.exit_code = body.get("exit_code")
            if bridge.exit_after_answer and not bridge.input and b"Finished writing evidence." in bridge.output:
                bridge.frame(b"/quit\r")
            if bridge.real_pi and bridge.ready_at is None and b"startup help and loaded resources" in bridge.output:
                bridge.ready_at = time.monotonic()
            if bridge.real_pi and not bridge.prompt_sent and bridge.ready_at is not None and time.monotonic() - bridge.ready_at > 0.5:
                bridge.prompt_sent = True
                bridge.frame("Write the requested evidence file. 雪\r".encode())
            if bridge.finished.is_set() and len(bridge.input) == 1:
                # Allow the agent loop to settle before asking pi to exit.
                time.sleep(0.3)
                bridge.frame(b"/quit\r")
            response = {"offset": bridge.offset, "input": [f for f in bridge.input if f["seq"] > bridge.ack], "cols": 100, "rows": 32}
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.end_headers()
            self.wfile.write(json.dumps(response).encode())
            return
        if self.path.endswith("/chat/completions"):
            bridge.requests.append(body)
            if len(bridge.requests) == 1:
                delta = {"role": "assistant", "content": "Writing the file.", "tool_calls": [{"index": 0, "id": "call_evidence", "type": "function", "function": {"name": "bash", "arguments": json.dumps({"command": "printf 'VM evidence' > evidence.txt; pwd"})}}]}
                finish = "tool_calls"
            else:
                bridge.tool_seen.set()
                delta = {"role": "assistant", "content": "Finished writing evidence. 雪"}
                finish = "stop"
                bridge.finished.set()
            first = {"id": "test", "object": "chat.completion.chunk", "model": body["model"], "choices": [{"index": 0, "delta": delta, "finish_reason": None}]}
            last = dict(first, choices=[{"index": 0, "delta": {}, "finish_reason": finish}], usage={"prompt_tokens": 12, "completion_tokens": 4, "total_tokens": 16})
            self.send_response(200)
            self.send_header("Content-Type", "text/event-stream")
            self.end_headers()
            thinking = dict(first, choices=[{"index": 0, "delta": {"reasoning_content": "PRIVATE_REASONING_MARKER " * 80}, "finish_reason": None}])
            self.wfile.write(f"data: {json.dumps(thinking)}\n\n".encode())
            self.wfile.flush()
            time.sleep(0.15)
            self.wfile.write((f"data: {json.dumps(first)}\n\ndata: {json.dumps(last)}\n\ndata: [DONE]\n\n").encode())
            return
        self.send_error(404)

