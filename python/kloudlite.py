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
               packages: list | None = None, region: str | None = None):
        """``workspace`` is ``"name"`` or ``"team/name"``. ``repo`` seeds from a PLATFORM repo
        ("owner/name", with ``branch``); ``packages`` are nixpkgs attributes, optionally pinned
        (``attr@version``). ``region`` matters only on a multi-region fleet — absent, the
        deployment's one region is used."""
        team, name = _owned(workspace)
        body = {"name": name, "region": region, "quota_gb": quota_gb,
                "team": team, "repo": repo, "branch": branch,
                "packages": packages or []}
        if image is not None:
            body["image"] = image
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
        """A stop cuts a sync point and tears the pod down — seconds, never waiting on a replica."""
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

    def update_packages(self, *, workspace: str):
        """Re-resolve every pinned package — the only call that moves existing locks."""
        return self._at("POST", workspace, "/packages/update")

    def tools(self, *, workspace: str):
        """The workspace's tool-server address and token — answered to the owner alone."""
        return self._at("GET", workspace, "/tools")

    def ide(self, *, workspace: str) -> "Ide":
        """An :class:`Ide` bound to this workspace's tool server. The address is a pod IP:
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
               quota_gb: int | None = None, snapshot: str | None = None):
        """``snapshot`` grows it from a pushed snapshot — also one of a deleted environment —
        and then ``services`` default to the ones that snapshot froze."""
        owner, name = _owned(environment)
        return self._c.request("POST", "/v1/environments", body={
            "name": name, "owner": owner, "services": _services(services),
            "quota_gb": quota_gb, "snapshot_id": snapshot,
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

    def hold(self, *, environment: str):
        """Reset the idle clock — call it periodically while something depends on the services."""
        return self._at("POST", environment, "/hold")

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

    def create(self, *, session: str, environment: str):
        """``environment`` must be the session's team's: an id, a name or ``"team/name"``."""
        team, name = _owned(session)
        return self._c.request("POST", "/v1/sessions",
                               body={"name": name, "team": team, "environment": environment})

    def list(self, *, team: str | None = None):
        return self._c.request("GET", "/v1/sessions", query={"team": team})

    def get(self, *, session: str):
        """The session with its ``workspaces`` and the ``intercepts`` it holds."""
        return self._at("GET", session)

    def delete(self, *, session: str):
        """Releases its intercepts and lets its workspaces go — never deletes a workspace."""
        return self._at("DELETE", session)

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
