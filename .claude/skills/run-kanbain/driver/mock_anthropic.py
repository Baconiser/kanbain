"""Scripted fake Anthropic Messages API for sandbox end-to-end tests.

Requests carrying a Bash tool get the next scripted bash command as a
tool_use, until the script is exhausted; then an end_turn text. Requests
without tools (titles, summaries) get a short text. Every response reports
usage, so the proxy can price it. OpenAI-compatible `/v1/chat/completions`
works the same way (for OpenCode's OpenAI-compatible provider).
Usage: mock_anthropic.py PORT SCRIPT.json [LOG]
SCRIPT.json: {"commands": ["...", ...], "output_tokens": 50000,
              "markers": ["text to look for in the system prompt", ...]}
Each request is logged with its tool names, and which markers appear in the
system prompt (`system_markers`) or anywhere in the request (`markers`).
"""
import json
import sys
import uuid
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

PORT = int(sys.argv[1])
SCRIPT = json.load(open(sys.argv[2]))
LOG = open(sys.argv[3] if len(sys.argv) > 3 else "/dev/null", "a")


def sse(event, data):
    return f"event: {event}\ndata: {json.dumps(data)}\n\n".encode()


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *args):
        pass

    def do_POST(self):
        raw = self.rfile.read(int(self.headers.get("content-length", 0))) or b"{}"
        body = json.loads(raw)
        path = self.path.split("?")[0]
        openai = path.endswith("/chat/completions")
        tools = [t.get("name") or (t.get("function") or {}).get("name")
                 for t in body.get("tools") or []]
        markers = SCRIPT.get("markers", [])
        system = system_text(body, openai)
        text = raw.decode("utf-8", "replace")
        LOG.write(json.dumps({"path": self.path, "auth": self.headers.get("x-api-key")
                              or self.headers.get("authorization"),
                              "model": body.get("model"), "tools": tools,
                              "system_markers": [m for m in markers if m in system],
                              "markers": [m for m in markers if m in text]}) + "\n")
        LOG.flush()
        if path.endswith("/count_tokens"):
            return self.json({"input_tokens": 100})
        if openai:
            return self.chat_completion(body, tools)
        if path != "/v1/messages":
            return self.json({"type": "error", "error": {"type": "not_found_error", "message": path}}, 404)

        model = body.get("model", "claude-sonnet-5-5")
        bash = "Bash" if "Bash" in tools else "bash"
        tool_results = sum(
            1
            for m in body.get("messages", [])
            if m.get("role") == "user" and isinstance(m.get("content"), list)
            for c in m["content"]
            if c.get("type") == "tool_result"
        )
        commands = SCRIPT["commands"]
        if bash in tools and tool_results < len(commands):
            block = {"type": "tool_use", "id": "toolu_" + uuid.uuid4().hex[:20], "name": bash,
                     "input": {"command": commands[tool_results], "description": "scripted step"}}
            stop = "tool_use"
        else:
            block = {"type": "text", "text": "Done. KAN work committed."}
            stop = "end_turn"
        out_tokens = SCRIPT.get("output_tokens", 200) if bash in tools else 20
        usage_start = {"input_tokens": 1500, "output_tokens": 1,
                       "cache_creation_input_tokens": 0, "cache_read_input_tokens": 0}

        if not body.get("stream"):
            return self.json({"id": "msg_" + uuid.uuid4().hex, "type": "message", "role": "assistant",
                              "model": model, "content": [block], "stop_reason": stop,
                              "stop_sequence": None,
                              "usage": dict(usage_start, output_tokens=out_tokens)})

        self.send_response(200)
        self.send_header("content-type", "text/event-stream")
        self.send_header("cache-control", "no-cache")
        self.send_header("transfer-encoding", "chunked")
        self.end_headers()

        def chunk(data):
            self.wfile.write(f"{len(data):x}\r\n".encode() + data + b"\r\n")
            self.wfile.flush()

        chunk(sse("message_start", {"type": "message_start", "message": {
            "id": "msg_" + uuid.uuid4().hex, "type": "message", "role": "assistant", "model": model,
            "content": [], "stop_reason": None, "stop_sequence": None, "usage": usage_start}}))
        if block["type"] == "tool_use":
            # Long text first so a small budget trips mid-stream.
            filler = SCRIPT.get("filler_chars", 0)
            idx = 0
            if filler:
                chunk(sse("content_block_start", {"type": "content_block_start", "index": 0,
                                                   "content_block": {"type": "text", "text": ""}}))
                for _ in range(filler // 1000):
                    chunk(sse("content_block_delta", {"type": "content_block_delta", "index": 0,
                                                       "delta": {"type": "text_delta", "text": "x" * 1000}}))
                chunk(sse("content_block_stop", {"type": "content_block_stop", "index": 0}))
                idx = 1
            chunk(sse("content_block_start", {"type": "content_block_start", "index": idx,
                                               "content_block": dict(block, input={})}))
            chunk(sse("content_block_delta", {"type": "content_block_delta", "index": idx,
                                               "delta": {"type": "input_json_delta",
                                                         "partial_json": json.dumps(block["input"])}}))
            chunk(sse("content_block_stop", {"type": "content_block_stop", "index": idx}))
        else:
            chunk(sse("content_block_start", {"type": "content_block_start", "index": 0,
                                               "content_block": {"type": "text", "text": ""}}))
            chunk(sse("content_block_delta", {"type": "content_block_delta", "index": 0,
                                               "delta": {"type": "text_delta", "text": block["text"]}}))
            chunk(sse("content_block_stop", {"type": "content_block_stop", "index": 0}))
        chunk(sse("message_delta", {"type": "message_delta",
                                     "delta": {"stop_reason": stop, "stop_sequence": None},
                                     "usage": {"output_tokens": out_tokens}}))
        chunk(sse("message_stop", {"type": "message_stop"}))
        chunk(b"")

    def chat_completion(self, body, tools):
        model = body.get("model", "gpt-mock")
        tool_results = sum(1 for m in body.get("messages", []) if m.get("role") == "tool")
        commands = SCRIPT["commands"]
        if "bash" in tools and tool_results < len(commands):
            call = {"index": 0, "id": "call_" + uuid.uuid4().hex[:20], "type": "function",
                    "function": {"name": "bash", "arguments": json.dumps(
                        {"command": commands[tool_results], "description": "scripted step"})}}
            message, finish = {"role": "assistant", "content": None, "tool_calls": [call]}, "tool_calls"
        else:
            message, finish = {"role": "assistant", "content": "Done. KAN work committed."}, "stop"
        usage = {"prompt_tokens": 1500, "completion_tokens": SCRIPT.get("output_tokens", 200),
                 "total_tokens": 1500 + SCRIPT.get("output_tokens", 200)}
        cid = "chatcmpl-" + uuid.uuid4().hex
        if not body.get("stream"):
            return self.json({"id": cid, "object": "chat.completion", "model": model,
                              "choices": [{"index": 0, "message": message, "finish_reason": finish}],
                              "usage": usage})
        self.send_response(200)
        self.send_header("content-type", "text/event-stream")
        self.send_header("cache-control", "no-cache")
        self.send_header("transfer-encoding", "chunked")
        self.end_headers()

        def chunk(obj):
            data = b"data: " + (obj if isinstance(obj, bytes) else json.dumps(obj).encode()) + b"\n\n"
            self.wfile.write(f"{len(data):x}\r\n".encode() + data + b"\r\n")
            self.wfile.flush()

        base = {"id": cid, "object": "chat.completion.chunk", "model": model}
        chunk(dict(base, choices=[{"index": 0, "delta": message, "finish_reason": None}]))
        chunk(dict(base, choices=[{"index": 0, "delta": {}, "finish_reason": finish}]))
        chunk(dict(base, choices=[], usage=usage))
        chunk(b"[DONE]")
        self.wfile.write(b"0\r\n\r\n")
        self.wfile.flush()

    def do_GET(self):
        self.json({"data": []})

    def json(self, obj, status=200):
        data = json.dumps(obj).encode()
        self.send_response(status)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)


def system_text(body, openai):
    if openai:
        parts = [m.get("content") for m in body.get("messages", []) if m.get("role") == "system"]
    else:
        parts = [body.get("system")]
    out = []
    for part in parts:
        if isinstance(part, str):
            out.append(part)
        elif isinstance(part, list):
            out.extend(p.get("text", "") for p in part if isinstance(p, dict))
    return "\n".join(out)


ThreadingHTTPServer(("127.0.0.1", PORT), Handler).serve_forever()
