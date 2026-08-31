"""本地 WebDAV mock（配置同步运行时验证用）：PUT 存 / GET 回单文件。"""
import http.server

class WebDAVMock(http.server.BaseHTTPRequestHandler):
    store: dict[str, str] = {}

    def do_PUT(self):
        length = int(self.headers.get('Content-Length', 0))
        body = self.rfile.read(length).decode('utf-8')
        WebDAVMock.store[self.path] = body
        self.send_response(201)
        self.send_header('Content-Length', '0')
        self.send_header('Connection', 'close')
        self.end_headers()

    def do_GET(self):
        body = WebDAVMock.store.get(self.path)
        if body is not None:
            data = body.encode('utf-8')
            self.send_response(200)
            self.send_header('Content-Type', 'application/json')
            self.send_header('Content-Length', str(len(data)))
            self.send_header('Connection', 'close')
            self.end_headers()
            self.wfile.write(data)
        else:
            self.send_response(404)
            self.send_header('Content-Length', '0')
            self.send_header('Connection', 'close')
            self.end_headers()

    def log_message(self, *a):
        pass

if __name__ == '__main__':
    srv = http.server.ThreadingHTTPServer(('127.0.0.1', 8777), WebDAVMock)
    print('webdav mock on 8777', flush=True)
    srv.serve_forever()
