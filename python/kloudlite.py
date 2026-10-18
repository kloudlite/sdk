"""Kloudlite SDK for Python — stdlib only.

Auth is a Bearer token (a session or CLI token). Every module hangs off the one
client, and every repository is named by ONE ``"owner/name"`` string — the owner
half being a person's handle or a team slug::

    from kloudlite import Kloudlite

    kl = Kloudlite(base_url="https://api.example.com", token="…")
    repo = kl.code_repos.create(repo="acme/payments")

Every public call takes its inputs BY NAME (keyword-only), so no caller ever has to remember
argument order.
"""

from __future__ import annotations

import asyncio
import base64
import contextlib
import functools
import json
import math
import threading
import time
import urllib.error
import urllib.parse
import urllib.request

__all__ = ["Kloudlite", "ApiError", "SandboxTerminated"]

VERSION = "0.1.0"
USER_AGENT = f"kloudlite-sdk-python/{VERSION}"


class ApiError(Exception):
    """A non-2xx answer. ``status`` is the HTTP code, ``message`` the server's text."""

    def __init__(self, status: int, message: str):
        super().__init__(f"{status}: {message}")
        self.status = status
        self.message = message


class Kloudlite:
    def __init__(self, *, base_url: str, token: str | None = None,
                 hub_url: str | None = None, timeout: float = 30.0):
        self.base_url = base_url.rstrip("/")
        self.hub_url = hub_url.rstrip("/") if hub_url else None
        self._token = token
        self._timeout = timeout
        self.code_repos = CodeRepos(self)
        self.model_repos = HubRepos(self, "model")
        self.data_repos = HubRepos(self, "dataset")
        self.workspaces = Workspaces(self)
        self.environments = Environments(self)
        self.sessions = Sessions(self)
        self.volumes = Volumes(self)
        self.secrets = Secrets(self)
        self.sandboxes = Sandboxes(self)
        self.pipelines = Pipelines(self)

    def quota(self, *, owner: str | None = None):
        """The ceiling and what is against it; ``owner`` defaults to the caller's own."""
        return self.request("GET", "/v1/quota", query={"owner": owner})

    # ── request core ──────────────────────────────────────────────────────

    def request(self, method: str, path: str, *, body=None, query: dict | None = None):
        """One HTTP round trip. Returns parsed JSON (or None on an empty body);
        raises :class:`ApiError` on any non-2xx status."""
        return self._request(self.base_url, method, path, body, query)

    def hub_request(self, method: str, path: str, *, body=None, query: dict | None = None):
        """The same round trip against the hub listener (models and datasets)."""
        if not self.hub_url:
            raise ValueError("this client has no hub_url; pass one to reach models and datasets")
        return self._request(self.hub_url, method, path, body, query)

    def _request(self, base: str, method: str, path: str, body=None, query: dict | None = None):
        req = self._prepare(base, method, path, body, query, "application/json")
        try:
            with urllib.request.urlopen(req, timeout=self._timeout) as resp:
                raw = resp.read()
        except urllib.error.HTTPError as e:
            raise ApiError(e.code, _error_text(e.read())) from None
        if not raw:
            return None
        return json.loads(raw)

    def _ndjson(self, path: str, query: dict | None = None):
        """GET an NDJSON stream from the api and yield each line, decoded, as it arrives. Same
        auth, base URL and :class:`ApiError` mapping as :meth:`request`, but no read timeout: a
        process may print nothing for longer than any round trip should take, and the server
        ends the stream itself after the exit frame."""
        req = self._prepare(self.base_url, "GET", path, None, query, "application/x-ndjson")
        try:
            resp = urllib.request.urlopen(req, timeout=None)
        except urllib.error.HTTPError as e:
            raise ApiError(e.code, _error_text(e.read())) from None
        with resp:
            for line in resp:  # readline on a chunked body returns each line once it lands
                line = line.strip()
                if line:
                    yield json.loads(line)

    def _prepare(self, base: str, method: str, path: str, body, query: dict | None, accept: str):
        url = base + path
        if query:
            q = {k: v for k, v in query.items() if v is not None}
            if q:
                url += "?" + urllib.parse.urlencode(q)
        data = None
        headers = {"User-Agent": USER_AGENT, "Accept": accept}
        if self._token:
            headers["Authorization"] = f"Bearer {self._token}"
        if body is not None:
            data = json.dumps(body).encode()
            headers["Content-Type"] = "application/json"
        return urllib.request.Request(url, data=data, headers=headers, method=method)


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
    def create(self, *, repo: str, visibility: str = "private", description: str = ""):
        owner, name = _split(repo)
        return self._c.request("POST", "/v1/repos", body={
            "owner": owner, "name": name, "visibility": visibility, "description": description,
        })

    def list(self, *, owner: str):
        return self._c.request("GET", "/v1/repos", query={"owner": owner})

    def get(self, *, repo: str):
        return self._c.request("GET", self._path(repo))

    def update(self, *, repo: str, description: str | None = None, visibility: str | None = None):
        return self._c.request("PATCH", self._path(repo), body={
            "description": description, "visibility": visibility,
        })

    def delete(self, *, repo: str):
        return self._c.request("DELETE", self._path(repo))

    def fork(self, *, repo: str, to: str, visibility: str = "private", description: str = ""):
        """A new repo ``to`` copied from ``repo``: objects and refs, never its pull
        requests or settings. Private by default, whatever the source was."""
        owner, name = _split(to)
        return self._c.request("POST", self._path(repo, "/fork"), body={
            "owner": owner, "name": name, "visibility": visibility, "description": description,
        })

    def import_git(self, *, repo: str, url: str, visibility: str = "private", description: str = ""):
        """A new repo whose contents are fetched from a PUBLIC git remote — ``url`` is a strict
        ``https://`` URL (GitHub is one host among any; the wire field stays ``git``). Answers
        202: the repo exists but the fetch runs in the background; poll :meth:`import_status`."""
        owner, name = _split(repo)
        return self._c.request("POST", "/v1/repos/import", body={
            "owner": owner, "name": name, "git": url,
            "visibility": visibility, "description": description,
        })

    def import_status(self, *, repo: str):
        """``{"from", "state": queued|running|done|failed, "detail"?}`` for an imported repo."""
        return self._c.request("GET", self._path(repo, "/import"))

    # branches
    def branches(self, *, repo: str):
        return self._c.request("GET", self._path(repo, "/branches"))

    def delete_branch(self, *, repo: str, branch: str, oid: str):
        """`oid` is the head the caller saw; the delete is refused if the branch moved."""
        return self._c.request("DELETE", self._path(repo, f"/branches/{_seg(branch)}"),
                               query={"oid": oid})

    # protection
    def protection(self, *, repo: str):
        return self._c.request("GET", self._path(repo, "/protection"))

    def protect(self, *, repo: str, pattern: str, remove: bool = False,
                no_force: bool = True, no_delete: bool = True):
        return self._c.request("POST", self._path(repo, "/protection"), body={
            "pattern": pattern, "remove": remove, "no_force": no_force, "no_delete": no_delete,
        })

    # pull requests
    def pulls(self, *, repo: str, state: str | None = None, limit: int | None = None):
        return self._c.request("GET", self._path(repo, "/pulls"),
                               query={"state": state, "limit": limit})

    def pull(self, *, repo: str, number: int):
        return self._c.request("GET", self._path(repo, f"/pulls/{int(number)}"))

    def open_pull(self, *, repo: str, title: str, head: str, base: str,
                  head_repo: str | None = None, description: str = ""):
        """``head_repo`` (another ``"owner/name"``) makes it a cross-repo pull:
        ``head`` then names a branch of that fork, proposed onto this repo's ``base``."""
        if head_repo is not None:
            _split(head_repo)  # same shape rule as every other repo reference
        return self._c.request("POST", self._path(repo, "/pulls"), body={
            "title": title, "head": head, "base": base, "description": description,
            "head_repo": head_repo,
        })

    def comment(self, *, repo: str, number: int, body: str):
        return self._c.request("POST", self._path(repo, f"/pulls/{int(number)}/comments"),
                               body={"body": body})

    def merge(self, *, repo: str, number: int, strategy: str = "fast-forward"):
        """strategy: fast-forward, squash, merge or rebase. Answers 202 — the merge is a job."""
        return self._c.request("POST", self._path(repo, f"/pulls/{int(number)}/merge"),
                               query={"strategy": strategy})

    def close(self, *, repo: str, number: int):
        return self._c.request("POST", self._path(repo, f"/pulls/{int(number)}/close"))

    # commits
    def compare(self, *, repo: str, base: str, head: str):
        return self._c.request("GET", self._path(repo, "/compare"),
                               query={"base": base, "head": head})

    def commit(self, *, repo: str, branch: str, message: str, changes: list,
               expect: str | None = None, new_branch: str | None = None):
        """One commit of file changes; each change is
        ``{"path", "contentBase64"?, "executable"?, "delete"?}``."""
        return self._c.request("POST", self._path(repo, "/commits"), body={
            "branch": branch, "message": message, "changes": changes,
            "expect": expect, "newBranch": new_branch,
        })

    def signature(self, *, repo: str, sha: str):
        return self._c.request("GET", self._path(repo, f"/commits/{_seg(sha)}/signature"))


class Workspaces:
    """Btrfs-backed dev workspaces, over ``/v1/workspaces``. A workspace is named like an
    environment — ``"name"`` for the caller's own, ``"team/name"`` for one in a team — or by its
    id; pushes cut snapshots, kept on the workspace's VOLUME — see :class:`Volumes` for history,
    restore targets and deletion."""

    def __init__(self, client: Kloudlite):
        self._c = client

    def create(self, *, workspace: str, quota_gb: int, image: str | None = None,
               repo: str | None = None, branch: str | None = None,
               packages: list | None = None,
               ttl_secs: int | None = None, env: dict | None = None):
        """``workspace`` is ``"name"`` or ``"team/name"``. ``env`` maps variable names to literal
        values or ``"${{ secrets.NAME }}"`` references to the owner's secrets. ``ttl_secs`` (600..=2592000) deletes it
        after that long unused; absent, never. ``repo`` seeds from a PLATFORM repo
        ("owner/name", with ``branch``); ``packages`` are nixpkgs attributes, optionally pinned
        (``attr@version``)."""
        team, name = _owned(workspace)
        body = {"name": name, "quota_gb": quota_gb,
                "team": team, "repo": repo, "branch": branch,
                "packages": packages or [], "ttl_secs": ttl_secs}
        if image is not None:
            body["image"] = image
        if env is not None:
            body["env"] = env
        return self._c.request("POST", "/v1/workspaces", body=body)

    def list(self, *, team: str | None = None):
        """The caller's workspaces; ``team`` scopes to their workspaces in that team."""
        return self._c.request("GET", "/v1/workspaces", query={"team": team})

    def get(self, *, workspace: str):
        return self._at("GET", workspace)

    def delete(self, *, workspace: str):
        """Runs the worktree finalizer: the volume survives while a pushed snapshot references it."""
        return self._at("DELETE", workspace)

    def start(self, *, workspace: str):
        return self._at("POST", workspace, "/start")

    def stop(self, *, workspace: str):
        """A stop cuts a sync point and tears the workspace down — seconds, never waiting on a replica."""
        return self._at("POST", workspace, "/stop")

    def push(self, *, workspace: str, message: str | None = None):
        """Cut a SNAPSHOT of the working copy — the one durable, restorable state."""
        return self._at("POST", workspace, "/push",
                        body={"message": message} if message is not None else None)

    def clone(self, *, workspace: str, to: str, task: str | None = None):
        """A new workspace ``to`` from this one's state, cut at the moment of the request; the
        answer's ``based_on`` names exactly what it grafted onto."""
        return self._at("POST", workspace, "/clone", body={"name": to, "task": task})

    def restore(self, *, snapshot: str, to: str, image: str | None = None,
                packages: list | None = None, quota_gb: int | None = None):
        """A new workspace ``to`` grafted onto a PUSHED ``snapshot`` — also one whose workspace is
        long gone. Absent overrides mean "whatever the snapshot froze"."""
        return self._c.request("POST", "/v1/workspaces/restore", body={
            "name": to, "snapshot_id": snapshot,
            "image": image, "packages": packages, "quota_gb": quota_gb,
        })

    def set_packages(self, *, workspace: str, packages: list):
        """Replace the package list; pinned entries are locked before anything is written."""
        return self._at("PATCH", workspace, body={"packages": packages})

    def set_ttl(self, *, workspace: str, ttl_secs: int | None):
        """Seconds unused before the workspace is deleted (600..=2592000); ``None`` clears it. Not
        for a sandbox or a bench."""
        return self._at("PUT", workspace, "/ttl", body={"ttl_secs": ttl_secs})

    def set_env(self, *, workspace: str, env: dict):
        """Replace the whole env: literals or ``"${{ secrets.NAME }}"`` references. Takes effect on the
        next start."""
        return self._at("PUT", workspace, "/env", body={"env": env})

    def update_packages(self, *, workspace: str):
        """Re-resolve every pinned package — the only call that moves existing locks."""
        return self._at("POST", workspace, "/packages/update")

    def tools(self, *, workspace: str):
        """The workspace's tool-server address and token — answered to the owner alone."""
        return self._at("GET", workspace, "/tools")

    def ide(self, *, workspace: str) -> "Ide":
        """An :class:`Ide` bound to this workspace's tool server. The address is a private IP:
        reachable from inside the fleet (a bench); from a laptop, tunnel with
        ``kl-connect ws ide`` and use ``Ide(url="http://127.0.0.1:7788", token=…)`` instead."""
        doc = self.tools(workspace=workspace)
        return Ide(url=f"http://{doc['address']}", token=doc.get("token"))

    def _at(self, method: str, workspace: str, tail: str = "", body=None):
        team, name = _owned(workspace)
        return self._c.request(method, f"/v1/workspaces/{_seg(name)}{tail}", body=body,
                               query={"owner": team})


class Ide:
    """A workspace's tool server (``kl ide serve``): the full tool set over plain HTTP —
    ``GET /tools`` lists every tool with its schema, ``POST /tools/{name}`` runs one. The typed
    methods below cover them all; :meth:`call` takes anything the listing names. ``None``
    arguments are dropped, so every optional means "the tool's own default"."""

    def __init__(self, *, url: str, token: str | None = None, timeout: float = 630.0):
        self.url = url.rstrip("/")
        self._token = token
        self._timeout = timeout

    def list(self):
        """Every tool, with its JSON schema — the authority on arguments."""
        return self._http("GET", "/tools")

    def call(self, *, tool: str, **args):
        """Run ``tool``; every other keyword is that tool's own argument."""
        return self._http("POST", f"/tools/{_seg(tool)}", {k: v for k, v in args.items() if v is not None})

    # ── files ─────────────────────────────────────────────────────────────
    def read(self, *, path: str | None = None, paths: list | None = None,
             offset: int | None = None, limit: int | None = None):
        return self.call(tool="read", path=path, paths=paths, offset=offset, limit=limit)

    def write(self, *, path: str, content: str):
        return self.call(tool="write", path=path, content=content)

    def edit(self, *, path: str | None = None, edits: list | None = None, files: list | None = None):
        """One file as ``path=, edits=``, several as ``files=[{path, edits}]``; each edit is
        ``{"old", "new", "replace_all"?}`` and the whole call is atomic."""
        return self.call(tool="edit", path=path, edits=edits, files=files)

    def patch(self, *, diff: str, cwd: str | None = None):
        return self.call(tool="patch", diff=diff, cwd=cwd)

    def glob(self, *, pattern: str, cwd: str | None = None):
        return self.call(tool="glob", pattern=pattern, cwd=cwd)

    def grep(self, *, pattern: str, cwd: str | None = None, glob: str | None = None,
             mode: str | None = None, context: int | None = None,
             ignore_case: bool | None = None, max: int | None = None):
        return self.call(tool="grep", pattern=pattern, cwd=cwd, glob=glob, mode=mode,
                         context=context, ignore_case=ignore_case, max=max)

    # ── commands and processes ────────────────────────────────────────────
    def exec(self, *, cmd, cwd: str | None = None, env: dict | None = None,
             timeout_ms: int | None = None, head: int | None = None, tail: int | None = None,
             quiet: bool | None = None, detach: bool | None = None):
        """``cmd`` is a shell string or an argv list. ``detach=True`` answers ``{id}`` and the
        command becomes a process for the ``process_*`` calls (pass that id as ``process``)."""
        return self.call(tool="exec", cmd=cmd, cwd=cwd, env=env, timeout_ms=timeout_ms,
                         head=head, tail=tail, quiet=quiet, detach=detach)

    def process_list(self):
        return self.call(tool="process_list")

    def process_output(self, *, process: str, since: int | None = None, since_err: int | None = None):
        return self.call(tool="process_output", id=process, since=since, since_err=since_err)

    def process_write(self, *, process: str, data: str):
        return self.call(tool="process_write", id=process, data=data)

    def process_kill(self, *, process: str, signal: str | None = None):
        return self.call(tool="process_kill", id=process, signal=signal)

    # ── watches ───────────────────────────────────────────────────────────
    def watch(self, *, paths: list | None = None, cmd=None, pattern: str | None = None,
              once: bool | None = None):
        return self.call(tool="watch", paths=paths, cmd=cmd, pattern=pattern, once=once)

    def watch_poll(self, *, watch: str, since: int | None = None):
        return self.call(tool="watch_poll", id=watch, since=since)

    def watch_stop(self, *, watch: str):
        return self.call(tool="watch_stop", id=watch)

    # ── containers (the hidden per-owner builder) ─────────────────────────
    def container_build(self, *, tags: list, context: str | None = None, file: str | None = None,
                        build_args: list | None = None, platform: str | None = None,
                        no_cache: bool | None = None, timeout_ms: int | None = None,
                        head: int | None = None, tail: int | None = None):
        return self.call(tool="container_build", tags=tags, context=context, file=file,
                         build_args=build_args, platform=platform, no_cache=no_cache,
                         timeout_ms=timeout_ms, head=head, tail=tail)

    def container_push(self, *, src: str, dst: list):
        return self.call(tool="container_push", src=src, dst=dst)

    def _http(self, method: str, path: str, body=None):
        headers = {"User-Agent": USER_AGENT, "Accept": "application/json"}
        if self._token:
            headers["Authorization"] = f"Bearer {self._token}"
        data = None
        if body is not None:
            data = json.dumps(body).encode()
            headers["Content-Type"] = "application/json"
        req = urllib.request.Request(self.url + path, data=data, headers=headers, method=method)
        try:
            with urllib.request.urlopen(req, timeout=self._timeout) as resp:
                raw = resp.read()
        except urllib.error.HTTPError as e:
            raise ApiError(e.code, _error_text(e.read())) from None
        return json.loads(raw) if raw else None


class Environments:
    """Multi-service environments, over ``/v1/environments``: a set of containers sharing one
    disk. An environment is named by ONE string — ``"name"`` for the caller's own, ``"team/name"``
    for a team's — or by its id (``env-…``). Each service is a dict: ``name`` and ``image``, plus
    optional ``ports``, ``env``, ``command`` and ``mounts`` (``[{"folder", "path"}]``, a folder of
    the environment's disk)."""

    def __init__(self, client: Kloudlite):
        self._c = client

    def create(self, *, environment: str, services: list | None = None,
               quota_gb: int | None = None, snapshot: str | None = None,
               ttl_secs: int | None = None):
        """``snapshot`` grows it from a pushed snapshot — also one of a deleted environment —
        and then ``services`` default to the ones that snapshot froze. ``ttl_secs``
        (600..=2592000) deletes it after that long unused; absent, never."""
        owner, name = _owned(environment)
        return self._c.request("POST", "/v1/environments", body={
            "name": name, "owner": owner, "services": _services(services),
            "quota_gb": quota_gb, "snapshot_id": snapshot, "ttl_secs": ttl_secs,
        })

    def list(self, *, owner: str | None = None):
        """The caller's environments and their teams'; ``owner`` narrows to one handle or team."""
        return self._c.request("GET", "/v1/environments", query={"owner": owner})

    def get(self, *, environment: str):
        return self._at("GET", environment)

    def set_services(self, *, environment: str, services: list):
        """Replace the WHOLE service list. A removed service keeps its folders on the disk, so
        adding it back finds its data."""
        return self._at("PATCH", environment, body={"services": _services(services)})

    def start(self, *, environment: str):
        return self._at("POST", environment, "/start")

    def stop(self, *, environment: str):
        return self._at("POST", environment, "/stop")

    def set_ttl(self, *, environment: str, ttl_secs: int | None):
        """Seconds the environment may sit unused before it is deleted (600..=2592000); None clears."""
        return self._at("PUT", environment, "/ttl", body={"ttl_secs": ttl_secs})

    def delete(self, *, environment: str):
        """The volume survives while a pushed snapshot references it."""
        return self._at("DELETE", environment)

    def push(self, *, environment: str, message: str | None = None):
        """Cut a SNAPSHOT of every service's data — the one durable, restorable state."""
        return self._at("POST", environment, "/push",
                        body={"message": message} if message is not None else None)

    def clone(self, *, environment: str, to: str, snapshot: str | None = None,
              services: list | None = None, quota_gb: int | None = None):
        """A new environment ``to`` (``"name"`` or ``"team/name"``): a copy of the live disk, or
        with ``snapshot`` of one of this environment's snapshots."""
        owner, name = _owned(to)
        return self._at("POST", environment, "/clone", body={
            "name": name, "owner": owner, "snapshot_id": snapshot,
            "services": _services(services) if services else None, "quota_gb": quota_gb,
        })

    def restore(self, *, environment: str, snapshot: str, discard: list | None = None):
        """Put THIS environment back to one of its own snapshots, services and data (202). A
        persistent service the snapshot lacks is a 409 naming it; list it in ``discard`` to
        accept losing its data."""
        return self._at("POST", environment, "/restore",
                        body={"snapshot_id": snapshot, "discard": discard or []})

    def _at(self, method: str, environment: str, tail: str = "", body=None):
        owner, name = _owned(environment)
        return self._c.request(method, f"/v1/environments/{_seg(name)}{tail}", body=body,
                               query={"owner": owner})


def _owned(s: str) -> tuple[str | None, str]:
    """``"name"`` → (None, name); ``"team/name"`` → (team, name)."""
    owner, sep, name = s.rpartition("/")
    if sep and (not owner or not name or "/" in owner):
        raise ValueError(f"expected name or team/name, got {s!r}")
    return (owner or None), name


def _services(services: list | None) -> list:
    """The server requires every field; fill the ones a caller leaves out."""
    return [{"command": [], "env": {}, "mounts": [], "ports": [], **sv} for sv in services or []]


class Sessions:
    """Working sessions, over ``/v1/sessions``: some of YOUR workspaces in one team, connected to
    one environment. Many people's sessions connect to the same environment and collaborate
    through it; an intercept a session takes is environment-wide, and the session owns it — only
    it can release it, and removing the workspace or deleting the session releases it.

    A session is named like an environment — ``"name"`` (personal) or ``"team/name"`` — or by its
    id (``ses-…``); a workspace by its id, name or ``"team/name"``, sent verbatim."""

    def __init__(self, client: Kloudlite):
        self._c = client

    def create(self, *, session: str, environment: str | None = None, ttl_secs: int | None = None):
        """``environment``, when given, must be the session's team's: an id, a name or
        ``"team/name"``; without one the session connects to nothing until ``connect``.
        ``ttl_secs`` (600..=2592000) deletes the session after that long unused."""
        team, name = _owned(session)
        return self._c.request("POST", "/v1/sessions",
                               body={"name": name, "team": team, "environment": environment,
                                     "ttl_secs": ttl_secs})

    def set_ttl(self, *, session: str, ttl_secs: int | None):
        """Seconds unused before the session is deleted (600..=2592000); ``None`` clears it."""
        return self._at("PUT", session, "/ttl", body={"ttl_secs": ttl_secs})

    def list(self, *, team: str | None = None):
        return self._c.request("GET", "/v1/sessions", query={"team": team})

    def get(self, *, session: str):
        """The session with its ``workspaces`` and the ``intercepts`` it holds."""
        return self._at("GET", session)

    def delete(self, *, session: str):
        """Releases its intercepts and lets its workspaces go — never deletes a workspace."""
        return self._at("DELETE", session)

    def connect(self, *, session: str, environment: str | None):
        """Sets, switches or (``None``) clears the session's environment. Its intercepts in the old
        environment are released first; deleting an environment disconnects its sessions the same
        way."""
        return self._at("PUT", session, "/environment", body={"environment": environment})

    def add(self, *, session: str, workspace: str):
        """A workspace is in at most one session; while in one it resolves that session's
        environment by bare service name instead of its team's choice."""
        return self._at("POST", session, "/workspaces", body={"workspace": workspace})

    def remove(self, *, session: str, workspace: str):
        """Releases whatever the workspace intercepts in this session first."""
        return self._at("DELETE", session, f"/workspaces/{_seg(workspace)}")

    def intercept(self, *, session: str, service: str, workspace: str, ports: dict | None = None):
        """Send everything that calls ``service`` in the environment to ``workspace`` (a member),
        ``ports`` mapping each service port to the workspace port that answers it
        (``{8080: 3000}``); an unmapped port answers on the same number."""
        return self._at("POST", session, "/intercepts", body={
            "service": service, "workspace": workspace,
            "ports": [{"service": int(k), "workspace": int(v)} for k, v in (ports or {}).items()],
        })

    def release(self, *, session: str, service: str):
        return self._at("DELETE", session, f"/intercepts/{_seg(service)}")

    def _at(self, method: str, session: str, tail: str = "", body=None):
        team, name = _owned(session)
        return self._c.request(method, f"/v1/sessions/{_seg(name)}{tail}", body=body,
                               query={"owner": team})


class Secrets:
    """An owner's named secrets, over ``/v1/secrets``. A value goes in and never comes back:
    ``list`` returns names with who and when. Reference one from a workspace env as
    ``"${{ secrets.NAME }}"``."""

    def __init__(self, client: Kloudlite):
        self._c = client

    def set(self, *, owner: str, name: str, value: str) -> None:
        self._c.request("PUT", f"/v1/secrets/{_seg(owner)}/{_seg(name)}", body={"value": value})

    def list(self, *, owner: str):
        return self._c.request("GET", f"/v1/secrets/{_seg(owner)}")

    def delete(self, *, owner: str, name: str) -> None:
        """409 while a workspace or environment still references it; the body names them."""
        self._c.request("DELETE", f"/v1/secrets/{_seg(owner)}/{_seg(name)}")


class Volumes:
    """The btrfs volumes behind workspaces and environments: snapshot history, restore targets,
    and the two deletes — over ``/v1/volumes``."""

    def __init__(self, client: Kloudlite):
        self._c = client

    def list(self, *, kind: str | None = None, owner: str | None = None):
        """``kind`` is ``workspace`` or ``environment``; ``owner`` a handle or team slug."""
        return self._c.request("GET", "/v1/volumes", query={"kind": kind, "owner": owner})

    def history(self, *, volume: str):
        """The chain of PUSHED snapshots, newest first — each row a restore target."""
        return self._c.request("GET", f"/v1/volumes/{_seg(volume)}/history")

    def refs(self, *, volume: str):
        return self._c.request("GET", f"/v1/volumes/{_seg(volume)}/refs")

    def delete_snapshot(self, *, volume: str, snapshot: str):
        """Refused (409) for a sync point or a running worktree's base; deleting a detached
        volume's last snapshot deletes the volume."""
        return self._c.request("DELETE", f"/v1/volumes/{_seg(volume)}/snapshots/{_seg(snapshot)}")

    def delete(self, *, volume: str):
        """Takes a DETACHED volume with all its snapshots; refused while a working copy remains."""
        return self._c.request("DELETE", f"/v1/volumes/{_seg(volume)}")


class SandboxTerminated(ApiError):
    """A call needed the sandbox ``running`` and it is ``terminated``. ``reason`` is the server's
    (``Timeout``, ``Idle``, ``Exited``, ``Failed``, ``Unplaceable``, ``SourceGone``, ``Lost``,
    ``EnvironmentGone`` or ``Terminated``); ``status`` is 409, as the tool route answers a sandbox that is not running."""

    def __init__(self, sandbox: str, reason: str | None, message: str | None = None):
        text = f"sandbox {sandbox} terminated: {reason or 'unknown reason'}"
        super().__init__(409, f"{text} ({message})" if message else text)
        self.sandbox = sandbox
        self.reason = reason


class _aio:
    """``obj.call(...)`` blocks; ``await obj.call.aio(...)`` runs the very same call on a worker
    thread (:func:`asyncio.to_thread`) — one implementation per call, so the async spelling can
    never drift from the blocking one."""

    def __init__(self, fn):
        self._fn = fn
        functools.update_wrapper(self, fn)

    def __call__(self, *args, **kwargs):  # Class.method(obj, ...) still works
        return self._fn(*args, **kwargs)

    def __get__(self, obj, owner=None):
        if obj is None:
            return self
        fn = self._fn

        @functools.wraps(fn)
        def call(*args, **kwargs):
            return fn(obj, *args, **kwargs)

        async def aio(*args, **kwargs):
            return await asyncio.to_thread(fn, obj, *args, **kwargs)

        call.aio = aio
        return call


def _backoff(timeout_s: float | None, what: str):
    """Sleep 0.5 s growing to 2 s between polls; TimeoutError naming ``what`` past ``timeout_s``."""
    deadline = None if timeout_s is None else time.monotonic() + timeout_s
    delay = 0.5
    while True:
        yield
        if deadline is not None and time.monotonic() + delay > deadline:
            raise TimeoutError(f"{what} after {timeout_s} s")
        time.sleep(delay)
        delay = min(delay * 1.5, 2.0)


class Sandboxes:
    """Throwaway, isolated machines that run code, over ``/v1/sandboxes``. A sandbox starts from an
    image, from one of your workspaces (``from_workspace``) or from a pushed snapshot
    (``from_snapshot``), lives at most ``timeout_s`` (server default 300) and is gone once
    terminated — read anything you need out of it first. Every call answers a :class:`Sandbox`
    handle, and every call here and on the handles also has an async twin, ``.aio``::

        sb = kl.sandboxes.create(image="python:3.12")
        p = sb.exec(cmd=["python", "-c", "print(input())"])
        p.stdin.write(b"hi\\n"); p.stdin.write_eof()
        assert p.wait() == 0 and p.stdout.read() == "hi\\n"
        sb.terminate()
    """

    def __init__(self, client: Kloudlite):
        self._c = client

    @_aio
    def create(self, *, image: str | None = None, packages: list | None = None,
               from_workspace: str | None = None, from_snapshot: str | None = None,
               team: str | None = None,
               timeout_s: int | None = None, idle_timeout_s: int | None = None,
               network: str | None = None, env: dict | None = None, cpu: int | None = None,
               memory_gb: int | None = None, disk_gb: int | None = None,
               session: str | None = None) -> "Sandbox":
        """Ask for a sandbox and return at once (202) — it is usually still ``starting``;
        :meth:`Sandbox.exec` waits for it. ``from_workspace`` is an id, ``"name"`` or
        ``"team/name"``; ``network`` is ``internet`` or ``none``; ``cpu`` in cores.
        ``session`` (an id, ``"name"`` or ``"team/name"`` of your session in the sandbox's team)
        makes it a member from birth — the only way it reaches an environment, whose services
        then resolve by bare name inside it. Unset arguments are
        left to the server's defaults and the owner's quota."""
        body = {"image": image, "packages": packages, "from_workspace": from_workspace,
                "from_snapshot": from_snapshot, "team": team,
                "timeout_secs": timeout_s, "idle_timeout_secs": idle_timeout_s,
                "network": network, "env": env, "cpu": cpu, "memory_gb": memory_gb,
                "disk_gb": disk_gb, "session": session}
        doc = self._c.request("POST", "/v1/sandboxes",
                              body={k: v for k, v in body.items() if v is not None})
        return Sandbox(self._c, doc)

    @_aio
    def get(self, *, sandbox: str) -> "Sandbox":
        return Sandbox(self._c, self._c.request("GET", f"/v1/sandboxes/{_seg(sandbox)}"))

    @_aio
    def list(self, *, team: str | None = None) -> list:
        """The caller's sandboxes, newest first; ``team`` scopes to theirs in that team."""
        docs = self._c.request("GET", "/v1/sandboxes", query={"team": team}) or []
        return [Sandbox(self._c, d) for d in docs]

    @_aio
    def run(self, *, cmd, cwd: str | None = None, env: dict | None = None,
            timeout_s: float | None = None, image: str | None = None,
            packages: list | None = None, from_workspace: str | None = None,
            from_snapshot: str | None = None, team: str | None = None,
            idle_timeout_s: int | None = None,
            network: str | None = None, cpu: int | None = None, memory_gb: int | None = None,
            disk_gb: int | None = None, session: str | None = None) -> dict:
        """One command in a fresh sandbox: create, exec, wait, read every byte of output, and
        terminate — ALWAYS, also when anything before it raised. Answers
        ``{"exit_code", "stdout", "stderr", "sandbox"}``. ``env`` and ``cwd`` are the command's;
        ``timeout_s`` bounds the command (None waits until it exits) and, when given, also sizes
        the sandbox's own lifetime to cover it plus a minute to start."""
        lifetime = None if timeout_s is None else min(86_400, max(10, math.ceil(timeout_s) + 60))
        sb = self.create(image=image, packages=packages, from_workspace=from_workspace,
                         from_snapshot=from_snapshot, team=team,
                         timeout_s=lifetime, idle_timeout_s=idle_timeout_s, network=network,
                         cpu=cpu, memory_gb=memory_gb, disk_gb=disk_gb,
                         session=session)
        try:
            p = sb.exec(cmd=cmd, cwd=cwd, env=env)
            code = p.wait(timeout_s=timeout_s)
            out, err = p._collect()
        except BaseException:
            with contextlib.suppress(Exception):  # never mask the failure that got us here
                sb.terminate()
            raise
        sb.terminate()
        return {"exit_code": code, "stdout": out, "stderr": err, "sandbox": sb.id}


class Sandbox:
    """A handle on one sandbox: ``id``, ``phase`` (``pending``, ``starting``, ``running``,
    ``terminated``), ``reason`` once terminated, and the whole server ``doc`` as of the last
    answer — :meth:`refresh` re-reads it."""

    def __init__(self, client: Kloudlite, doc: dict):
        self._c = client
        self.doc = doc

    id = property(lambda self: self.doc["id"])
    phase = property(lambda self: self.doc.get("phase"))
    reason = property(lambda self: self.doc.get("reason"))

    def __repr__(self):
        return f"<Sandbox {self.id} {self.phase}{' ' + self.reason if self.reason else ''}>"

    @_aio
    def refresh(self) -> "Sandbox":
        self.doc = self._c.request("GET", self._path())
        return self

    @_aio
    def wait_running(self, *, timeout_s: float = 120) -> "Sandbox":
        """Poll until ``running``; :class:`SandboxTerminated` naming the reason if it terminates
        instead (an image that does not pull, a quota, a node that cannot place it)."""
        for _ in _backoff(timeout_s, f"sandbox {self.id} not running"):
            self.refresh()
            if self.phase == "running":
                return self
            self._raise_if_terminated()

    @_aio
    def exec(self, *, cmd, cwd: str | None = None, env: dict | None = None) -> "Process":
        """Start ``cmd`` — an argv list, or a string run by the shell — and return its
        :class:`Process` at once; waits for the sandbox to run first. Always detached: the
        process is followed through its stdin/stdout/stderr and :meth:`Process.wait`."""
        if self.phase != "running":
            self.wait_running()
        body = {"cmd": cmd, "detach": True}
        if cwd is not None:
            body["cwd"] = cwd
        if env is not None:
            body["env"] = env
        return Process(self, str(self._tool("exec", body)["id"]))

    @_aio
    def read(self, *, path: str):
        """The tool server's ``read``; ``path`` is relative to ``~/workspace``."""
        return self._tool("read", {"path": path})

    @_aio
    def write(self, *, path: str, content: str):
        return self._tool("write", {"path": path, "content": content})

    @_aio
    def terminate(self) -> "Sandbox":
        """Stop it for good (202); the record stays until :meth:`delete`."""
        self.doc = self._c.request("POST", self._path("/terminate")) or self.doc
        return self

    @_aio
    def wait(self, *, timeout_s: float | None = None) -> str | None:
        """Poll until ``terminated`` and answer the reason."""
        for _ in _backoff(timeout_s, f"sandbox {self.id} still {self.phase}"):
            self.refresh()
            if self.phase == "terminated":
                return self.reason

    @_aio
    def delete(self):
        return self._c.request("DELETE", self._path())

    def _path(self, tail: str = "") -> str:
        return f"/v1/sandboxes/{_seg(self.id)}{tail}"

    def _raise_if_terminated(self):
        if self.phase == "terminated":
            raise SandboxTerminated(self.id, self.reason, self.doc.get("message"))

    def _tool(self, name: str, body: dict):
        try:
            return self._c.request("POST", self._path(f"/tools/{_seg(name)}"), body=body)
        except ApiError as e:
            if e.status != 409:
                raise
            # 409 is the api saying "not running": name WHY rather than the bare refusal.
            with contextlib.suppress(ApiError):
                self.refresh()
            self._raise_if_terminated()
            raise


class Process:
    """A detached process in a sandbox: ``id``, ``sandbox``, ``stdin`` to write to, ``stdout`` and
    ``stderr`` to read or iterate, and ``returncode`` once it is known."""

    def __init__(self, sandbox: Sandbox, pid: str):
        self.sandbox = sandbox
        self.id = pid
        self._returncode = None
        self.stdin = ProcessStdin(self)
        self.stdout = ProcessOutput(self, "stdout")
        self.stderr = ProcessOutput(self, "stderr")

    def __repr__(self):
        return f"<Process {self.id} in {self.sandbox.id} returncode={self._returncode}>"

    @property
    def returncode(self) -> int | None:
        """The exit code as of the last :meth:`wait`, :meth:`poll` or finished read; None
        before then (or for a process a signal ended)."""
        return self._returncode

    @_aio
    def wait(self, *, timeout_s: float | None = None) -> int | None:
        """Block until it exits and answer the exit code; TimeoutError past ``timeout_s``. Waits
        in polls of at most 20 s, the most the api holds one open."""
        deadline = None if timeout_s is None else time.monotonic() + timeout_s
        while True:
            left_ms = 20_000 if deadline is None else int((deadline - time.monotonic()) * 1000)
            if left_ms <= 0:
                raise TimeoutError(f"process {self.id} still running after {timeout_s} s")
            code = self._wait_ms(min(left_ms, 20_000))
            if code is not _RUNNING:
                return code

    @_aio
    def poll(self) -> int | None:
        """The exit code if it has exited, else None — never blocks."""
        code = self._wait_ms(0)
        return None if code is _RUNNING else code

    @_aio
    def kill(self, *, signal: str | None = None):
        """Signal the whole process group (the tool server's default is a kill)."""
        body = {"id": self.id}
        if signal is not None:
            body["signal"] = signal
        return self.sandbox._tool("process_kill", body)

    def _wait_ms(self, timeout_ms: int):
        out = self.sandbox._tool("process_wait", {"id": self.id, "timeout_ms": timeout_ms})
        if out.get("state") != "exited":
            return _RUNNING
        self._returncode = out.get("exit_code")
        return self._returncode

    def _frames(self):
        """Every frame of the output stream, from the first byte the ring still holds to the
        exit frame — each read or iteration is its own replay, so they never steal output."""
        path = self.sandbox._path(f"/processes/{_seg(self.id)}/stream")
        for frame in self.sandbox._c._ndjson(path, {"since": 0, "since_err": 0}):
            if "exit" in frame:
                self._returncode = frame["exit"]
            yield frame

    def _collect(self) -> tuple[str, str]:
        out, err = [], []
        for frame in self._frames():
            if frame.get("stream") == "stdout":
                out.append(frame.get("data", ""))
            elif frame.get("stream") == "stderr":
                err.append(frame.get("data", ""))
        return "".join(out), "".join(err)


_RUNNING = object()


class ProcessStdin:
    """A process's stdin. Each write is delivered when the call returns, so :meth:`drain` has
    nothing to wait for; :meth:`write_eof` closes it."""

    def __init__(self, process: Process):
        self._p = process

    @_aio
    def write(self, data: str | bytes) -> int:
        """Send ``data`` (bytes, or text as UTF-8) and answer how many bytes went. The one call
        that also takes its input by position — ``stdin.write(b"…")`` — because it is a file's
        ``write`` and has exactly one input to get in order."""
        raw = data.encode() if isinstance(data, str) else bytes(data)
        self._p.sandbox._tool("process_write", {
            "id": self._p.id, "data_base64": base64.b64encode(raw).decode("ascii"),
        })
        return len(raw)

    @_aio
    def write_eof(self):
        """Close stdin — what ends a ``while read line`` loop."""
        return self._p.sandbox._tool("process_close_stdin", {"id": self._p.id})

    @_aio
    def drain(self):
        """Nothing to flush: kept so code written against a stream writer reads the same."""
        return None


class ProcessOutput:
    """A process's stdout or stderr. :meth:`read` answers the whole stream once the process has
    exited; iterating yields lines as they arrive — newline kept, as a file does, and a last
    unterminated line at the end — and :meth:`aio_lines` is the same iteration for ``async for``."""

    def __init__(self, process: Process, name: str):
        self._p = process
        self._name = name

    @_aio
    def read(self) -> str:
        return "".join(self._chunks())

    def __iter__(self):
        pending = ""
        for chunk in self._chunks():
            pending += chunk
            *lines, pending = pending.split("\n")
            for line in lines:
                yield line + "\n"
        if pending:
            yield pending

    async def aio_lines(self):
        """``async for line in p.stdout.aio_lines()``: the blocking stream runs on its own
        daemon thread and hands lines over a queue, so a consumer that stops early leaves
        nothing for the event loop to wait on at shutdown."""
        loop = asyncio.get_running_loop()
        queue: asyncio.Queue = asyncio.Queue()
        stop = threading.Event()
        end = object()

        def hand(item):
            try:
                loop.call_soon_threadsafe(queue.put_nowait, item)
            except RuntimeError:  # the loop closed under a consumer that went away
                stop.set()

        def pump():
            try:
                for line in self:
                    if stop.is_set():
                        return
                    hand((line, None))
                hand((end, None))
            except BaseException as e:  # surfaced in the consumer, not lost on the thread
                hand((end, e))

        threading.Thread(target=pump, daemon=True).start()
        try:
            while True:
                line, err = await queue.get()
                if line is end:
                    if err is not None:
                        raise err
                    return
                yield line
        finally:
            stop.set()

    def _chunks(self):
        for frame in self._p._frames():
            if frame.get("stream") == self._name:
                yield frame.get("data", "")


_RUN_TERMINAL = ("success", "failure", "cancelled")


class Pipelines:
    """CI pipelines, over ``/v1/pipelines``: definitions live as YAML in an owner's
    ``{owner}/pipelines`` repository and run against that owner's repositories, each job in its
    own sandbox. A run is recorded on the repository it is about, so it is read back by that
    ``repo`` (``"owner/name"``). :meth:`run`, :meth:`runs` and :meth:`get_run` answer :class:`Run`
    handles, and every call here and on a handle also has an async twin, ``.aio``::

        run = kl.pipelines.run(owner="acme", pipeline="ci", repo="acme/payments")
        for chunk in run.follow(job="test"):
            print(chunk, end="")
        assert run.wait() == "success"
    """

    def __init__(self, client: Kloudlite):
        self._c = client

    @_aio
    def list(self, *, owner: str | None = None) -> dict:
        """The definitions on ``{owner}/pipelines``'s main branch, broken ones included:
        ``{"repo", "sha", "pipelines": [{"name", "file", "valid", "errors", "repos", "on",
        "jobs"}], "errors"}``. ``owner`` (a handle or team slug) defaults to the caller."""
        return self._c.request("GET", "/v1/pipelines", query={"owner": owner})

    @_aio
    def run(self, *, owner: str, pipeline: str, repo: str, ref: str | None = None,
            inputs: dict | None = None, environment: str | None = None,
            clone: str | None = None) -> "Run":
        """Start ``owner``'s ``pipeline`` by hand on ``repo`` (a name, or ``"owner/name"`` — one of
        ``owner``'s) at ``ref`` (a branch, tag, full ref or sha; default ``main``) and answer the
        :class:`Run` at once (202), ``queued``. ``inputs`` are the ones the pipeline's
        ``on.manual.inputs`` declares. ``environment`` runs every job that names none in that
        environment; ``clone`` instead in a per-run copy of it, deleted when the run ends — one or
        the other. A definition that cannot run is an :class:`ApiError` 422 whose message lists
        every reason, and nothing is recorded."""
        body = {"repo": repo, "ref": ref, "inputs": inputs,
                "environment": _run_environment(environment, clone)}
        doc = self._c.request("POST", f"/v1/pipelines/{_seg(owner)}/{_seg(pipeline)}/runs",
                              body={k: v for k, v in body.items() if v is not None})
        return Run(self._c, doc)

    @_aio
    def runs(self, *, repo: str, pipeline: str | None = None, limit: int | None = None) -> list:
        """``repo``'s runs, newest first; ``pipeline`` keeps one pipeline's."""
        docs = self._c.request("GET", _repo_runs(repo), query={"pipeline": pipeline, "limit": limit})
        return [Run(self._c, d) for d in docs or []]

    @_aio
    def get_run(self, *, repo: str, run: str) -> "Run":
        """One run of ``repo`` by its id (``1760400000000-0a1b2c``)."""
        return Run(self._c, self._c.request("GET", _repo_runs(repo, f"/{_seg(run)}")))


def _repo_runs(repo: str, tail: str = "") -> str:
    owner, name = _split(repo)
    return f"/v1/repos/{_seg(owner)}/{_seg(name)}/runs{tail}"


def _run_environment(environment: str | None, clone: str | None):
    """The wire's one ``environment`` field: a name, or ``{"clone": name}``."""
    if environment is not None and clone is not None:
        raise ValueError("pass environment or clone, not both")
    return {"clone": clone} if clone is not None else environment


class Run:
    """A handle on one pipeline run: its fields as of the last answer (``id``, ``number``,
    ``state`` — ``queued``, ``running``, ``success``, ``failure`` or ``cancelled`` — ``jobs`` and
    the rest; ``doc`` is the whole server answer, camelCase as the wire spells it) and the calls
    on it. :meth:`refresh` re-reads it. Each job in ``jobs`` is a dict with ``key``, ``job``,
    ``matrix``, ``needs``, ``state`` (``pending``, ``running``, ``success``, ``failure``,
    ``skipped``, ``cancelled``), ``sandbox``, ``startedAtMs``, ``finishedAtMs``, ``exitCode``,
    ``failedStep`` and ``detail``; :meth:`logs` and :meth:`follow` name one by its index or its
    ``key``."""

    def __init__(self, client: Kloudlite, doc: dict):
        self._c = client
        self.doc = doc

    id = property(lambda self: self.doc["id"])
    number = property(lambda self: self.doc.get("number"))
    pipeline = property(lambda self: self.doc.get("pipeline"))
    pipelines_repo = property(lambda self: self.doc.get("pipelinesRepo"))
    pipelines_sha = property(lambda self: self.doc.get("pipelinesSha"))
    repo = property(lambda self: self.doc.get("repo"))
    ref = property(lambda self: self.doc.get("ref"))
    sha = property(lambda self: self.doc.get("sha"))
    event = property(lambda self: self.doc.get("event"))
    inputs = property(lambda self: self.doc.get("inputs") or {})
    environment = property(lambda self: self.doc.get("environment"))
    requested_by = property(lambda self: self.doc.get("requestedBy"))
    requested_at_ms = property(lambda self: self.doc.get("requestedAtMs"))
    state = property(lambda self: self.doc.get("state"))
    cancel_requested = property(lambda self: bool(self.doc.get("cancelRequested")))
    started_at_ms = property(lambda self: self.doc.get("startedAtMs"))
    finished_at_ms = property(lambda self: self.doc.get("finishedAtMs"))
    detail = property(lambda self: self.doc.get("detail"))
    jobs = property(lambda self: self.doc.get("jobs") or [])

    def __repr__(self):
        return f"<Run {self.repo}#{self.number} {self.pipeline} {self.state}>"

    @_aio
    def refresh(self) -> "Run":
        self.doc = self._c.request("GET", self._path())
        return self

    @_aio
    def cancel(self) -> "Run":
        """Ask it to stop: a queued run is cancelled at once, a running one once its worker has
        ended its sandboxes — :meth:`wait` sees it reach ``cancelled``."""
        self.doc = self._c.request("POST", self._path("/cancel")) or self.doc
        return self

    @_aio
    def wait(self, *, timeout_s: float | None = None, poll_s: float = 5) -> str:
        """Poll every ``poll_s`` until the run has ended and answer its final ``state``;
        TimeoutError past ``timeout_s`` (None waits as long as it takes)."""
        deadline = None if timeout_s is None else time.monotonic() + timeout_s
        while self.state not in _RUN_TERMINAL:
            self.refresh()
            if self.state in _RUN_TERMINAL:
                break
            left = None if deadline is None else deadline - time.monotonic()
            if left is not None and left <= 0:
                raise TimeoutError(f"run {self.id} still {self.state} after {timeout_s} s")
            time.sleep(poll_s if left is None else min(poll_s, left))
        return self.state

    @_aio
    def logs(self, *, job: int | str) -> str:
        """The job's whole log so far (combined stdout and stderr), every page of it. ``job`` is
        its index in ``jobs`` or its ``key`` (``"test (postgres16)"``)."""
        index, offset, out = self._job_index(job), 0, []
        while True:
            page = self._page(index, offset)
            out.append(page.get("data") or "")
            nxt = int(page.get("next") or offset)
            if page.get("complete") or nxt <= offset:
                return "".join(out)
            offset = nxt

    def follow(self, *, job: int | str, poll_s: float = 5):
        """Yield the job's log in chunks as it is written — from the first byte, polling every
        ``poll_s`` while nothing new has landed — and stop once the job has ended and every byte
        was yielded. :meth:`aio_follow` is the same for ``async for``."""
        index, offset = self._job_index(job), 0
        while True:
            page = self._page(index, offset)
            if page.get("data"):
                yield page["data"]
            if page.get("complete"):
                return
            nxt = int(page.get("next") or offset)
            if nxt <= offset:
                time.sleep(poll_s)
            offset = max(offset, nxt)

    async def aio_follow(self, *, job: int | str, poll_s: float = 5):
        """``async for chunk in run.aio_follow(job=…)``: :meth:`follow` with each page read on a
        worker thread and the waits on the event loop."""
        index, offset = self._job_index(job), 0
        while True:
            page = await asyncio.to_thread(self._page, index, offset)
            if page.get("data"):
                yield page["data"]
            if page.get("complete"):
                return
            nxt = int(page.get("next") or offset)
            if nxt <= offset:
                await asyncio.sleep(poll_s)
            offset = max(offset, nxt)

    def _path(self, tail: str = "") -> str:
        return _repo_runs(self.repo, f"/{_seg(self.id)}{tail}")

    def _page(self, index: int, offset: int) -> dict:
        """``{data, next, complete, state}``: at most a page of the log from byte ``offset``."""
        return self._c.request("GET", self._path(f"/jobs/{index}/logs"), query={"offset": offset})

    def _job_index(self, job: int | str) -> int:
        keys = [j.get("key") for j in self.jobs]
        if isinstance(job, int) and not isinstance(job, bool):
            if not 0 <= job < len(keys):
                raise ValueError(f"run {self.id} has {len(keys)} jobs, no job {job}")
            return job
        if job in keys:
            return keys.index(job)
        raise ValueError(f"run {self.id} has no job {job!r}; its jobs are {keys}")


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

    def create(self, *, repo: str, private: bool = True):
        """Sent explicitly rather than trusting a server default: a model that was meant to be
        private must not hinge on one."""
        return self._c.hub_request("POST", "/api/repos/create",
                                   body=self._body(repo, {"private": private}))

    def get(self, *, repo: str):
        return self._c.hub_request("GET", self._path(repo))

    def refs(self, *, repo: str):
        """Branches and tags, as the hub spells them."""
        return self._c.hub_request("GET", self._path(repo, "/refs"))

    def set_private(self, *, repo: str, private: bool):
        return self._c.hub_request("PUT", self._path(repo, "/settings"), body={"private": private})

    def delete(self, *, repo: str):
        return self._c.hub_request("DELETE", "/api/repos/delete", body=self._body(repo))

    def import_git(self, *, repo: str, url: str, private: bool = True):
        """A new repo whose contents — large files included, through git-lfs — are fetched from a
        PUBLIC git remote; ``url`` is a strict ``https://`` URL (the Hugging Face Hub is one host
        among any). Answers 202; poll :meth:`import_status`."""
        return self._c.hub_request("POST", self._path(repo, "/import"),
                                   body={"from": url, "private": private})

    def import_status(self, *, repo: str):
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
