import json, sys, time
from http.server import BaseHTTPRequestHandler, HTTPServer

MODE = sys.argv[1] if len(sys.argv) > 1 else "ok"
PORT = int(sys.argv[2]) if len(sys.argv) > 2 else 8765
state = {"n": 0}

def sse(h, chunks, done=True):
    h.send_response(200)
    h.send_header("content-type", "text/event-stream")
    h.end_headers()
    for c in chunks:
        h.wfile.write(b"data: " + c + b"\n\n")
        h.wfile.flush()
    if done:
        h.wfile.write(b"data: [DONE]\n\n")
        h.wfile.flush()

def tool_call(name, args):
    d = {"choices": [{"index": 0, "delta": {"role": "assistant", "tool_calls": [{"index": 0, "id": "call_1", "type": "function", "function": {"name": name, "arguments": json.dumps(args)}}]}, "finish_reason": None}]}
    e = {"choices": [{"index": 0, "delta": {}, "finish_reason": "tool_calls"}], "usage": {"prompt_tokens": 100, "completion_tokens": 10, "total_tokens": 110}}
    return [json.dumps(d).encode(), json.dumps(e).encode()]

def text(t):
    d = {"choices": [{"index": 0, "delta": {"role": "assistant", "content": t}}]}
    e = {"choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}], "usage": {"prompt_tokens": 150, "completion_tokens": 8, "total_tokens": 158}}
    return [json.dumps(d).encode(), json.dumps(e).encode()]

class H(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass
    def do_GET(self):
        b = json.dumps({"object": "list", "data": [{"id": "m1", "object": "model"}]}).encode()
        self.send_response(200)
        self.send_header("content-type", "application/json")
        self.end_headers()
        self.wfile.write(b)
    def do_POST(self):
        n = int(self.headers.get("content-length", 0))
        self.rfile.read(n)
        state["n"] += 1
        if MODE == "ok":
            sse(self, tool_call("run", {"code": "print(7*6)"}) if state["n"] == 1 else text("The result is 42."))
        elif MODE == "suicide":
            sse(self, tool_call("run", {"code": "import os; os.kill(os.getpid(), 9)"}) if state["n"] == 1 else text("The worker died but the turn survived."))
        elif MODE == "garbage":
            self.send_response(200)
            self.send_header("content-type", "text/event-stream")
            self.end_headers()
            self.wfile.write(b"data: {this is \xff\xfe not json\n\n")
            self.wfile.write(b"data: " + json.dumps({"choices": [{"index": 0, "delta": {"content": "recovered after garbage"}}]}).encode() + b"\n\n")
            self.wfile.write(b"data: [DONE]\n\n")
        elif MODE == "close":
            self.send_response(200)
            self.send_header("content-type", "text/event-stream")
            self.end_headers()
            self.wfile.write(b"data: " + json.dumps({"choices": [{"index": 0, "delta": {"content": "you will never see th"}}]}).encode() + b"\n")
            self.wfile.flush()
            self.connection.close()
        elif MODE == "error500":
            self.send_response(500)
            self.send_header("content-type", "text/plain")
            self.end_headers()
            self.wfile.write(b"upstream exploded: totally not json")
        elif MODE == "bigline":
            self.send_response(200)
            self.send_header("content-type", "text/event-stream")
            self.end_headers()
            self.wfile.write(b"data: " + b"x" * (17 << 20) + b"\n\n")
        elif MODE == "hang":
            time.sleep(600)

HTTPServer(("127.0.0.1", PORT), H).serve_forever()
