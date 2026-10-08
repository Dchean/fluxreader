#!/usr/bin/env python3
"""TASK-059 实机验证用本地后端：模拟 FreshRSS / Miniflux 两种 API 布局。

为什么需要：Rust 侧的 mock 已覆盖「纯域名能连」，但**实机证据**要求真实运行的应用
在**只填域名**时连上真实协议服务端。两种后端的差异正是「API 放在哪个子路径」，
所以这里用 `--layout` 精确复现：

- `freshrss`：GReader API 在 `{域名}/api/greader.php`（**根路径 404**），Fever 在 `/api/fever.php`；
- `miniflux`：GReader API 在**站点根**，Fever 在 `/fever/`。

只依赖标准库。每个请求返回时在 stdout 打一行记录（由驱动脚本捕获成证据），用于证明：
① 纯域名时确实发生了有界探测；② 完整路径时首个候选即命中。

用法：
    python tools/mock_greader_ui_server.py --layout freshrss --port 8899
"""

import argparse
import hashlib
import json
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, urlparse

# 只接受这一组凭据：密码不符时返回 401（用于验证「凭据错不得被说成地址错」）
GOOD_USER = "demo"
GOOD_PASS = "demo-pass"

FEED_XML = """<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0"><channel>
<title>{title}</title><link>http://localhost/</link><description>t059 mock</description>
<item><title>{title} 文章一</title><link>http://localhost/{slug}/1</link>
<description>t059 内容</description><guid>t059-{slug}-1</guid></item>
<item><title>{title} 文章二</title><link>http://localhost/{slug}/2</link>
<description>t059 内容</description><guid>t059-{slug}-2</guid></item>
</channel></rss>
"""

SUBSCRIPTIONS = [
    ("feed/10", "T059 源一", "/feed1.xml"),
    ("feed/11", "T059 源二", "/feed2.xml"),
]

# 已知的回环地址：本服务只监听它，接受任意端口即可
LOOPBACK = "127.0.0.1"


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    layout = "freshrss"

    # ---------- 工具 ----------

    def _api_prefix(self):
        return "" if self.layout == "miniflux" else "/api/greader.php"

    def _fever_entry(self):
        return "/fever/" if self.layout == "miniflux" else "/api/fever.php"

    def _send(self, status, body, ctype="application/json"):
        payload = body.encode("utf-8") if isinstance(body, str) else body
        self.send_response(status)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)
        # 证据：请求行 + 状态码（驱动脚本捕获 stdout 落盘）
        print(f"REQ {self.command} {self.path} -> {status}", flush=True)

    def _not_found(self):
        self._send(404, '{"error":"not found"}')

    def _base_url(self):
        return f"http://{LOOPBACK}:{self.server.server_port}"

    # ---------- 路由 ----------

    def do_GET(self):
        parsed = urlparse(self.path)
        path = parsed.path
        prefix = self._api_prefix()

        if path in ("/feed1.xml", "/feed2.xml"):
            idx = "1" if path.endswith("1.xml") else "2"
            xml = FEED_XML.format(title=f"T059 源{idx}", slug=f"f{idx}")
            return self._send(200, xml, "application/rss+xml")

        if path == self._fever_entry():
            return self._fever(parsed.query)

        if path == f"{prefix}/reader/api/0/subscription/list":
            subs = [
                {
                    "id": fid,
                    "title": title,
                    "url": f"{self._base_url()}{url}",
                    "htmlUrl": f"{self._base_url()}{url}",
                    "categories": [
                        {"id": "user/-/label/T059 分类", "label": "T059 分类", "type": "folder"}
                    ],
                }
                for fid, title, url in SUBSCRIPTIONS
            ]
            return self._send(200, json.dumps({"subscriptions": subs}))

        if path == f"{prefix}/reader/api/0/tag/list":
            tags = [
                {"id": "user/-/state/com.google/starred"},
                {"id": "user/-/label/T059 分类", "label": "T059 分类", "type": "folder"},
            ]
            return self._send(200, json.dumps({"tags": tags}))

        if path == f"{prefix}/reader/api/0/stream/items/ids":
            return self._send(200, json.dumps({"itemRefs": [{"id": "1001"}, {"id": "1002"}]}))

        self._not_found()

    def do_POST(self):
        parsed = urlparse(self.path)
        path = parsed.path
        length = int(self.headers.get("Content-Length") or 0)
        body = self.rfile.read(length).decode("utf-8", "replace") if length else ""
        form = parse_qs(body)
        prefix = self._api_prefix()

        if path == self._fever_entry():
            return self._fever(parsed.query)

        if path == f"{prefix}/accounts/ClientLogin":
            email = (form.get("Email") or [""])[0]
            passwd = (form.get("Passwd") or [""])[0]
            if email != GOOD_USER or passwd != GOOD_PASS:
                # 路径正确、凭据被拒——客户端必须如实报凭据错，不得说成「找不到 API」
                return self._send(401, "Unauthorized!")
            return self._send(200, json.dumps({"SID": "s", "LSID": "l", "Auth": "t059/mocktoken"}))

        if path == f"{prefix}/reader/api/0/stream/items/contents":
            items = []
            for feed_id, title, _url in SUBSCRIPTIONS:
                for n in (1, 2):
                    items.append(
                        {
                            "id": f"100{n}",
                            "title": f"{title} 文章{n}",
                            "published": 1750000000,
                            "updated": 1750000000,
                            "canonical": [{"href": f"{self._base_url()}/f{n}"}],
                            "alternate": [{"href": f"{self._base_url()}/f{n}"}],
                            "origin": {"streamId": feed_id, "title": title},
                            "summary": {"content": f"t059 摘要 {n}"},
                        }
                    )
            return self._send(200, json.dumps({"items": items}))

        if path in (
            f"{prefix}/reader/api/0/edit-tag",
            f"{prefix}/reader/api/0/mark-all-as-read",
            f"{prefix}/reader/api/0/subscription/edit",
        ):
            return self._send(200, "OK", "text/plain")

        if path == f"{prefix}/reader/api/0/subscription/quickadd":
            return self._send(200, json.dumps({"streamId": "feed/99", "numResults": 1}))

        self._not_found()

    def _fever(self, query):
        """Fever 信封。FreshRSS 实测返回 api_version=4（TASK-059 已放宽到 >=3）。"""
        api_key = (parse_qs(query).get("api_key") or [""])[0]
        # Fever 协议规定 api_key = md5("username:password")——这是**协议规定的取值方式**，
        # 不是完整性/口令保护手段，故显式标注 usedforsecurity=False，与安全用途区分开。
        expected = hashlib.md5(  # noqa: S324 - 协议仿真用，非安全用途
            f"{GOOD_USER}:{GOOD_PASS}".encode(), usedforsecurity=False
        ).hexdigest()
        auth = 1 if api_key == expected else 0
        env = {
            "api_version": 4,
            "auth": auth,
            "groups": [{"id": 1, "title": "T059 分类"}],
            "feeds": [{"id": 10, "title": "T059 源一", "url": f"{self._base_url()}/feed1.xml"}],
            "feeds_groups": [{"feed_ids": "10", "group_id": 1}],
        }
        return self._send(200, json.dumps(env))

    def log_message(self, *args):
        pass  # 静音默认日志：请求记录走上方的 REQ 行


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--layout", choices=["freshrss", "miniflux"], required=True)
    ap.add_argument("--port", type=int, required=True)
    args = ap.parse_args()

    Handler.layout = args.layout
    srv = ThreadingHTTPServer((LOOPBACK, args.port), Handler)
    print(f"t059 mock backend ready: layout={args.layout} port={args.port}", flush=True)
    srv.serve_forever()


if __name__ == "__main__":
    main()