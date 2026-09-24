#!/usr/bin/env python3
"""Loopback decider stub for test/compat/decider-v0_12_2.sh.

Binds 127.0.0.1 on an ephemeral port, writes the port to --port-file, and
answers every POST in Jev's wire shape. It never opens a connection of its
own. A request whose Authorization header isn't "Bearer <--key>" gets 401.

Choice questions are answered with the criterion named by --choice (or the
first criterion when that one wasn't offered) at probability 0.95, the rest
sharing 0.05. Proposition ("noul") questions are answered with 0.5, which
no threshold accepts. Each request's path and question names are appended
to --log as one JSON line.
"""

import argparse
import json
import os
import socketserver
import sys
from http.server import BaseHTTPRequestHandler, HTTPServer


class LoopbackServer(HTTPServer):
    """HTTPServer without the reverse DNS lookup in server_bind, so the stub
    does no name resolution and starts at once."""

    def server_bind(self):
        socketserver.TCPServer.server_bind(self)
        self.server_name = "127.0.0.1"
        self.server_port = self.server_address[1]


def answer(question, choice):
    kind = question.get("type")
    if kind == "choice":
        keys = list(question.get("criteria", {}).keys())
        winner = choice if choice in keys else keys[0]
        rest = 0.05 / (len(keys) - 1)
        probs = {k: (0.95 if k == winner else rest) for k in keys}
        return {"type": "choice", "choice": winner, "probabilities": probs, "confidence": 0.95}
    if kind == "noul":
        return {"type": "noul", "noul": 0.5}
    raise ValueError("unknown question type")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--port-file", required=True)
    parser.add_argument("--key", required=True)
    parser.add_argument("--log", required=True)
    parser.add_argument("--choice", default="auto")
    args = parser.parse_args()

    class Handler(BaseHTTPRequestHandler):
        def reply(self, status, body):
            data = json.dumps(body).encode()
            self.send_response(status)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(data)))
            self.send_header("Connection", "close")
            self.end_headers()
            self.wfile.write(data)

        def do_POST(self):
            length = int(self.headers.get("Content-Length", "0"))
            raw = self.rfile.read(length)
            if self.headers.get("Authorization") != "Bearer " + args.key:
                self.reply(401, {"error": "bad key"})
                return
            try:
                questions = json.loads(raw)["questions"]
                answers = {name: answer(q, args.choice) for name, q in questions.items()}
            except (ValueError, KeyError, TypeError, IndexError, ZeroDivisionError):
                self.reply(400, {"error": "bad request"})
                return
            with open(args.log, "a") as log:
                log.write(json.dumps({"path": self.path, "questions": sorted(questions)}) + "\n")
            self.reply(200, {"model": "compat-stub-1.0.0", "answers": answers, "usage": {}})

        def log_message(self, fmt, *a):
            pass

    server = LoopbackServer(("127.0.0.1", 0), Handler)
    tmp = args.port_file + ".tmp"
    with open(tmp, "w") as f:
        f.write(str(server.server_address[1]))
    # Rename so a reader never sees a half-written port.
    os.replace(tmp, args.port_file)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    return 0


if __name__ == "__main__":
    sys.exit(main())
