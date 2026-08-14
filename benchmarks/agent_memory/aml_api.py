from __future__ import annotations

import argparse
import hmac
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
from typing import Any
from urllib.parse import urlsplit

from benchmarks.agent_memory.lwc_backend import AdapterError, ConflictError, LwcBackend


_MAX_BODY_BYTES = 8 * 1024 * 1024


class _Server(ThreadingHTTPServer):
    daemon_threads = True

    def __init__(
        self,
        address: tuple[str, int],
        backend: LwcBackend,
        api_key: str | None,
    ) -> None:
        super().__init__(address, _Handler)
        self.backend = backend
        self.api_key = api_key


class _Handler(BaseHTTPRequestHandler):
    server: _Server

    def log_message(self, format: str, *args: object) -> None:
        pass

    def do_GET(self) -> None:
        if urlsplit(self.path).path == "/health":
            self._json(200, {"status": "ok"})
            return
        self._error(404, "not_found", "endpoint not found")

    def do_POST(self) -> None:
        path = urlsplit(self.path).path
        if path not in {"/add", "/search"}:
            self._error(404, "not_found", "endpoint not found")
            return
        if not self._authenticated():
            self._error(401, "unauthorized", "authentication required")
            return
        try:
            payload = self._read_payload()
            if path == "/add":
                self._add(payload)
            else:
                self._search(payload)
        except ValueError as error:
            self._error(422, "validation_error", str(error))
        except ConflictError:
            self._error(409, "identity_conflict", "request_id was reused with different content")
        except AdapterError:
            self._error(500, "backend_error", "memory backend failed")

    def _add(self, payload: dict[str, Any]) -> None:
        request_id = _required_text(payload.get("request_id"), "request_id")
        user_id = _required_text(payload.get("user_id"), "user_id")
        session_id = _required_text(payload.get("session_id"), "session_id")
        messages = payload.get("messages")
        if not isinstance(messages, list) or not messages:
            raise ValueError("messages must be a non-empty list")
        for message in messages:
            if not isinstance(message, dict):
                raise ValueError("each message must be an object")
            _required_text(message.get("role"), "message.role")
            _required_text(message.get("content"), "message.content")

        self.server.backend.add(user_id, request_id, session_id, messages)
        self._json(
            200,
            {
                "success": True,
                "request_id": request_id,
                "user_id": user_id,
                "session_id": session_id,
            },
        )

    def _search(self, payload: dict[str, Any]) -> None:
        query = _required_text(payload.get("query"), "query")
        user_id = _required_text(payload.get("user_id"), "user_id")
        top_k = payload.get("top_k")
        if not isinstance(top_k, int) or isinstance(top_k, bool) or top_k < 1:
            raise ValueError("top_k must be a positive integer")
        options = payload.get("options")
        if options is not None and (
            not isinstance(options, list)
            or any(not isinstance(option, str) for option in options)
        ):
            raise ValueError("options must be a list of strings")

        evidence = self.server.backend.search(user_id, query, min(top_k, 100))
        self._json(
            200,
            {
                "data": [
                    {
                        "id": item.id,
                        "content": item.content,
                        "score": item.score,
                        **({"created_at": item.created_at} if item.created_at else {}),
                    }
                    for item in evidence
                ]
            },
        )

    def _read_payload(self) -> dict[str, Any]:
        value = self.headers.get("Content-Length")
        try:
            length = int(value or "")
        except ValueError as error:
            raise ValueError("Content-Length must be an integer") from error
        if length < 1:
            raise ValueError("request body must not be empty")
        if length > _MAX_BODY_BYTES:
            self.close_connection = True
            raise ValueError("request body is too large")
        try:
            payload = json.loads(self.rfile.read(length))
        except (UnicodeDecodeError, json.JSONDecodeError) as error:
            raise ValueError("request body must be valid JSON") from error
        if not isinstance(payload, dict):
            raise ValueError("request body must be a JSON object")
        return payload

    def _authenticated(self) -> bool:
        expected = self.server.api_key
        if expected is None:
            return True
        authorization = self.headers.get("Authorization", "")
        supplied = self.headers.get("X-Api-Key", "")
        for prefix in ("Bearer ", "Token "):
            if authorization.startswith(prefix):
                supplied = authorization[len(prefix) :]
                break
        return hmac.compare_digest(supplied, expected)

    def _error(self, status: int, code: str, message: str) -> None:
        self._json(status, {"error": {"code": code, "message": message}})

    def _json(self, status: int, payload: object) -> None:
        body = json.dumps(payload, ensure_ascii=False, separators=(",", ":")).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)


def _required_text(value: object, name: str) -> str:
    if not isinstance(value, str) or not value.strip():
        raise ValueError(f"{name} must be a non-empty string")
    return value


def create_server(
    backend: LwcBackend,
    host: str = "127.0.0.1",
    port: int = 8080,
    api_key: str | None = None,
) -> ThreadingHTTPServer:
    return _Server((host, port), backend, api_key)


def main() -> None:
    parser = argparse.ArgumentParser(description="Serve the AML Add/Search contract")
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int, default=8080)
    parser.add_argument("--state-root", type=Path, required=True)
    parser.add_argument("--lwc-binary", default="lwc")
    args = parser.parse_args()
    server = create_server(
        LwcBackend(args.state_root, binary=args.lwc_binary),
        host=args.host,
        port=args.port,
        api_key=os.environ.get("AML_MEMORY_API_KEY"),
    )
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        server.server_close()


if __name__ == "__main__":
    main()
