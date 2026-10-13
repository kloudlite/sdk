"""Kloudlite SDK for Python — stdlib only.

Auth is a Bearer token (a session or CLI token). Every module hangs off the one
client, and every repository is named by ONE ``"owner/name"`` string — the owner
half being a person's handle or a team slug::

    from kloudlite import Kloudlite

    kl = Kloudlite("https://api.example.com", token="…")
    repo = kl.repos.create("acme/payments")
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


def _split(repo: str) -> tuple[str, str]:
    """``"owner/name"`` → its two halves, refused client-side when malformed —
    a shape the server would only ever 404."""
    owner, sep, name = repo.partition("/")
    if not sep or not owner or not name or "/" in name:
        raise ValueError(f"a repository is named owner/name, got {repo!r}")
    return owner, name


class Repos:
    """Code repositories: ``/v1/repos``. Every ``repo`` argument is one
    ``"owner/name"`` string; only owner-scoped calls (``list``) take a bare handle."""

    def __init__(self, client: Kloudlite):
        self._c = client

    def _path(self, repo: str, tail: str = "") -> str:
        owner, name = _split(repo)
        return f"/v1/repos/{_seg(owner)}/{_seg(name)}{tail}"

    # repos
    def create(self, repo: str, visibility: str = "private", description: str = ""):
        owner, name = _split(repo)
        return self._c.request("POST", "/v1/repos", {
            "owner": owner, "name": name, "visibility": visibility, "description": description,
        })

    def list(self, owner: str):
        return self._c.request("GET", "/v1/repos", query={"owner": owner})

    def get(self, repo: str):
        return self._c.request("GET", self._path(repo))

    def update(self, repo: str, description: str | None = None, visibility: str | None = None):
        return self._c.request("PATCH", self._path(repo), {
            "description": description, "visibility": visibility,
        })

    def delete(self, repo: str):
        return self._c.request("DELETE", self._path(repo))

    def fork(self, src: str, dst: str, visibility: str = "private", description: str = ""):
        """A new repo ``dst`` copied from ``src``: objects and refs, never its pull
        requests or settings. Private by default, whatever the source was."""
        owner, name = _split(dst)
        return self._c.request("POST", self._path(src, "/fork"), {
            "owner": owner, "name": name, "visibility": visibility, "description": description,
        })

    def import_github(self, repo: str, github: str, visibility: str = "private", description: str = ""):
        """A new repo whose contents are fetched from a PUBLIC GitHub repository — ``github`` is
        its ``"owner/name"`` pair, never a URL. Answers 202: the repo exists but the fetch runs
        in the background; poll :meth:`import_status`."""
        owner, name = _split(repo)
        return self._c.request("POST", "/v1/repos/import", {
            "owner": owner, "name": name, "github": github,
            "visibility": visibility, "description": description,
        })

    def import_status(self, repo: str):
        """``{"from", "state": queued|running|done|failed, "detail"?}`` for an imported repo."""
        return self._c.request("GET", self._path(repo, "/import"))

    # branches
    def branches(self, repo: str):
        return self._c.request("GET", self._path(repo, "/branches"))

    def delete_branch(self, repo: str, branch: str, oid: str):
        """`oid` is the head the caller saw; the delete is refused if the branch moved."""
        return self._c.request("DELETE", self._path(repo, f"/branches/{_seg(branch)}"),
                               query={"oid": oid})

    # protection
    def protection(self, repo: str):
        return self._c.request("GET", self._path(repo, "/protection"))

    def protect(self, repo: str, pattern: str, remove: bool = False,
                no_force: bool = True, no_delete: bool = True):
        return self._c.request("POST", self._path(repo, "/protection"), {
            "pattern": pattern, "remove": remove, "no_force": no_force, "no_delete": no_delete,
        })

    # pull requests
    def pulls(self, repo: str, state: str | None = None, limit: int | None = None):
        return self._c.request("GET", self._path(repo, "/pulls"),
                               query={"state": state, "limit": limit})

    def pull(self, repo: str, number: int):
        return self._c.request("GET", self._path(repo, f"/pulls/{int(number)}"))

    def open_pull(self, repo: str, title: str, head: str, base: str,
                  description: str = "", head_repo: str | None = None):
        """``head_repo`` (another ``"owner/name"``) makes it a cross-repo pull:
        ``head`` then names a branch of that fork, proposed onto this repo's ``base``."""
        if head_repo is not None:
            _split(head_repo)  # same shape rule as every other repo reference
        return self._c.request("POST", self._path(repo, "/pulls"), {
            "title": title, "head": head, "base": base, "description": description,
            "head_repo": head_repo,
        })

    def comment(self, repo: str, number: int, body: str):
        return self._c.request("POST", self._path(repo, f"/pulls/{int(number)}/comments"),
                               {"body": body})

    def merge(self, repo: str, number: int, strategy: str = "fast-forward"):
        """strategy: fast-forward, squash, merge or rebase. Answers 202 — the merge is a job."""
        return self._c.request("POST", self._path(repo, f"/pulls/{int(number)}/merge"),
                               query={"strategy": strategy})

    def close(self, repo: str, number: int):
        return self._c.request("POST", self._path(repo, f"/pulls/{int(number)}/close"))

    # commits
    def compare(self, repo: str, base: str, head: str):
        return self._c.request("GET", self._path(repo, "/compare"),
                               query={"base": base, "head": head})

    def commit(self, repo: str, branch: str, message: str, changes: list,
               expect: str | None = None, new_branch: str | None = None):
        """One commit of file changes; each change is
        ``{"path", "contentBase64"?, "executable"?, "delete"?}``."""
        return self._c.request("POST", self._path(repo, "/commits"), {
            "branch": branch, "message": message, "changes": changes,
            "expect": expect, "newBranch": new_branch,
        })

    def signature(self, repo: str, sha: str):
        return self._c.request("GET", self._path(repo, f"/commits/{_seg(sha)}/signature"))


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
