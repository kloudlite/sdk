"""Kloudlite SDK for Python — stdlib only.

Auth is a Bearer token (a session or CLI token). Every module hangs off the one
client::

    from kloudlite import Kloudlite

    kl = Kloudlite("https://api.example.com", token="…")
    repo = kl.repos.create(owner="acme", name="payments")
"""

from __future__ import annotations

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
    def __init__(self, base_url: str, token: str | None = None, timeout: float = 30.0):
        self.base_url = base_url.rstrip("/")
        self._token = token
        self._timeout = timeout
        self.repos = Repos(self)

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
        if self._token:
            headers["Authorization"] = f"Bearer {self._token}"
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


def _seg(s: str) -> str:
    return urllib.parse.quote(str(s), safe="")


class Repos:
    """Code repositories: ``/v1/repos``."""

    def __init__(self, client: Kloudlite):
        self._c = client

    # repos
    def create(self, owner: str, name: str, visibility: str = "private", description: str = ""):
        return self._c.request("POST", "/v1/repos", {
            "owner": owner, "name": name, "visibility": visibility, "description": description,
        })

    def list(self, owner: str):
        return self._c.request("GET", "/v1/repos", query={"owner": owner})

    def get(self, owner: str, name: str):
        return self._c.request("GET", f"/v1/repos/{_seg(owner)}/{_seg(name)}")

    def update(self, owner: str, name: str, description: str | None = None, visibility: str | None = None):
        return self._c.request("PATCH", f"/v1/repos/{_seg(owner)}/{_seg(name)}", {
            "description": description, "visibility": visibility,
        })

    def delete(self, owner: str, name: str):
        return self._c.request("DELETE", f"/v1/repos/{_seg(owner)}/{_seg(name)}")

    def fork(self, src_owner: str, src_name: str, owner: str, name: str,
             visibility: str = "private", description: str = ""):
        """A new repo copied from another: objects and refs, never its pull requests or settings.
        Private by default, whatever the source was."""
        return self._c.request("POST", f"/v1/repos/{_seg(src_owner)}/{_seg(src_name)}/fork", {
            "owner": owner, "name": name, "visibility": visibility, "description": description,
        })

    # branches
    def branches(self, owner: str, name: str):
        return self._c.request("GET", f"/v1/repos/{_seg(owner)}/{_seg(name)}/branches")

    def delete_branch(self, owner: str, name: str, branch: str, oid: str):
        """`oid` is the head the caller saw; the delete is refused if the branch moved."""
        return self._c.request(
            "DELETE", f"/v1/repos/{_seg(owner)}/{_seg(name)}/branches/{_seg(branch)}",
            query={"oid": oid},
        )

    # protection
    def protection(self, owner: str, name: str):
        return self._c.request("GET", f"/v1/repos/{_seg(owner)}/{_seg(name)}/protection")

    def protect(self, owner: str, name: str, pattern: str, remove: bool = False,
                no_force: bool = True, no_delete: bool = True):
        return self._c.request("POST", f"/v1/repos/{_seg(owner)}/{_seg(name)}/protection", {
            "pattern": pattern, "remove": remove, "no_force": no_force, "no_delete": no_delete,
        })

    # pull requests
    def pulls(self, owner: str, name: str, state: str | None = None, limit: int | None = None):
        return self._c.request("GET", f"/v1/repos/{_seg(owner)}/{_seg(name)}/pulls",
                               query={"state": state, "limit": limit})

    def pull(self, owner: str, name: str, number: int):
        return self._c.request("GET", f"/v1/repos/{_seg(owner)}/{_seg(name)}/pulls/{number}")

    def open_pull(self, owner: str, name: str, title: str, head: str, base: str,
                  description: str = "", head_repo: str | None = None):
        """``head_repo`` ("owner/name") makes it a cross-repo pull: ``head`` then names a branch
        of that fork, proposed onto this repo's ``base``."""
        return self._c.request("POST", f"/v1/repos/{_seg(owner)}/{_seg(name)}/pulls", {
            "title": title, "head": head, "base": base, "description": description,
            "head_repo": head_repo,
        })

    def comment(self, owner: str, name: str, number: int, body: str):
        return self._c.request("POST", f"/v1/repos/{_seg(owner)}/{_seg(name)}/pulls/{number}/comments",
                               {"body": body})

    def merge(self, owner: str, name: str, number: int, strategy: str = "fast-forward"):
        """strategy: fast-forward, squash, merge or rebase. Answers 202 — the merge is a job."""
        return self._c.request("POST", f"/v1/repos/{_seg(owner)}/{_seg(name)}/pulls/{number}/merge",
                               query={"strategy": strategy})

    def close(self, owner: str, name: str, number: int):
        return self._c.request("POST", f"/v1/repos/{_seg(owner)}/{_seg(name)}/pulls/{number}/close")

    # commits
    def compare(self, owner: str, name: str, base: str, head: str):
        return self._c.request("GET", f"/v1/repos/{_seg(owner)}/{_seg(name)}/compare",
                               query={"base": base, "head": head})

    def commit(self, owner: str, name: str, branch: str, message: str, changes: list,
               expect: str | None = None, new_branch: str | None = None):
        """One commit of file changes; each change is
        ``{"path", "contentBase64"?, "executable"?, "delete"?}``."""
        return self._c.request("POST", f"/v1/repos/{_seg(owner)}/{_seg(name)}/commits", {
            "branch": branch, "message": message, "changes": changes,
            "expect": expect, "newBranch": new_branch,
        })

    def signature(self, owner: str, name: str, sha: str):
        return self._c.request("GET", f"/v1/repos/{_seg(owner)}/{_seg(name)}/commits/{_seg(sha)}/signature")


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
