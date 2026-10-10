#!/usr/bin/env python3
"""The backend of the React record editor (fixtures/react_form): serves the
page and keeps one record, so a test can tell what was really submitted from
what the field showed.

  python3 react_form_server.py <port> <record file>

GET /record gives the record as JSON; POST /save sets its title. The record
file always holds the saved record (the test reads it).
"""
import json
import os
import sys
from http.server import HTTPServer, SimpleHTTPRequestHandler

port, store = int(sys.argv[1]), sys.argv[2]
here = os.path.join(os.path.dirname(os.path.abspath(__file__)), "react_form")
record = {"id": 7, "title": "Old title", "saves": 0}


def keep():
    with open(store + ".tmp", "w") as f:
        json.dump(record, f)
    os.replace(store + ".tmp", store)


class Handler(SimpleHTTPRequestHandler):
    def __init__(self, *a, **k):
        super().__init__(*a, directory=here, **k)

    def log_message(self, *a):
        pass

    def answer(self, body):
        data = json.dumps(body).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.send_header("Cache-Control", "no-store")
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self):
        if self.path == "/record":
            return self.answer(record)
        return super().do_GET()

    def do_POST(self):
        if self.path != "/save":
            return self.send_error(404)
        body = json.loads(self.rfile.read(int(self.headers.get("Content-Length", 0))) or b"{}")
        record["title"] = str(body.get("title", ""))
        record["saves"] += 1
        keep()
        self.answer(record)


keep()
HTTPServer(("127.0.0.1", port), Handler).serve_forever()
