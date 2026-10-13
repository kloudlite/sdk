"""Kloudlite SDK for Python — stdlib only.

The client speaks to the browse API with HTTP Basic auth: the username names the
owner the token belongs to (git's ``x`` placeholder also works), the token is the
secret. No credentials is still a valid client — public repositories admit
anonymous reads.

Usage::

    from kloudlite import Kloudlite

    kl = Kloudlite("https://git.example.com", username="acme", token="…")
"""

from __future__ import annotations

import base64
import json
import urllib.error
import urllib.parse
import urllib.request

__all__ = ["Kloudlite", "ApiError"]

VERSION = "0.1.0"
USER_AGENT = f"kloudlite-sdk-python/{VERSION}"


class ApiError(Exception):
    """A non-2xx answer. ``status`` is the HTTP code, ``message`` the server's text."""

    def __init__(self, status: int, message: str):
        super().__init__(f"{status}: {message}")
        self.status = status
        self.message = message


class Kloudlite:
    def __init__(self, base_url: str, username: str = "x", token: str | None = None, timeout: float = 30.0):
        self.base_url = base_url.rstrip("/")
        self._timeout = timeout
        self._auth = None
        if token:
            raw = f"{username}:{token}".encode()
            self._auth = "Basic " + base64.b64encode(raw).decode()

    # ── request core ──────────────────────────────────────────────────────

    def request(self, method: str, path: str, body=None, query: dict | None = None):
        """One HTTP round trip. Returns parsed JSON (or None on an empty body);
        raises :class:`ApiError` on any non-2xx status."""
        url = self.base_url + path
        if query:
            q = {k: v for k, v in query.items() if v is not None}
            if q:
                url += "?" + urllib.parse.urlencode(q)
        data = None
        headers = {"User-Agent": USER_AGENT, "Accept": "application/json"}
        if self._auth:
            headers["Authorization"] = self._auth
        if body is not None:
            data = json.dumps(body).encode()
            headers["Content-Type"] = "application/json"
        req = urllib.request.Request(url, data=data, headers=headers, method=method)
        try:
            with urllib.request.urlopen(req, timeout=self._timeout) as resp:
                raw = resp.read()
        except urllib.error.HTTPError as e:
            raise ApiError(e.code, _error_text(e.read())) from None
        if not raw:
            return None
        return json.loads(raw)

    @staticmethod
    def _segment(s: str) -> str:
        return urllib.parse.quote(s, safe="")


def _error_text(raw: bytes) -> str:
    """The server answers errors as ``{"error": …}`` or plain text; take either."""
    text = raw.decode(errors="replace").strip()
    try:
        parsed = json.loads(text)
        if isinstance(parsed, dict) and "error" in parsed:
            return str(parsed["error"])
    except ValueError:
        pass
    return text or "request failed"
