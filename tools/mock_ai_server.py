# -*- coding: utf-8 -*-
"""本地 mock OpenAI 兼容服务：用于无真实 Key 时端到端验证 AI 摘要/翻译链路。

端点：
  GET  /v1/models           → 模型列表（设置页「测试连通性」/模型下拉用）
  POST /v1/chat/completions → SSE 流式响应（打字机效果）

用法：python tools/mock_ai_server.py [port]   （默认 8123）
在 FluxReader 设置 → AI服务 → 自定义预设填 http://127.0.0.1:8123
"""
import json
import sys
import time
from http.server import BaseHTTPRequestHandler, HTTPServer

PORT = int(sys.argv[1]) if len(sys.argv) > 1 else 8123

CHUNKS = [
    "这是一段由本地 mock 服务返回的流式摘要，",
    "用于在没有真实 API Key 的情况下验证",
    "「打开文章 → 自动摘要/自动翻译 → 打字机渲染 → 落库缓存」全链路。",
]


class Handler(BaseHTTPRequestHandler):
    # HTTP/1.1 + 显式 Connection: close（SSE 无 Content-Length，靠连接关闭界定流边界；
    # e2e 测试里的 mock 也是这么发的，reqwest 消费正常）
    protocol_version = "HTTP/1.1"

    def log_message(self, fmt, *args):
        sys.stderr.write("[mock-ai] %s\n" % (fmt % args))

    def _close(self):
        # 每个响应都带 Connection: close 并标记关闭，防止 keep-alive 复用导致
        # 下一请求拼进同一 socket 触发 Bad request syntax
        self.close_connection = True
        self.send_header("Connection", "close")

    def _json(self, code, obj):
        body = json.dumps(obj, ensure_ascii=False).encode("utf-8")
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self._close()
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        if self.path.startswith("/v1/models"):
            self._json(200, {
                "object": "list",
                "data": [
                    {"id": "mock-chat", "object": "model", "owned_by": "mock"},
                    {"id": "mock-pro", "object": "model", "owned_by": "mock"},
                ],
            })
        else:
            self._json(404, {"error": {"message": "not found"}})

    def do_POST(self):
        if not self.path.startswith("/v1/chat/completions"):
            self._json(404, {"error": {"message": "not found"}})
            return
        length = int(self.headers.get("Content-Length") or 0)
        if length:
            json.loads(self.rfile.read(length))  # 请求体仅做合法性校验
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Cache-Control", "no-cache")
        self._close()
        self.end_headers()
        for piece in CHUNKS:
            frame = {
                "choices": [{"delta": {"content": piece}, "index": 0}],
            }
            self.wfile.write(b"data: " + json.dumps(frame, ensure_ascii=False).encode("utf-8") + b"\n\n")
            self.wfile.flush()
            time.sleep(0.25)
        # 终止帧：OpenAI 兼容格式 finish + usage
        self.wfile.write(b"data: " + json.dumps({
            "choices": [{"delta": {}, "finish_reason": "stop", "index": 0}],
        }).encode("utf-8") + b"\n\n")
        self.wfile.write(b"data: [DONE]\n\n")
        self.wfile.flush()


if __name__ == "__main__":
    print(f"mock OpenAI server on http://127.0.0.1:{PORT}  (Ctrl+C 停止)")
    # Threading：应用可能并发发起摘要+翻译，单线程会串流
    from http.server import ThreadingHTTPServer
    ThreadingHTTPServer(("127.0.0.1", PORT), Handler).serve_forever()
