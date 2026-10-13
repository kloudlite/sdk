"""Kloudlite SDK for Python — stdlib only.

Auth is a Bearer token (a session or CLI token). Every module hangs off the one
client, and every repository is named by ONE ``"owner/name"`` string — the owner
half being a person's handle or a team slug::

    from kloudlite import Kloudlite

    kl = Kloudlite("https://api.example.com", token="…")
    repo = kl.code_repos.create("acme/payments")
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
    def __init__(self, base_url: str, token: str | None = None,
                 hub_url: str | None = None, timeout: float = 30.0):
        self.base_url = base_url.rstrip("/")
        self.hub_url = hub_url.rstrip("/") if hub_url else None
        self._token = token
        self._timeout = timeout
        self.code_repos = CodeRepos(self)
        self.model_repos = HubRepos(self, "model")
        self.data_repos = HubRepos(self, "dataset")
        self.workspaces = Workspaces(self)
        self.volumes = Volumes(self)

    def regions(self):
        """The regions an admin has registered — a workspace is created into one of them."""
        return self.request("GET", "/v1/regions")

    def quota(self, owner: str | None = None):
        """The ceiling and what is against it; ``owner`` defaults to the caller's own."""
        return self.request("GET", "/v1/quota", query={"owner": owner})

    # ── request core ──────────────────────────────────────────────────────

    def request(self, method: str, path: str, body=None, query: dict | None = None):
        """One HTTP round trip. Returns parsed JSON (or None on an empty body);
        raises :class:`ApiError` on any non-2xx status."""
        return self._request(self.base_url, method, path, body, query)

    def hub_request(self, method: str, path: str, body=None, query: dict | None = None):
        """The same round trip against the hub listener (models and datasets)."""
        if not self.hub_url:
            raise ValueError("this client has no hub_url; pass one to reach models and datasets")
        return self._request(self.hub_url, method, path, body, query)

    def _request(self, base: str, method: str, path: str, body=None, query: dict | None = None):
        url = base + path
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


class CodeRepos:
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

    def import_git(self, repo: str, git: str, visibility: str = "private", description: str = ""):
        """A new repo whose contents are fetched from a PUBLIC git remote — ``git`` is a strict
        ``https://`` URL (GitHub is one host among any). Answers 202: the repo exists but the
        fetch runs in the background; poll :meth:`import_status`."""
        owner, name = _split(repo)
        return self._c.request("POST", "/v1/repos/import", {
            "owner": owner, "name": name, "git": git,
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


class Workspaces:
    """Btrfs-backed dev workspaces, over ``/v1/workspaces``. A workspace is named by its id (the
    ``name`` given at create); pushes cut snapshots, kept on the workspace's VOLUME — see
    :class:`Volumes` for history, restore targets and deletion."""

    def __init__(self, client: Kloudlite):
        self._c = client

    def create(self, name: str, region: str, quota_gb: int, team: str | None = None,
               image: str | None = None, repo: str | None = None, branch: str | None = None,
               packages: list | None = None):
        """``repo`` seeds from a PLATFORM repo ("owner/name", with ``branch``); ``packages`` are
        nixpkgs attributes, optionally pinned (``attr@version``)."""
        body = {"name": name, "region": region, "quota_gb": quota_gb,
                "team": team, "repo": repo, "branch": branch,
                "packages": packages or []}
        if image is not None:
            body["image"] = image
        return self._c.request("POST", "/v1/workspaces", body)

    def list(self, team: str | None = None):
        """The caller's workspaces; ``team`` scopes to their workspaces in that team."""
        return self._c.request("GET", "/v1/workspaces", query={"team": team})

    def get(self, id: str):
        return self._c.request("GET", f"/v1/workspaces/{_seg(id)}")

    def delete(self, id: str):
        """Runs the worktree finalizer: the volume survives while a pushed snapshot references it."""
        return self._c.request("DELETE", f"/v1/workspaces/{_seg(id)}")

    def start(self, id: str):
        return self._c.request("POST", f"/v1/workspaces/{_seg(id)}/start")

    def stop(self, id: str):
        """A stop cuts a sync point and tears the pod down — seconds, never waiting on a replica."""
        return self._c.request("POST", f"/v1/workspaces/{_seg(id)}/stop")

    def push(self, id: str, message: str | None = None):
        """Cut a SNAPSHOT of the working copy — the one durable, restorable state."""
        body = {"message": message} if message is not None else None
        return self._c.request("POST", f"/v1/workspaces/{_seg(id)}/push", body)

    def clone(self, id: str, name: str, task: str | None = None):
        """A new workspace from this one's state, cut at the moment of the request; the answer's
        ``based_on`` names exactly what it grafted onto."""
        return self._c.request("POST", f"/v1/workspaces/{_seg(id)}/clone",
                               {"name": name, "task": task})

    def restore(self, name: str, snapshot_id: str, image: str | None = None,
                packages: list | None = None, quota_gb: int | None = None):
        """A new workspace grafted onto a PUSHED snapshot — also one whose workspace is long
        gone. Absent overrides mean "whatever the snapshot froze"."""
        return self._c.request("POST", "/v1/workspaces/restore", {
            "name": name, "snapshot_id": snapshot_id,
            "image": image, "packages": packages, "quota_gb": quota_gb,
        })

    def set_packages(self, id: str, packages: list):
        """Replace the package list; pinned entries are locked before anything is written."""
        return self._c.request("PATCH", f"/v1/workspaces/{_seg(id)}", {"packages": packages})

    def update_packages(self, id: str):
        """Re-resolve every pinned package — the only call that moves existing locks."""
        return self._c.request("POST", f"/v1/workspaces/{_seg(id)}/packages/update")

    def tools(self, id: str):
        """The workspace's tool-server address — answered to the owner alone."""
        return self._c.request("GET", f"/v1/workspaces/{_seg(id)}/tools")


class Volumes:
    """The btrfs volumes behind workspaces and environments: snapshot history, restore targets,
    and the two deletes — over ``/v1/volumes``."""

    def __init__(self, client: Kloudlite):
        self._c = client

    def list(self, kind: str | None = None, owner: str | None = None):
        """``kind`` is ``workspace`` or ``environment``; ``owner`` a handle or team slug."""
        return self._c.request("GET", "/v1/volumes", query={"kind": kind, "owner": owner})

    def history(self, name: str):
        """The chain of PUSHED snapshots, newest first — each row a restore target."""
        return self._c.request("GET", f"/v1/volumes/{_seg(name)}/history")

    def refs(self, name: str):
        return self._c.request("GET", f"/v1/volumes/{_seg(name)}/refs")

    def delete_snapshot(self, name: str, snapshot_id: str):
        """Refused (409) for a sync point or a running worktree's base; deleting a detached
        volume's last snapshot deletes the volume."""
        return self._c.request("DELETE", f"/v1/volumes/{_seg(name)}/snapshots/{_seg(snapshot_id)}")

    def delete(self, name: str):
        """Takes a DETACHED volume with all its snapshots; refused while a working copy remains."""
        return self._c.request("DELETE", f"/v1/volumes/{_seg(name)}")


class HubRepos:
    """Model and dataset repositories, on the hub listener (Hugging Face Hub API).
    Management level: create, read, refs, visibility, delete — files move with
    git/`huggingface_hub` against the same hub URL. Every ``repo`` is one
    ``"owner/name"`` string, exactly as in :class:`CodeRepos`."""

    def __init__(self, client: Kloudlite, kind: str):
        self._c = client
        self._kind = kind
        self._plural = {"model": "models", "dataset": "datasets"}[kind]

    def _body(self, repo: str, extra: dict | None = None) -> dict:
        owner, name = _split(repo)
        out = {"name": name, "organization": owner, "type": self._kind}
        out.update(extra or {})
        return out

    def _path(self, repo: str, tail: str = "") -> str:
        owner, name = _split(repo)
        return f"/api/{self._plural}/{_seg(owner)}/{_seg(name)}{tail}"

    def create(self, repo: str, private: bool = True):
        """Sent explicitly rather than trusting a server default: a model that was meant to be
        private must not hinge on one."""
        return self._c.hub_request("POST", "/api/repos/create", self._body(repo, {"private": private}))

    def get(self, repo: str):
        return self._c.hub_request("GET", self._path(repo))

    def refs(self, repo: str):
        """Branches and tags, as the hub spells them."""
        return self._c.hub_request("GET", self._path(repo, "/refs"))

    def set_private(self, repo: str, private: bool):
        return self._c.hub_request("PUT", self._path(repo, "/settings"), {"private": private})

    def delete(self, repo: str):
        return self._c.hub_request("DELETE", "/api/repos/delete", self._body(repo))

    def import_git(self, repo: str, git: str, private: bool = True):
        """A new repo whose contents — large files included, through git-lfs — are fetched from a
        PUBLIC git remote; ``git`` is a strict ``https://`` URL (the Hugging Face Hub is one host
        among any). Answers 202; poll :meth:`import_status`."""
        return self._c.hub_request("POST", self._path(repo, "/import"), {"from": git, "private": private})

    def import_status(self, repo: str):
        """``{"from", "state": queued|running|done|failed, "detail"?}`` for an imported repo."""
        return self._c.hub_request("GET", self._path(repo, "/import"))


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
