#!/usr/bin/env python3
"""Authenticated, Docker-bridge-only HTTP receiver for GNOME notifications."""

from __future__ import annotations

import hmac
import json
import logging
import os
import subprocess
from http import HTTPStatus
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer


MAX_BODY_BYTES = 8 * 1024
MAX_NOTIFICATION_TEXT = 2_000
VALID_URGENCIES = {"low", "normal", "critical"}


def required_env(name: str) -> str:
    value = os.environ.get(name, "")
    if not value:
        raise RuntimeError(f"{name} must be set")
    return value


API_KEY = required_env("GNOME_NOTIFICATION_SERVICE_API_KEY")
BIND_ADDRESS = os.environ.get("N8N_GNOME_NOTIFY_BIND_ADDRESS", "192.168.0.1")
PORT = int(os.environ.get("N8N_GNOME_NOTIFY_PORT", "8788"))


class NotificationHandler(BaseHTTPRequestHandler):
    server_version = "n8n-gnome-notify/1.0"

    def log_message(self, format: str, *args: object) -> None:
        logging.info("%s - %s", self.address_string(), format % args)

    def send_json(self, status: HTTPStatus, payload: dict[str, object]) -> None:
        data = json.dumps(payload).encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def authorized(self) -> bool:
        authorization = self.headers.get("Authorization", "")
        expected = f"Bearer {API_KEY}"
        return hmac.compare_digest(authorization, expected)

    def do_GET(self) -> None:  # noqa: N802
        if self.path != "/healthz":
            self.send_json(HTTPStatus.NOT_FOUND, {"error": "not_found"})
            return
        self.send_json(HTTPStatus.OK, {"status": "ok"})

    def do_POST(self) -> None:  # noqa: N802
        if self.path != "/v1/notifications":
            self.send_json(HTTPStatus.NOT_FOUND, {"error": "not_found"})
            return
        if not self.authorized():
            self.send_json(HTTPStatus.UNAUTHORIZED, {"error": "unauthorized"})
            return

        try:
            content_length = int(self.headers.get("Content-Length", "0"))
        except ValueError:
            content_length = 0
        if content_length <= 0 or content_length > MAX_BODY_BYTES:
            self.send_json(HTTPStatus.BAD_REQUEST, {"error": "invalid_content_length"})
            return

        try:
            payload = json.loads(self.rfile.read(content_length))
            title = str(payload["title"]).strip()[:MAX_NOTIFICATION_TEXT]
            body = str(payload["body"]).strip()[:MAX_NOTIFICATION_TEXT]
            urgency = str(payload.get("urgency", "normal"))
        except (json.JSONDecodeError, KeyError, TypeError, UnicodeDecodeError):
            self.send_json(HTTPStatus.BAD_REQUEST, {"error": "invalid_payload"})
            return

        if not title or not body or urgency not in VALID_URGENCIES:
            self.send_json(HTTPStatus.BAD_REQUEST, {"error": "invalid_notification"})
            return

        try:
            subprocess.run(
                ["notify-send", "--app-name=n8n", f"--urgency={urgency}", title, body],
                check=True,
                timeout=10,
            )
        except (OSError, subprocess.CalledProcessError, subprocess.TimeoutExpired):
            logging.exception("Could not deliver GNOME notification")
            self.send_json(HTTPStatus.SERVICE_UNAVAILABLE, {"error": "notification_failed"})
            return

        self.send_json(HTTPStatus.ACCEPTED, {"status": "accepted"})


def main() -> None:
    logging.basicConfig(level=logging.INFO, format="%(asctime)s %(levelname)s %(message)s")
    server = ThreadingHTTPServer((BIND_ADDRESS, PORT), NotificationHandler)
    logging.info("Listening on %s:%s", BIND_ADDRESS, PORT)
    server.serve_forever()


if __name__ == "__main__":
    main()
