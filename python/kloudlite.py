"""Kloudlite SDK for Python — stdlib only. Needs Python 3.9 or later.

Auth is a Bearer token (a CLI token). Every module hangs off the one client::

    from kloudlite import Kloudlite

    kl = Kloudlite()   # the login of `kloudlite`; see "Discovery" below
    workspaces = kl.workspaces.list()

Discovery: the API origin and the token each come from the first source that has one:
1. the arguments ``base_url=`` / ``token=``; 2. the environment ``KLOUDLITE_URL`` /
``KLOUDLITE_TOKEN``; 3. ``api`` / ``token`` in ``config.json`` in ``KLOUDLITE_CONFIG_DIR``, else
``~/.config/kloudlite``. ``/v1`` is added to the origin. With no token the constructor raises
``RuntimeError`` carrying the sentence ``no Kloudlite login: pass token=, set KLOUDLITE_TOKEN, or run kloudlite login``.

Every public call takes its inputs BY NAME (keyword-only), so no caller ever has to remember
argument order.
"""

from __future__ import annotations

import sys

if sys.version_info < (3, 9):  # asyncio.to_thread; annotations stay lazy, so this runs first
    raise ImportError("kloudlite.py needs Python 3.9 or later, found %d.%d" % sys.version_info[:2])

import asyncio
import contextlib
import functools
import json
import math
import os
import time
import urllib.error
import urllib.parse
import urllib.request

__all__ = ["Kloudlite", "ApiError", "SandboxTerminated"]

VERSION = "0.2.1"
USER_AGENT = f"kloudlite-sdk-python/{VERSION}"
NO_LOGIN = "no Kloudlite login: pass token=, set KLOUDLITE_TOKEN, or run kloudlite login"


class ApiError(Exception):
    """A non-2xx answer. ``status`` is the HTTP code, ``message`` the server's text."""

    def __init__(self, status: int, message: str):
        super().__init__(f"{status}: {message}")
        self.status = status
        self.message = message


def _login() -> dict:
    """``config.json`` of the ``kloudlite`` login; {} when it is absent or unreadable."""
    d = os.environ.get("KLOUDLITE_CONFIG_DIR") or os.path.join(os.path.expanduser("~"), ".config", "kloudlite")
    try:
        with open(os.path.join(d, "config.json")) as f:
            doc = json.load(f)
        return doc if isinstance(doc, dict) else {}
    except (OSError, ValueError):
        return {}


class Kloudlite:
    def __init__(self, *, base_url: str | None = None, token: str | None = None,
                 timeout: float = 30.0):
        env, cfg = os.environ, None
        base_url = base_url or env.get("KLOUDLITE_URL")
        token = token or env.get("KLOUDLITE_TOKEN")
        if not base_url or not token:
            cfg = _login()
            base_url = base_url or cfg.get("api")
            token = token or cfg.get("token")
        if not token:
            raise RuntimeError(NO_LOGIN)
        if not base_url:
            raise RuntimeError("no Kloudlite API URL: pass base_url=, set KLOUDLITE_URL, or run kloudlite login")
        base_url = base_url.rstrip("/")
        self.base_url = base_url[:-3] if base_url.endswith("/v1") else base_url  # paths carry /v1
        self._token = token
        self._timeout = timeout
        self.workspaces = Workspaces(self)
        self.environments = Environments(self)
        self.sessions = Sessions(self)
        self.volumes = Volumes(self)
        self.secrets = Secrets(self)
        self.connectors = Connectors(self)
        self.sandboxes = Sandboxes(self)
        self.pipelines = Pipelines(self)

    def quota(self, *, owner: str | None = None):
        """The ceiling and what is against it; ``owner`` defaults to the caller's own."""
        return self.request("GET", "/v1/quota", query={"owner": owner})

    # ── request core ──────────────────────────────────────────────────────

    def request(self, method: str, path: str, *, body=None, query: dict | None = None):
        """One HTTP round trip. Returns parsed JSON (or None on an empty body);
        raises :class:`ApiError` on any non-2xx status."""
        return self._request(method, path, body, query)[0]

    def _request(self, method: str, path: str, body=None, query: dict | None = None,
                 *, text: bool = False, timeout="default"):
        """(decoded body, response headers). ``text`` returns the body as a str, not JSON."""
        req = self._prepare(self.base_url, method, path, body, query,
                            "text/plain" if text else "application/json")
        try:
            with urllib.request.urlopen(req, timeout=self._timeout if timeout == "default" else timeout) as resp:
                raw, headers = resp.read(), resp.headers
        except urllib.error.HTTPError as e:
            raise ApiError(e.code, _error_text(e.read())) from None
        if text:
            return raw.decode(errors="replace"), headers
        return (json.loads(raw) if raw else None), headers

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


def _drop(d: dict) -> dict:
    """The body without its ``None`` values: an unset input is the server's default."""
    return {k: v for k, v in d.items() if v is not None}


class Workspaces:
    """Btrfs-backed dev workspaces, over ``/v1/workspaces``. A workspace is named like an
    environment — ``"name"`` for the caller's own, ``"team/name"`` for one in a team — or by its
    id; pushes cut snapshots, kept on the workspace's VOLUME — see :class:`Volumes` for history,
    restore targets and deletion."""

    def __init__(self, client: Kloudlite):
        self._c = client

    def create(self, *, workspace: str, quota_gb: int, image: str | None = None,
               packages: list | None = None, ttl_secs: int | None = None,
               env: dict | None = None):
        """``workspace`` is ``"name"`` or ``"team/name"``. ``env`` maps variable names to literal
        values or ``"${{ secrets.NAME }}"`` references to the owner's secrets. ``ttl_secs``
        (600..=2592000) deletes it after that long unused; absent, never. ``packages`` are
        nixpkgs attributes, optionally pinned (``attr@version``). 202."""
        team, name = _owned(workspace)
        return self._c.request("POST", "/v1/workspaces", body=_drop({
            "name": name, "quota_gb": quota_gb, "team": team, "image": image,
            "packages": packages, "ttl_secs": ttl_secs, "env": env}))

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

    def restart(self, *, workspace: str):
        """Restart a running workspace so it gets the current env (202); 409 when not running."""
        return self._at("POST", workspace, "/restart")

    def take_snapshot(self, *, workspace: str, message: str | None = None, volume: str | None = None,
                      ttl_secs: int | None = None, never: bool | None = None):
        """Push a SNAPSHOT of the working copy — the one durable, restorable state (202). ``volume``
        names the volume and is required on its first push; ``ttl_secs`` (3600..=31536000) or
        ``never=True`` set its expiry. ``push`` is the old name."""
        return self._at("POST", workspace, "/snapshots", body=_drop(
            {"message": message, "volume": volume, "ttl_secs": ttl_secs, "never": never}))

    push = take_snapshot

    def clone(self, *, workspace: str, to: str, task: str | None = None):
        """A new workspace ``to`` from this one's state, cut at the moment of the request; the
        answer's ``based_on`` names exactly what it grafted onto."""
        return self._at("POST", workspace, "/clone", body={"name": to, "task": task})

    def restore(self, *, snapshot: str, to: str, team: str | None = None, image: str | None = None,
                packages: list | None = None, quota_gb: int | None = None):
        """A new workspace ``to`` grafted onto a PUSHED ``snapshot`` (a volume name, ``name@N`` or a
        snapshot id) — also one whose workspace is long gone. ``team`` (or ``to="team/name"``)
        is required for a team's volume. Absent overrides mean "whatever the snapshot froze"."""
        owner, name = _owned(to)
        return self._c.request("POST", "/v1/workspaces/restore", body=_drop({
            "name": name, "snapshot_id": snapshot, "team": team or owner,
            "image": image, "packages": packages, "quota_gb": quota_gb}))

    def set_packages(self, *, workspace: str, packages: list):
        """Replace the package list; pinned entries are locked before anything is written."""
        return self._at("PATCH", workspace, body={"packages": packages})

    def set_ttl(self, *, workspace: str, ttl_secs: int | None):
        """Seconds unused before the workspace is deleted (600..=2592000); ``None`` clears it. Not
        for a sandbox or a bench."""
        return self._at("PUT", workspace, "/ttl", body={"ttl_secs": ttl_secs})

    def set_env(self, *, workspace: str, env: dict, restart_on_env_change: bool | None = None):
        """Replace the whole env: literals or ``"${{ secrets.NAME }}"`` references. Takes effect on the
        next start; a running workspace is only marked ``restart_required`` unless
        ``restart_on_env_change=True``."""
        return self._at("PUT", workspace, "/env", body=_drop(
            {"env": env, "restartOnEnvChange": restart_on_env_change}))

    def update_packages(self, *, workspace: str):
        """Re-resolve every pinned package — the only call that moves existing locks."""
        return self._at("POST", workspace, "/packages/update")

    def tools(self, *, workspace: str):
        """The workspace's tool-server address and token — answered to the owner alone."""
        return self._at("GET", workspace, "/tools")

    def ide(self, *, workspace: str) -> "Ide":
        """An :class:`Ide` bound to this workspace's tool server. The address is a private IP:
        reachable from inside Kloudlite (a bench); from a laptop, tunnel with
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
        return self._c.request("POST", "/v1/environments", body=_drop({
            "name": name, "owner": owner, "services": _services(services) if services else None,
            "quota_gb": quota_gb, "snapshot_id": snapshot, "ttl_secs": ttl_secs}))

    def list(self, *, owner: str | None = None):
        """The caller's environments and their teams'; ``owner`` narrows to one handle or team."""
        return self._c.request("GET", "/v1/environments", query={"owner": owner})

    def get(self, *, environment: str):
        return self._at("GET", environment)

    def set_services(self, *, environment: str, services: list,
                     restart_on_env_change: bool | None = None):
        """Replace the WHOLE service list. A removed service keeps its folders on the disk, so
        adding it back finds its data. A changed ``env`` only marks a running service
        ``restartRequired`` unless ``restart_on_env_change=True``."""
        return self._at("PATCH", environment, body=_drop(
            {"services": _services(services), "restartOnEnvChange": restart_on_env_change}))

    def start(self, *, environment: str):
        return self._at("POST", environment, "/start")

    def stop(self, *, environment: str):
        return self._at("POST", environment, "/stop")

    def restart(self, *, environment: str, service: str):
        """Restart one service so it gets the current env (202); 409 when the environment is not running."""
        return self._at("POST", environment, f"/services/{_seg(service)}/restart")

    def set_ttl(self, *, environment: str, ttl_secs: int | None):
        """Seconds the environment may sit unused before it is deleted (600..=2592000); None clears."""
        return self._at("PUT", environment, "/ttl", body={"ttl_secs": ttl_secs})

    def delete(self, *, environment: str):
        """The volume survives while a pushed snapshot references it."""
        return self._at("DELETE", environment)

    def take_snapshot(self, *, environment: str, message: str | None = None,
                      volume: str | None = None, ttl_secs: int | None = None,
                      never: bool | None = None):
        """Push a SNAPSHOT of every service's data and the service list (202). ``volume`` names
        the volume and is required on its first push. ``push`` is the old name."""
        return self._at("POST", environment, "/snapshots", body=_drop(
            {"message": message, "volume": volume, "ttl_secs": ttl_secs, "never": never}))

    push = take_snapshot

    def clone(self, *, environment: str, to: str, snapshot: str | None = None,
              services: list | None = None, quota_gb: int | None = None,
              ttl_secs: int | None = None):
        """A new environment ``to`` (``"name"``; ``"team/name"`` only with ``snapshot``): a copy of
        the live disk, or with ``snapshot`` of one of this environment's snapshots. ``services``
        and ``quota_gb`` apply only with ``snapshot``."""
        owner, name = _owned(to)
        return self._at("POST", environment, "/clone", body=_drop({
            "name": name, "owner": owner, "snapshot_id": snapshot,
            "services": _services(services) if services else None, "quota_gb": quota_gb,
            "ttl_secs": ttl_secs}))

    def restore(self, *, environment: str, snapshot: str, discard: list | None = None):
        """Put THIS environment back to one of its own snapshots, services and data (202). A
        persistent service the snapshot lacks is a 409 naming it; list it in ``discard`` to
        accept losing its data."""
        return self._at("POST", environment, "/restore",
                        body=_drop({"snapshot_id": snapshot, "discard": discard}))

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

    def stop(self, *, session: str):
        """Stop the session and every workspace in it (202, ``{"stopped": [ids]}``); sandboxes and
        the environment are untouched."""
        return self._at("POST", session, "/stop")

    def create_workspace(self, *, session: str, workspace: str, quota_gb: int,
                         image: str | None = None, packages: list | None = None,
                         ttl_secs: int | None = None, env: dict | None = None):
        """Create a workspace ``workspace`` (a bare name) managed by the session, in its team
        (201). The session controls it from then on; it takes the inputs of
        :meth:`Workspaces.create`."""
        return self._at("POST", session, "/workspaces", body=_drop({
            "name": workspace, "quota_gb": quota_gb, "image": image, "packages": packages,
            "ttl_secs": ttl_secs, "env": env}))

    def delete_workspace(self, *, session: str, workspace: str):
        """Delete a workspace the session manages, releasing its intercepts first (204); snapshots stay."""
        return self._at("DELETE", session, f"/workspaces/{_seg(workspace)}")

    def create_sandbox(self, *, session: str, cmd: list, **options) -> "Sandbox":
        """Create a sandbox managed by the session (202), in its team and reaching its environment.
        Same inputs as :meth:`Sandboxes.create` (``team`` must be the session's)."""
        doc = self._at("POST", session, "/sandboxes", body=_sandbox_body(cmd=cmd, **options))
        return Sandbox(self._c, doc)

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

    def set(self, *, owner: str, name: str, value: str, scope: str = "mine") -> None:
        """Store ``name``. ``scope`` ``"mine"`` (default) is your own secret in the team ``owner``
        (``PUT /v1/secrets/{owner}/mine/{name}``; for your own handle, your personal one);
        ``"team"`` is the team's shared secret and needs an admin."""
        self._c.request("PUT", self._path(owner, scope, name), body={"value": value})

    def list(self, *, owner: str, scope: str = "mine"):
        """Names with ``updated_at`` and ``updated_by``, never values."""
        return self._c.request("GET", self._path(owner, scope))

    def delete(self, *, owner: str, name: str, scope: str = "mine") -> None:
        """409 while a workspace or environment still references it; the body names them."""
        self._c.request("DELETE", self._path(owner, scope, name))

    @staticmethod
    def _path(owner: str, scope: str, name: str | None = None) -> str:
        if scope not in ("mine", "team"):
            raise ValueError(f"scope is 'mine' or 'team', got {scope!r}")
        p = f"/v1/secrets/{_seg(owner)}" + ("/mine" if scope == "mine" else "")
        return p + (f"/{_seg(name)}" if name is not None else "")


class Connectors:
    """The provider accounts linked to an owner, over ``/v1/connectors``."""

    def __init__(self, client: Kloudlite):
        self._c = client

    def list(self, *, owner: str | None = None):
        """Connectors by name; ``owner`` is a handle or team slug, default your own."""
        return self._c.request("GET", "/v1/connectors", query={"owner": owner})

    def delete(self, *, name: str) -> None:
        """Remove a connector (204); a team's needs an admin."""
        self._c.request("DELETE", f"/v1/connectors/{_seg(name)}")


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

    def delete_by_id(self, *, volume: str):
        """Takes a DETACHED volume with all its snapshots; refused while a working copy remains."""
        return self._c.request("DELETE", f"/v1/volumes/{_seg(volume)}")

    def get(self, *, owner: str, name: str):
        """One named volume; ``owner`` is your handle or a team slug."""
        return self._c.request("GET", self._named(owner, name))

    def versions(self, *, owner: str, name: str):
        """The versions of a named volume, newest first."""
        return self._c.request("GET", self._named(owner, name, "/versions"))

    def delete(self, *, owner: str, name: str):
        """Delete a named volume and every snapshot on it (204); the name is free again."""
        return self._c.request("DELETE", self._named(owner, name))

    def delete_version(self, *, owner: str, name: str, n: int):
        """Delete version ``n`` (204); deleting the last one deletes the volume."""
        return self._c.request("DELETE", self._named(owner, name, f"/versions/{int(n)}"))

    @staticmethod
    def _named(owner: str, name: str, tail: str = "") -> str:
        return f"/v1/volumes/{_seg(owner)}/{_seg(name)}{tail}"


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


def _sandbox_body(*, cmd, image: str | None = None, packages: list | None = None,
                  from_workspace: str | None = None, from_snapshot: str | None = None,
                  team: str | None = None, tty: bool | None = None, stdin: bool | None = None,
                  timeout_s: int | None = None, network: str | None = None,
                  env: dict | None = None, cpu: int | None = None, memory_gb: int | None = None,
                  disk_gb: int | None = None) -> dict:
    """The ``POST /v1/sandboxes`` body; refuses a missing ``cmd`` before anything is sent."""
    if isinstance(cmd, (list, tuple)) and cmd and all(isinstance(a, str) for a in cmd):
        cmd = list(cmd)
    else:
        raise ValueError("cmd is required: the program and its arguments as a non-empty list of strings")
    return _drop({"cmd": cmd, "image": image, "packages": packages,
                  "from_workspace": from_workspace, "from_snapshot": from_snapshot, "team": team,
                  "tty": tty, "stdin": stdin, "timeout_secs": timeout_s, "network": network,
                  "env": env, "cpu": cpu, "memory_gb": memory_gb, "disk_gb": disk_gb})


class Sandboxes:
    """Throwaway, isolated machines that run ONE command, over ``/v1/sandboxes``. A sandbox starts
    from an image, from one of your workspaces (``from_workspace``) or from a pushed snapshot
    (``from_snapshot``), runs ``cmd``, and ends when it exits or after ``timeout_s`` (server
    default 300). Every call answers a :class:`Sandbox` handle, and every call here and on the
    handles also has an async twin, ``.aio``::

        sb = kl.sandboxes.create(cmd=["python3", "-c", "print(6 * 7)"], packages=["python3"])
        code = sb.wait()
        print(code, sb.logs())
    """

    def __init__(self, client: Kloudlite):
        self._c = client

    @_aio
    def create(self, *, cmd: list, image: str | None = None, packages: list | None = None,
               from_workspace: str | None = None, from_snapshot: str | None = None,
               team: str | None = None, tty: bool | None = None, stdin: bool | None = None,
               timeout_s: int | None = None, network: str | None = None, env: dict | None = None,
               cpu: int | None = None, memory_gb: int | None = None,
               disk_gb: int | None = None) -> "Sandbox":
        """Ask for a sandbox and return at once (202); it is usually still ``pending``.
        ``cmd`` (required) is the program then its arguments. ``tty`` runs it on a pty; ``stdin``
        keeps input open for ``attach``. ``timeout_s`` is 10..=86400. ``from_workspace`` is an id,
        ``"name"`` or ``"team/name"``; ``from_snapshot`` a volume name, ``name@N`` or snapshot id
        (exclusive with it). ``network`` is ``internet`` or ``none``; ``cpu`` 1..=8, ``memory_gb``
        1..=32. A session's sandbox is made with :meth:`Sessions.create_sandbox`. Unset inputs are
        left to the server's defaults."""
        body = _sandbox_body(cmd=cmd, image=image, packages=packages, from_workspace=from_workspace,
                             from_snapshot=from_snapshot, team=team, tty=tty, stdin=stdin,
                             timeout_s=timeout_s, network=network, env=env, cpu=cpu,
                             memory_gb=memory_gb, disk_gb=disk_gb)
        return Sandbox(self._c, self._c.request("POST", "/v1/sandboxes", body=body))

    @_aio
    def get(self, *, sandbox: str) -> "Sandbox":
        """One sandbox by id (a name does not resolve)."""
        return Sandbox(self._c, self._c.request("GET", f"/v1/sandboxes/{_seg(sandbox)}"))

    @_aio
    def list(self, *, team: str | None = None, limit: int | None = None,
             cursor: str | None = None) -> list:
        """ONE page of the caller's sandboxes, newest first (``limit`` 1..=200, default 50);
        ``team`` scopes to that team. :meth:`list_page` also answers the next cursor."""
        return self.list_page(team=team, limit=limit, cursor=cursor)[0]

    @_aio
    def list_page(self, *, team: str | None = None, limit: int | None = None,
                  cursor: str | None = None) -> tuple:
        """``(sandboxes, next_cursor)``: a page, and the ``X-Next-Cursor`` to pass as ``cursor`` for
        the next one (``None`` on the last page)."""
        docs, headers = self._c._request("GET", "/v1/sandboxes",
                                         query={"team": team, "limit": limit, "cursor": cursor})
        return [Sandbox(self._c, d) for d in docs or []], headers.get("X-Next-Cursor")

    @_aio
    def run(self, *, cmd: list, timeout_s: float | None = None, **options) -> dict:
        """One command in a fresh sandbox: create, wait for it to exit, read the output, and
        terminate — ALWAYS, also when anything before it raised. Answers
        ``{"exit_code", "stdout", "stderr", "sandbox"}``: the sandbox keeps one stream, so
        ``stdout`` holds the output and error output together and ``stderr`` is ``""``.
        ``timeout_s`` bounds the wait (TimeoutError; None waits as long as it takes) and, when
        given, sizes the sandbox's own lifetime to cover it plus a minute. ``options`` are the
        other inputs of :meth:`create` (``image``, ``packages``, ``env``, …)."""
        if timeout_s is not None and "timeout_s" not in options:
            options["timeout_s"] = min(86_400, max(10, math.ceil(timeout_s) + 60))
        sb = self.create(cmd=cmd, **options)
        try:
            code = sb.wait(timeout_s=timeout_s)
            out = sb.logs()
        except BaseException:
            with contextlib.suppress(Exception):  # never mask the failure that got us here
                sb.terminate()
            raise
        sb.terminate()
        return {"exit_code": code, "stdout": out, "stderr": "", "sandbox": sb.id}


class Sandbox:
    """A handle on one sandbox: ``id``, ``phase`` (``pending``, ``starting``, ``running``,
    ``terminated``), ``reason`` once terminated, ``exit_code``, and the whole server ``doc`` as of
    the last answer — :meth:`refresh` re-reads it."""

    def __init__(self, client: Kloudlite, doc: dict):
        self._c = client
        self.doc = doc

    id = property(lambda self: self.doc["id"])
    phase = property(lambda self: self.doc.get("phase"))
    reason = property(lambda self: self.doc.get("reason"))
    exit_code = property(lambda self: self.doc.get("exit_code"))

    def __repr__(self):
        return f"<Sandbox {self.id} {self.phase}{' ' + self.reason if self.reason else ''}>"

    @_aio
    def refresh(self) -> "Sandbox":
        self.doc = self._c.request("GET", self._path())
        return self

    @_aio
    def wait_running(self, *, timeout_s: float = 120) -> "Sandbox":
        """Poll until ``running``; :class:`SandboxTerminated` naming the reason if it terminates
        instead (an image that does not pull, a quota, a node that cannot place it, or a command
        that already exited)."""
        for _ in _backoff(timeout_s, f"sandbox {self.id} not running"):
            self.refresh()
            if self.phase == "running":
                return self
            if self.phase == "terminated":
                raise SandboxTerminated(self.id, self.reason, self.doc.get("message"))

    @_aio
    def logs(self, *, follow: bool = False) -> str:
        """The main process's output (stdout and stderr together) as text: the live output while it
        runs, the last 256 KiB after. ``follow=True`` holds until the process ends. 409 while the
        sandbox has no output yet (still starting)."""
        return self._c._request("GET", self._path("/logs"), query={"follow": "true" if follow else None},
                                text=True, timeout=None if follow else "default")[0]

    def attach(self, *, cols: int | None = None, rows: int | None = None):
        """Not in this SDK: ``GET /v1/sandboxes/{id}/attach`` is a WebSocket and this client uses
        only the standard library."""
        raise NotImplementedError("sandbox attach is a WebSocket and is not in the Python SDK")

    @_aio
    def terminate(self) -> "Sandbox":
        """Stop it for good (202, or 200 if already ended); the record stays until :meth:`delete`."""
        self.doc = self._c.request("POST", self._path("/terminate")) or self.doc
        return self

    @_aio
    def wait(self, *, timeout_s: float | None = None) -> int | None:
        """Block until it has ended and answer its exit code (``None`` when it ended some other way
        than the command exiting; ``reason`` says how). Waits in steps of at most 20 s, the most the
        api holds one open; TimeoutError past ``timeout_s``."""
        deadline = None if timeout_s is None else time.monotonic() + timeout_s
        while True:
            left_ms = 20_000 if deadline is None else int((deadline - time.monotonic()) * 1000)
            if left_ms <= 0:
                raise TimeoutError(f"sandbox {self.id} still {self.phase} after {timeout_s} s")
            out = self._c.request("POST", self._path("/wait"), body={"timeout_ms": min(left_ms, 20_000)})
            self.doc = {**self.doc, **{k: v for k, v in (out or {}).items() if k in
                                       ("phase", "reason", "exit_code", "message")}}
            if self.phase == "terminated":
                return self.exit_code

    @_aio
    def delete(self):
        """Delete the record (204); a sandbox still running dies with it."""
        return self._c.request("DELETE", self._path())

    def _path(self, tail: str = "") -> str:
        return f"/v1/sandboxes/{_seg(self.id)}{tail}"


_RUN_TERMINAL = ("success", "failure", "cancelled")


class Pipelines:
    """Pipelines, over ``/v1/pipelines``: a pipeline is a JSON definition stored as numbered
    versions; a run belongs to a pipeline and executes one version, each job in its own sandbox.
    :meth:`run`, :meth:`runs` and :meth:`get_run` answer :class:`Run` handles, and every call here
    and on a handle also has an async twin, ``.aio``::

        kl.pipelines.create(owner="acme", definition=definition)
        run = kl.pipelines.run(owner="acme", pipeline="test", inputs={"target": "staging"})
        for chunk in run.follow(job="test"):
            print(chunk, end="")
        assert run.wait() == "success"
    """

    def __init__(self, client: Kloudlite):
        self._c = client

    @_aio
    def create(self, *, owner: str, definition: dict) -> dict:
        """Save ``definition`` (its ``name`` is the pipeline's) as version 1 (201). A webhook
        trigger's secret URL is in the answer's ``webhookUrl``, this once. 409 if the name exists."""
        return self._c.request("POST", f"/v1/pipelines/{_seg(owner)}", body=definition)

    @_aio
    def update(self, *, owner: str, name: str, definition: dict) -> dict:
        """Replace the definition: a changed one is the next version, an equal one saves nothing
        and answers the latest. To roll back, send an old definition. A concurrent save is an
        :class:`ApiError` 409 (``changed; retry``)."""
        return self._c.request("PUT", f"/v1/pipelines/{_seg(owner)}/{_seg(name)}", body=definition)

    @_aio
    def list(self, *, owner: str) -> list:
        """``owner``'s pipelines by name: ``name``, ``latestVersion``, ``updatedAtMs``, ``triggers``."""
        return self._c.request("GET", f"/v1/pipelines/{_seg(owner)}")

    @_aio
    def get(self, *, owner: str, name: str, version: int | None = None) -> dict:
        """The pipeline document of ``version`` (default the latest), ``definition`` included."""
        return self._c.request("GET", f"/v1/pipelines/{_seg(owner)}/{_seg(name)}",
                               query={"version": version})

    @_aio
    def versions(self, *, owner: str, name: str) -> list:
        """The versions, oldest first: ``version``, ``createdBy``, ``createdAtMs``, ``definitionSha256``."""
        return self._c.request("GET", f"/v1/pipelines/{_seg(owner)}/{_seg(name)}/versions")

    @_aio
    def delete(self, *, owner: str, name: str) -> None:
        """Delete the pipeline and its versions; finished runs stay. 409 while a run is queued or running."""
        self._c.request("DELETE", f"/v1/pipelines/{_seg(owner)}/{_seg(name)}")

    @_aio
    def delete_version(self, *, owner: str, name: str, version: int) -> None:
        """Delete one version; 409 for the latest or one a queued or running run uses."""
        self._c.request("DELETE", f"/v1/pipelines/{_seg(owner)}/{_seg(name)}/versions/{int(version)}")

    @_aio
    def rotate_webhook(self, *, owner: str, name: str) -> dict:
        """Make a new webhook secret; the old URL stops at once. ``{"webhookUrl"}``, shown only here."""
        return self._c.request("POST", f"/v1/pipelines/{_seg(owner)}/{_seg(name)}/webhook/rotate")

    @_aio
    def run(self, *, owner: str, pipeline: str, version: int | None = None,
            inputs: dict | None = None, environment: str | None = None,
            clone: str | None = None) -> "Run":
        """Start ``pipeline`` by hand and answer the :class:`Run` at once (202), ``queued``.
        ``version`` defaults to the newest; ``inputs`` are string values of the inputs it declares.
        ``environment`` runs every job that names none in that environment; ``clone`` instead in a
        per-run copy of it — one or the other. A version that cannot run is an :class:`ApiError`
        422 whose message lists every reason, and nothing is recorded."""
        body = {"version": version, "inputs": inputs,
                "environment": _run_environment(environment, clone)}
        doc = self._c.request("POST", f"/v1/pipelines/{_seg(owner)}/{_seg(pipeline)}/runs",
                              body={k: v for k, v in body.items() if v is not None})
        return Run(self._c, doc)

    @_aio
    def runs(self, *, owner: str, pipeline: str, limit: int | None = None) -> list:
        """The pipeline's runs, newest first (``limit`` default 50, at most 200)."""
        docs = self._c.request("GET", f"/v1/pipelines/{_seg(owner)}/{_seg(pipeline)}/runs",
                               query={"limit": limit})
        return [Run(self._c, d) for d in docs or []]

    @_aio
    def get_run(self, *, owner: str, pipeline: str, run: str) -> "Run":
        """One run by its id (``1760400000000-0a1b2c``), ``definition`` set while its version exists."""
        return Run(self._c, self._c.request(
            "GET", f"/v1/pipelines/{_seg(owner)}/{_seg(pipeline)}/runs/{_seg(run)}"))


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
    owner = property(lambda self: self.doc.get("owner"))
    pipeline_version = property(lambda self: self.doc.get("pipelineVersion"))
    definition_sha256 = property(lambda self: self.doc.get("definitionSha256"))
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
        return f"<Run {self.owner}/{self.pipeline}#{self.number} {self.state}>"

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
        return f"/v1/pipelines/{_seg(self.owner)}/{_seg(self.pipeline)}/runs/{_seg(self.id)}{tail}"

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
