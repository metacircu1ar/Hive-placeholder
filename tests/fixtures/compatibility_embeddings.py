"""Deterministic local embedding transport for the release corpus (no model)."""
import hashlib
from http.server import BaseHTTPRequestHandler, HTTPServer
import json
import signal
import sys


class Handler(BaseHTTPRequestHandler):
    def do_POST(self):
        request = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
        inputs = request['input']
        if isinstance(inputs, str):
            inputs = [inputs]
        data = []
        for index, value in enumerate(inputs):
            digest = hashlib.sha256(value.encode()).digest()
            data.append({'index': index, 'embedding': [float(byte + 1) for byte in digest[:8]]})
        body = json.dumps({'data': data}).encode()
        self.send_response(200)
        self.send_header('Content-Type', 'application/json')
        self.send_header('Content-Length', str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *_):
        return


if __name__ == '__main__':
    signal.signal(signal.SIGINT, lambda *_: sys.exit(0))
    with HTTPServer(('127.0.0.1', 0), Handler) as server:
        print(server.server_port, flush=True)
        server.serve_forever()
