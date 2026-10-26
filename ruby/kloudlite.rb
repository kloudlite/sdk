# Kloudlite SDK for Ruby — stdlib only (net/http + json). Needs Ruby 3.0 or later (endless
# methods); an older Ruby stops at the first such method with a SyntaxError.
#
# Auth is a Bearer token (a CLI token). Every module hangs off the one client:
#
#   require_relative "kloudlite"
#   kl = Kloudlite::Client.new   # the login of `kloudlite`; see "Discovery" below
#   workspaces = kl.workspaces.list
#
# Discovery: the API origin and the token each come from the first source that has one:
# 1. the keywords base_url: / token:; 2. the environment KLOUDLITE_URL / KLOUDLITE_TOKEN;
# 3. "api" / "token" in config.json in KLOUDLITE_CONFIG_DIR, else ~/.config/kloudlite. "/v1" is
# added to the origin. With no token the constructor raises RuntimeError carrying the sentence
# "no Kloudlite login: pass token=, set KLOUDLITE_TOKEN, or run kloudlite login".
#
# Every public method takes KEYWORD arguments, so no caller has to remember an order; a
# resource is always the keyword named after it (workspace:, environment:, session:,
# volume:, snapshot:, sandbox:, run:), and a new object made from an existing one is to:.

raise LoadError, "kloudlite.rb needs Ruby 3.0 or later, found #{RUBY_VERSION}" if RUBY_VERSION < "3.0"

require "json"
require "net/http"
require "timeout"
require "uri"

module Kloudlite
  VERSION = "0.2.2"
  USER_AGENT = "kloudlite-sdk-ruby/#{VERSION}".freeze
  NO_LOGIN = "no Kloudlite login: pass token=, set KLOUDLITE_TOKEN, or run kloudlite login"

  # A non-2xx answer. +status+ is the HTTP code, +message+ the server's text.
  class ApiError < StandardError
    attr_reader :status, :message

    def initialize(status, message)
      @status = status
      @message = message
      super("#{status}: #{message}")
    end
  end

  # A sandbox that ended while the caller waited for it to run; +reason+ is the server's
  # (Timeout, Idle, Unplaceable, …). An ApiError so one rescue covers both.
  class SandboxTerminated < ApiError
    attr_reader :reason

    def initialize(sandbox, reason, detail)
      @reason = reason
      text = "sandbox #{sandbox} is terminated (#{reason || 'unknown'})"
      text += ": #{detail}" unless detail.nil? || detail.to_s.empty?
      super(409, text)
    end
  end

  # A client-side wait ran out. A ::Timeout::Error, so a generic rescue still catches it.
  class TimeoutError < ::Timeout::Error; end

  # The naming rules every module shares. Mixed in privately; never part of the public surface.
  module Names
    private

    # "name" → [nil, name]; "team/name" → [team, name]. Workspaces, environments and sessions
    # send the pair as path segment +name+ plus ?owner=team, which the server folds back.
    def owned(s)
      s = s.to_s
      i = s.rindex("/")
      return [nil, s] if i.nil?
      owner, name = s[0...i], s[(i + 1)..]
      raise ArgumentError, "expected name or team/name, got #{s.inspect}" if owner.empty? || name.empty? || owner.include?("/")
      [owner, name]
    end

    # Ruby 3.0 has no URI.encode_uri_component (3.2); this is the same escape.
    def seg(s)
      URI.encode_www_form_component(s.to_s).gsub("+", "%20")
    end
  end

  class Client
    attr_reader :workspaces, :environments, :sessions, :volumes,
                :secrets, :connectors, :sandboxes, :pipelines

    def initialize(base_url: nil, token: nil, timeout: 30)
      base_url = presence(base_url) || presence(ENV["KLOUDLITE_URL"])
      token = presence(token) || presence(ENV["KLOUDLITE_TOKEN"])
      if base_url.nil? || token.nil?
        cfg = login
        base_url ||= presence(cfg["api"])
        token ||= presence(cfg["token"])
      end
      raise RuntimeError, NO_LOGIN if token.nil?
      raise RuntimeError, "no Kloudlite API URL: pass base_url=, set KLOUDLITE_URL, or run kloudlite login" if base_url.nil?

      @base_url = base_url.sub(%r{/+\z}, "").sub(%r{/v1\z}, "") # paths carry /v1
      @token = token
      @timeout = timeout
      @workspaces = Workspaces.new(self)
      @environments = Environments.new(self)
      @sessions = Sessions.new(self)
      @volumes = Volumes.new(self)
      @secrets = Secrets.new(self)
      @connectors = Connectors.new(self)
      @sandboxes = Sandboxes.new(self)
      @pipelines = Pipelines.new(self)
    end

    # The ceiling and what is against it; +owner+ defaults to the caller's own.
    def quota(owner: nil)
      request("GET", "/v1/quota", query: { owner: owner })
    end

    # One HTTP round trip. Returns parsed JSON (or nil on an empty body);
    # raises ApiError on any non-2xx status.
    def request(method, path, body: nil, query: nil)
      text = perform(method, path, body, query, "application/json", @timeout).body
      text.nil? || text.empty? ? nil : JSON.parse(text)
    end

    # The server answers errors as {"error": …} or plain text; take either. Shared with Ide.
    def self.error_text(raw)
      text = (raw || "").strip
      begin
        parsed = JSON.parse(text)
        return parsed["error"].to_s if parsed.is_a?(Hash) && parsed.key?("error")
      rescue JSON::ParserError
        # plain text
      end
      text.empty? ? "request failed" : text
    end

    private

    def presence(v)
      v.nil? || v.to_s.empty? ? nil : v.to_s
    end

    # config.json of the `kloudlite` login; {} when it is absent or unreadable.
    def login
      dir = presence(ENV["KLOUDLITE_CONFIG_DIR"]) || File.join(Dir.home, ".config", "kloudlite")
      doc = JSON.parse(File.read(File.join(dir, "config.json")))
      doc.is_a?(Hash) ? doc : {}
    rescue SystemCallError, IOError, JSON::ParserError, ArgumentError
      {}
    end

    # The raw Net::HTTP response of a 2xx; ApiError otherwise. The text, header-reading and
    # no-timeout (+read_timeout+ nil) callers reach it through __send__ — not public surface.
    def perform(method, path, body, query, accept, read_timeout)
      uri = URI.parse(@base_url + path)
      q = (query || {}).compact
      uri.query = URI.encode_www_form(q) unless q.empty?
      req = Net::HTTP.const_get(method.capitalize).new(uri)
      req["User-Agent"] = USER_AGENT
      req["Accept"] = accept
      req["Authorization"] = "Bearer #{@token}"
      unless body.nil?
        req["Content-Type"] = "application/json"
        req.body = JSON.generate(body)
      end
      resp = Net::HTTP.start(uri.hostname, uri.port, use_ssl: uri.scheme == "https",
                             open_timeout: @timeout, read_timeout: read_timeout) { |http| http.request(req) }
      raise ApiError.new(resp.code.to_i, self.class.error_text(resp.body)) unless resp.is_a?(Net::HTTPSuccess)
      resp
    end
  end

  # Btrfs-backed dev workspaces, over /v1/workspaces. A workspace is named like an environment —
  # "name" for the caller's own, "team/name" for a team's — or by its id; pushes cut snapshots,
  # kept on the workspace's VOLUME — see Volumes for history, restore targets and deletion.
  class Workspaces
    include Names

    def initialize(client)
      @c = client
    end

    # +workspace+ ("name" or "team/name") is split into the body's name and team. +packages+ are
    # nixpkgs attributes, optionally pinned ("attr@version"). +ttl_secs+ (600..=2592000) deletes
    # it after that long unused; absent, never. +env+ maps variables to literals or
    # "${{ secrets.NAME }}" references. Unset keywords are omitted (202).
    def create(workspace:, quota_gb:, image: nil, packages: nil, ttl_secs: nil, env: nil)
      team, name = owned(workspace)
      @c.request("POST", "/v1/workspaces",
                 body: { name: name, quota_gb: quota_gb, team: team, image: image,
                         packages: packages, ttl_secs: ttl_secs, env: env }.compact)
    end

    # The caller's workspaces; +team+ scopes to their workspaces in that team.
    def list(team: nil)
      @c.request("GET", "/v1/workspaces", query: { team: team })
    end

    def get(workspace:)
      at("GET", workspace)
    end

    # Runs the worktree finalizer: the volume survives while a pushed snapshot references it.
    def delete(workspace:)
      at("DELETE", workspace)
    end

    def start(workspace:)
      at("POST", workspace, "/start")
    end

    # A stop cuts a sync point and tears the workspace down — seconds, never waiting on a replica.
    def stop(workspace:)
      at("POST", workspace, "/stop")
    end

    # Restart a running workspace so it gets the current env (202); 409 when not running.
    def restart(workspace:)
      at("POST", workspace, "/restart")
    end

    # Push a SNAPSHOT of the working copy — the one durable, restorable state (202). +volume+
    # names the volume and is required on its first push; +ttl_secs+ (3600..=31536000) or
    # never: true set its expiry. +push+ is the old name.
    def take_snapshot(workspace:, message: nil, volume: nil, ttl_secs: nil, never: nil)
      at("POST", workspace, "/snapshots",
         body: { message: message, volume: volume, ttl_secs: ttl_secs, never: never }.compact)
    end
    alias_method :push, :take_snapshot

    # A new workspace +to+ from this one's state, cut at the moment of the request; the
    # answer's +based_on+ names exactly what it grafted onto.
    def clone(workspace:, to:, task: nil)
      at("POST", workspace, "/clone", body: { name: to, task: task })
    end

    # A new workspace +to+ grafted onto a PUSHED +snapshot+ (a volume name, "name@N" or a snapshot
    # id) — also one whose workspace is long gone. +team+ (or to: "team/name") is required for a
    # team's volume. Absent overrides mean "whatever the snapshot froze".
    def restore(snapshot:, to:, team: nil, image: nil, packages: nil, quota_gb: nil)
      owner, name = owned(to)
      @c.request("POST", "/v1/workspaces/restore",
                 body: { name: name, snapshot_id: snapshot, team: team || owner,
                         image: image, packages: packages, quota_gb: quota_gb }.compact)
    end

    # Replace the package list; pinned entries are locked before anything is written.
    def set_packages(workspace:, packages:)
      at("PATCH", workspace, body: { packages: packages })
    end

    # Seconds unused before the workspace is deleted (600..=2592000); nil clears it. Not for a
    # sandbox or a bench.
    def set_ttl(workspace:, ttl_secs:)
      at("PUT", workspace, "/ttl", body: { ttl_secs: ttl_secs })
    end

    # Replace the whole env (literals or "${{ secrets.NAME }}" references); takes effect on the
    # next start. A running workspace is only marked restart_required unless
    # restart_on_env_change: true.
    def set_env(workspace:, env:, restart_on_env_change: nil)
      at("PUT", workspace, "/env", body: { env: env, restartOnEnvChange: restart_on_env_change }.compact)
    end

    # Re-resolve every pinned package — the only call that moves existing locks.
    def update_packages(workspace:)
      at("POST", workspace, "/packages/update")
    end

    # The workspace's tool-server address and token — answered to the owner alone.
    def tools(workspace:)
      at("GET", workspace, "/tools")
    end

    # An Ide bound to this workspace's tool server. The address is a private IP: reachable from
    # inside Kloudlite (a bench); from a laptop, tunnel with `kl-connect ws ide` and use
    # Ide.new(url: "http://127.0.0.1:7788", token: token) instead.
    def ide(workspace:)
      doc = tools(workspace: workspace)
      Ide.new(url: "http://#{doc['address']}", token: doc["token"])
    end

    private

    def at(method, workspace, tail = "", body: nil)
      owner, name = owned(workspace)
      @c.request(method, "/v1/workspaces/#{seg(name)}#{tail}", body: body, query: { owner: owner })
    end
  end

  # A workspace's tool server (`kl ide serve`): the full tool set over plain HTTP — GET /tools
  # lists every tool with its schema, POST /tools/{name} runs one. +call+ takes anything the
  # listing names; nil arguments are dropped, so every optional means "the tool's own default".
  class Ide
    def initialize(url:, token: nil, timeout: 630)
      @url = url.sub(%r{/+\z}, "")
      @token = token
      @timeout = timeout
    end

    # Every tool, with its JSON schema — the authority on arguments.
    def list
      http("GET", "/tools")
    end

    # +tool+ names it; every other keyword is the tool's own argument, sent as given.
    def call(tool:, **args)
      body = args.compact.transform_keys(&:to_s)
      http("POST", "/tools/#{URI.encode_www_form_component(tool.to_s).gsub("+", "%20")}", body)
    end

    # files
    def read(path: nil, paths: nil, offset: nil, limit: nil)
      call(tool: "read", path: path, paths: paths, offset: offset, limit: limit)
    end

    def write(path:, content:)
      call(tool: "write", path: path, content: content)
    end

    # One file as (path:, edits:), several as files: [{path:, edits:}]; each edit is
    # {old:, new:, replace_all:?} and the whole call is atomic.
    def edit(path: nil, edits: nil, files: nil)
      call(tool: "edit", path: path, edits: edits, files: files)
    end

    def patch(diff:, cwd: nil)
      call(tool: "patch", diff: diff, cwd: cwd)
    end

    def glob(pattern:, cwd: nil)
      call(tool: "glob", pattern: pattern, cwd: cwd)
    end

    def grep(pattern:, cwd: nil, glob: nil, mode: nil, context: nil, ignore_case: nil, max: nil)
      call(tool: "grep", pattern: pattern, cwd: cwd, glob: glob, mode: mode,
                         context: context, ignore_case: ignore_case, max: max)
    end

    # commands and processes — detach: true answers {id}, passed back as +process+
    def exec(cmd:, cwd: nil, env: nil, timeout_ms: nil, head: nil, tail: nil, quiet: nil, detach: nil)
      call(tool: "exec", cmd: cmd, cwd: cwd, env: env, timeout_ms: timeout_ms,
                         head: head, tail: tail, quiet: quiet, detach: detach)
    end

    def process_list
      call(tool: "process_list")
    end

    def process_output(process:, since: nil, since_err: nil)
      call(tool: "process_output", id: process, since: since, since_err: since_err)
    end

    def process_write(process:, data:)
      call(tool: "process_write", id: process, data: data)
    end

    def process_kill(process:, signal: nil)
      call(tool: "process_kill", id: process, signal: signal)
    end

    # watches — +watch+ answers {id}, passed back as +watch+
    def watch(paths: nil, cmd: nil, pattern: nil, once: nil)
      call(tool: "watch", paths: paths, cmd: cmd, pattern: pattern, once: once)
    end

    def watch_poll(watch:, since: nil)
      call(tool: "watch_poll", id: watch, since: since)
    end

    def watch_stop(watch:)
      call(tool: "watch_stop", id: watch)
    end

    # containers (the hidden per-owner builder)
    def container_build(tags:, context: nil, file: nil, build_args: nil, platform: nil,
                        no_cache: nil, timeout_ms: nil, head: nil, tail: nil)
      call(tool: "container_build", tags: tags, context: context, file: file, build_args: build_args,
                                    platform: platform, no_cache: no_cache, timeout_ms: timeout_ms,
                                    head: head, tail: tail)
    end

    def container_push(src:, dst:)
      call(tool: "container_push", src: src, dst: dst)
    end

    private

    def http(method, path, body = nil)
      uri = URI.parse(@url + path)
      req = Net::HTTP.const_get(method.capitalize).new(uri)
      req["User-Agent"] = USER_AGENT
      req["Accept"] = "application/json"
      req["Authorization"] = "Bearer #{@token}" if @token
      if body
        req["Content-Type"] = "application/json"
        req.body = JSON.generate(body)
      end
      resp = Net::HTTP.start(uri.hostname, uri.port,
                             use_ssl: uri.scheme == "https",
                             open_timeout: @timeout, read_timeout: @timeout) { |http| http.request(req) }
      raise ApiError.new(resp.code.to_i, Client.error_text(resp.body)) unless resp.is_a?(Net::HTTPSuccess)
      resp.body.nil? || resp.body.empty? ? nil : JSON.parse(resp.body)
    end
  end

  # Multi-service environments, over /v1/environments: a set of containers sharing one disk. An
  # environment is named by ONE string — "name" for the caller's own, "team/name" for a team's —
  # or by its id ("env-…"). Each service is a hash: +name+ and +image+, plus optional +ports+,
  # +env+, +command+ and +mounts+ ([{ folder:, path: }], a folder of the environment's disk).
  class Environments
    include Names

    def initialize(client)
      @c = client
    end

    # +snapshot+ grows it from a pushed snapshot — also one of a deleted environment — and
    # then +services+ default to the ones that snapshot froze. +ttl_secs+ (600..=2592000) deletes
    # it after that long unused; absent, never.
    def create(environment:, services: nil, quota_gb: nil, snapshot: nil, ttl_secs: nil)
      owner, name = owned(environment)
      @c.request("POST", "/v1/environments",
                 body: { name: name, owner: owner,
                         services: services.nil? || services.empty? ? nil : fill(services),
                         quota_gb: quota_gb, snapshot_id: snapshot, ttl_secs: ttl_secs }.compact)
    end

    # The caller's environments and their teams'; +owner+ narrows to one handle or team.
    def list(owner: nil)
      @c.request("GET", "/v1/environments", query: { owner: owner })
    end

    def get(environment:)
      at("GET", environment)
    end

    # Replace the WHOLE service list. A removed service keeps its folders, so re-adding finds its
    # data. A changed env only marks a running service restartRequired unless
    # restart_on_env_change: true.
    def set_services(environment:, services:, restart_on_env_change: nil)
      at("PATCH", environment,
         body: { services: fill(services), restartOnEnvChange: restart_on_env_change }.compact)
    end

    def start(environment:)
      at("POST", environment, "/start")
    end

    def stop(environment:)
      at("POST", environment, "/stop")
    end

    # Restart one service so it gets the current env (202); 409 when the environment is not running.
    def restart(environment:, service:)
      at("POST", environment, "/services/#{seg(service)}/restart")
    end

    # Seconds the environment may sit unused (600..=2592000) before it is deleted; nil clears.
    def set_ttl(environment:, ttl_secs:)
      at("PUT", environment, "/ttl", body: { ttl_secs: ttl_secs })
    end

    # The volume survives while a pushed snapshot references it.
    def delete(environment:)
      at("DELETE", environment)
    end

    # Push a SNAPSHOT of every service's data and the service list (202). +volume+ names the
    # volume and is required on its first push. +push+ is the old name.
    def take_snapshot(environment:, message: nil, volume: nil, ttl_secs: nil, never: nil)
      at("POST", environment, "/snapshots",
         body: { message: message, volume: volume, ttl_secs: ttl_secs, never: never }.compact)
    end
    alias_method :push, :take_snapshot

    # A new environment +to+ ("name"; "team/name" only with +snapshot+): a copy of the live disk,
    # or with +snapshot+ of one of this environment's snapshots. +services+ and +quota_gb+ apply
    # only with +snapshot+.
    def clone(environment:, to:, snapshot: nil, services: nil, quota_gb: nil, ttl_secs: nil)
      owner, name = owned(to)
      at("POST", environment, "/clone",
         body: { name: name, owner: owner, snapshot_id: snapshot,
                 services: services.nil? || services.empty? ? nil : fill(services),
                 quota_gb: quota_gb, ttl_secs: ttl_secs }.compact)
    end

    # Put THIS environment back to one of its own snapshots, services and data (202). A
    # persistent service the snapshot lacks is a 409 naming it; list it in +discard+ to accept
    # losing its data.
    def restore(environment:, snapshot:, discard: nil)
      at("POST", environment, "/restore", body: { snapshot_id: snapshot, discard: discard }.compact)
    end

    private

    def at(method, environment, tail = "", body: nil)
      owner, name = owned(environment)
      @c.request(method, "/v1/environments/#{seg(name)}#{tail}", body: body, query: { owner: owner })
    end

    # The server requires every field; fill the ones a caller leaves out.
    def fill(services)
      services.map { |sv| { command: [], env: {}, mounts: [], ports: [] }.merge(sv.transform_keys(&:to_sym)) }
    end
  end

  # Working sessions, over /v1/sessions: some of YOUR workspaces in one team, connected to one
  # environment. Many people's sessions connect to the same environment and collaborate through
  # it; an intercept a session takes is environment-wide, and the session owns it — only it can
  # release it, and removing the workspace or deleting the session releases it.
  #
  # A session is named like an environment — "name" (personal) or "team/name" — or by its id
  # ("ses-…"); a workspace by its id, name or "team/name", sent verbatim.
  class Sessions
    include Names

    def initialize(client)
      @c = client
    end

    # +environment+, when given, must be the session's team's: an id, a name or "team/name";
    # without one the session connects to nothing until +connect+. +ttl_secs+ (600..=2592000)
    # deletes the session after that long unused.
    def create(session:, environment: nil, ttl_secs: nil)
      team, name = owned(session)
      @c.request("POST", "/v1/sessions", body: { name: name, team: team, environment: environment, ttl_secs: ttl_secs })
    end

    # Seconds unused before the session is deleted (600..=2592000); nil clears it.
    def set_ttl(session:, ttl_secs:)
      at("PUT", session, "/ttl", body: { ttl_secs: ttl_secs })
    end

    def list(team: nil)
      @c.request("GET", "/v1/sessions", query: { team: team })
    end

    # The session with its +workspaces+ and the +intercepts+ it holds.
    def get(session:)
      at("GET", session)
    end

    # Releases its intercepts and lets its workspaces go — never deletes a workspace.
    def delete(session:)
      at("DELETE", session)
    end

    # Sets, switches or (nil) clears the session's environment. Its intercepts in the old
    # environment are released first; deleting an environment disconnects its sessions the same way.
    def connect(session:, environment:)
      at("PUT", session, "/environment", body: { environment: environment })
    end

    # Stop the session and every workspace in it (202, {"stopped" => [ids]}); sandboxes and the
    # environment are untouched.
    def stop(session:)
      at("POST", session, "/stop")
    end

    # Create a workspace +workspace+ (a bare name) managed by the session, in its team (201). The
    # session controls it from then on; it takes the inputs of Workspaces#create.
    def create_workspace(session:, workspace:, quota_gb:, image: nil, packages: nil, ttl_secs: nil, env: nil)
      at("POST", session, "/workspaces",
         body: { name: workspace, quota_gb: quota_gb, image: image, packages: packages,
                 ttl_secs: ttl_secs, env: env }.compact)
    end

    # Delete a workspace the session manages, releasing its intercepts first (204); snapshots stay.
    def delete_workspace(session:, workspace:)
      at("DELETE", session, "/workspaces/#{seg(workspace)}")
    end

    # Create a sandbox managed by the session (202), in its team and reaching its environment.
    # Same inputs as Sandboxes#create (+team+ must be the session's).
    def create_sandbox(session:, cmd:, **options)
      Sandbox.new(@c, at("POST", session, "/sandboxes", body: Sandboxes.body(cmd: cmd, **options)))
    end

    # Send everything that calls +service+ in the environment to +workspace+ (a member), +ports+
    # mapping each service port to the workspace port that answers it ({ 8080 => 3000 }); an
    # unmapped port answers on the same number.
    def intercept(session:, service:, workspace:, ports: nil)
      at("POST", session, "/intercepts",
         body: { service: service, workspace: workspace,
                 ports: (ports || {}).map { |s, w| { service: Integer(s), workspace: Integer(w) } } })
    end

    def release(session:, service:)
      at("DELETE", session, "/intercepts/#{seg(service)}")
    end

    private

    def at(method, session, tail = "", body: nil)
      owner, name = owned(session)
      @c.request(method, "/v1/sessions/#{seg(name)}#{tail}", body: body, query: { owner: owner })
    end
  end

  # An owner's named secrets, over /v1/secrets. A value goes in and never comes back: +list+
  # answers names with who and when. Reference one from a workspace env as "${{ secrets.NAME }}".
  # +scope+ "mine" (default) is your own secret in the team +owner+ (for your own handle, your
  # personal one); "team" is the team's shared secret and needs an admin.
  class Secrets
    include Names

    def initialize(client)
      @c = client
    end

    def set(owner:, name:, value:, scope: "mine")
      @c.request("PUT", path(owner, scope, name), body: { value: value })
      nil
    end

    def list(owner:, scope: "mine")
      @c.request("GET", path(owner, scope))
    end

    # 409 while a workspace or environment still references it; the body names them.
    def delete(owner:, name:, scope: "mine")
      @c.request("DELETE", path(owner, scope, name))
      nil
    end

    private

    def path(owner, scope, name = nil)
      raise ArgumentError, "scope is 'mine' or 'team', got #{scope.inspect}" unless %w[mine team].include?(scope.to_s)

      "/v1/secrets/#{seg(owner)}#{'/mine' if scope.to_s == 'mine'}#{"/#{seg(name)}" if name}"
    end
  end

  # The provider accounts linked to an owner, over /v1/connectors.
  class Connectors
    include Names

    def initialize(client)
      @c = client
    end

    # Connectors by name; +owner+ is a handle or team slug, default your own.
    def list(owner: nil)
      @c.request("GET", "/v1/connectors", query: { owner: owner })
    end

    # Remove a connector (204); a team's needs an admin.
    def delete(name:)
      @c.request("DELETE", "/v1/connectors/#{seg(name)}")
      nil
    end
  end

  # The btrfs volumes behind workspaces and environments: snapshot history, restore targets, and
  # the deletes — over /v1/volumes.
  class Volumes
    include Names

    def initialize(client)
      @c = client
    end

    # +kind+ is "workspace" or "environment"; +owner+ a handle or team slug.
    def list(kind: nil, owner: nil)
      @c.request("GET", "/v1/volumes", query: { kind: kind, owner: owner })
    end

    # The chain of PUSHED snapshots, newest first — each row a restore target.
    def history(volume:)
      @c.request("GET", "/v1/volumes/#{seg(volume)}/history")
    end

    def refs(volume:)
      @c.request("GET", "/v1/volumes/#{seg(volume)}/refs")
    end

    # Refused (409) for a sync point or a running worktree's base; deleting a detached volume's
    # last snapshot deletes the volume.
    def delete_snapshot(volume:, snapshot:)
      @c.request("DELETE", "/v1/volumes/#{seg(volume)}/snapshots/#{seg(snapshot)}")
    end

    # Takes a DETACHED volume with all its snapshots; refused while a working copy remains.
    def delete_by_id(volume:)
      @c.request("DELETE", "/v1/volumes/#{seg(volume)}")
    end

    # One named volume; +owner+ is your handle or a team slug.
    def get(owner:, name:)
      @c.request("GET", named(owner, name))
    end

    # The versions of a named volume, newest first.
    def versions(owner:, name:)
      @c.request("GET", named(owner, name, "/versions"))
    end

    # Delete a named volume and every snapshot on it (204); the name is free again.
    def delete(owner:, name:)
      @c.request("DELETE", named(owner, name))
    end

    # Delete version +n+ (204); deleting the last one deletes the volume.
    def delete_version(owner:, name:, n:)
      @c.request("DELETE", named(owner, name, "/versions/#{Integer(n)}"))
    end

    private

    def named(owner, name, tail = "")
      "/v1/volumes/#{seg(owner)}/#{seg(name)}#{tail}"
    end
  end

  # Throwaway, isolated machines that run ONE command, over /v1/sandboxes. A sandbox starts from
  # an image, from one of your workspaces (+from_workspace+) or from a pushed snapshot
  # (+from_snapshot+), runs +cmd+, and ends when it exits or after +timeout_s+ (server default
  # 300). Every call answers a Sandbox handle.
  #
  #   sb = kl.sandboxes.create(cmd: ["python3", "-c", "print(6 * 7)"], packages: ["python3"])
  #   code = sb.wait
  #   puts code, sb.logs
  class Sandboxes
    include Names

    # The POST /v1/sandboxes body; refuses a missing +cmd+ before anything is sent. Shared with
    # Sessions#create_sandbox — not public surface.
    def self.body(cmd:, image: nil, packages: nil, from_workspace: nil, from_snapshot: nil, team: nil,
                  tty: nil, stdin: nil, timeout_s: nil, network: nil, env: nil, cpu: nil,
                  memory_gb: nil, disk_gb: nil)
      unless cmd.is_a?(Array) && !cmd.empty? && cmd.all?(String)
        raise ArgumentError, "cmd is required: the program and its arguments as a non-empty list of strings"
      end

      { cmd: cmd, image: image, packages: packages, from_workspace: from_workspace,
        from_snapshot: from_snapshot, team: team, tty: tty, stdin: stdin, timeout_secs: timeout_s,
        network: network, env: env, cpu: cpu, memory_gb: memory_gb, disk_gb: disk_gb }.compact
    end

    def initialize(client)
      @c = client
    end

    # Ask for a sandbox and return at once (202); it is usually still "pending". +cmd+ (required)
    # is the program then its arguments. +tty+ runs it on a pty; +stdin+ keeps input open for
    # attach. +timeout_s+ is 10..=86400. +from_workspace+ is an id, "name" or "team/name";
    # +from_snapshot+ a volume name, "name@N" or snapshot id (exclusive with it). +network+ is
    # "internet" or "none"; +cpu+ 1..=8, +memory_gb+ 1..=32. A session's sandbox is made with
    # Sessions#create_sandbox. Unset keywords are omitted, so the server's defaults apply.
    def create(cmd:, **options)
      Sandbox.new(@c, @c.request("POST", "/v1/sandboxes", body: Sandboxes.body(cmd: cmd, **options)))
    end

    # One sandbox by id (a name does not resolve).
    def get(sandbox:)
      Sandbox.new(@c, @c.request("GET", "/v1/sandboxes/#{seg(sandbox)}"))
    end

    # ONE page of the caller's sandboxes, newest first (+limit+ 1..=200, default 50); +team+
    # scopes to that team. +list_page+ also answers the next cursor.
    def list(team: nil, limit: nil, cursor: nil)
      list_page(team: team, limit: limit, cursor: cursor).first
    end

    # [sandboxes, next_cursor]: a page, and the X-Next-Cursor to pass as +cursor+ for the next
    # one (nil on the last page).
    def list_page(team: nil, limit: nil, cursor: nil)
      resp = @c.__send__(:perform, "GET", "/v1/sandboxes", nil,
                         { team: team, limit: limit, cursor: cursor }, "application/json", 30)
      docs = resp.body.to_s.empty? ? [] : JSON.parse(resp.body)
      [(docs || []).map { |d| Sandbox.new(@c, d) }, resp["X-Next-Cursor"]]
    end

    # One command in a fresh sandbox: create, wait for it to exit, read the output, and
    # terminate — ALWAYS, also when anything before it raised. Answers
    # {"exit_code", "stdout", "stderr", "sandbox"}: the sandbox keeps one stream, so "stdout"
    # holds the output and error output together and "stderr" is "". +timeout_s+ bounds the wait
    # (TimeoutError; nil waits as long as it takes) and, when given, sizes the sandbox's own
    # lifetime to cover it plus a minute. +options+ are the other keywords of +create+.
    def run(cmd:, timeout_s: nil, **options)
      options[:timeout_s] = (timeout_s.ceil + 60).clamp(10, 86_400) if timeout_s && !options.key?(:timeout_s)
      sb = create(cmd: cmd, **options)
      begin
        code = sb.wait(timeout_s: timeout_s)
        out = sb.logs
      rescue Exception # rubocop:disable Lint/RescueException -- terminate on any failure, then re-raise it
        begin
          sb.terminate
        rescue StandardError
          # never mask the failure that got us here
        end
        raise
      end
      sb.terminate
      { "exit_code" => code, "stdout" => out, "stderr" => "", "sandbox" => sb.id }
    end
  end

  # A handle on one sandbox: id, phase ("pending", "starting", "running", "terminated"), reason
  # once terminated, exit_code, and the whole server doc as of the last answer — +refresh+
  # re-reads it. Fields read as sb["phase"] or through the named readers.
  class Sandbox
    include Names

    attr_reader :doc

    def initialize(client, doc)
      @c = client
      @doc = doc || {}
    end

    def id = @doc["id"]
    def phase = @doc["phase"]
    def reason = @doc["reason"]
    def exit_code = @doc["exit_code"]
    def message = @doc["message"]

    def [](field)
      @doc[field.to_s]
    end

    # Re-read the doc; returns self.
    def refresh
      @doc = @c.request("GET", base)
      self
    end

    # Poll (0.5 s backing off to 2 s) until running. A sandbox that terminates first raises
    # SandboxTerminated naming the reason; +timeout_s+ passing raises TimeoutError.
    def wait_running(timeout_s: 120)
      poll_until(timeout_s, "running") do
        raise SandboxTerminated.new(id, reason, message) if phase == "terminated"
        phase == "running"
      end
    end

    # The main process's output (stdout and stderr together) as text: the live output while it
    # runs, the last 256 KiB after. +follow+ holds until the process ends. 409 while the sandbox
    # has no output yet (still starting).
    def logs(follow: false)
      @c.__send__(:perform, "GET", "#{base}/logs", nil, { follow: follow ? "true" : nil },
                  "text/plain", follow ? nil : 30).body.to_s.dup.force_encoding(Encoding::UTF_8).scrub
    end

    # Not in this SDK: GET /v1/sandboxes/{id}/attach is a WebSocket and this client uses only
    # the standard library.
    def attach(cols: nil, rows: nil)
      raise NotImplementedError, "sandbox attach is a WebSocket and is not in the Ruby SDK"
    end

    # Stop it for good (202, or 200 if already ended); the record stays until +delete+. Returns
    # self with the answered doc.
    def terminate
      @doc = @c.request("POST", "#{base}/terminate") || @doc
      self
    end

    # Block until it has ended and answer its exit code (nil when it ended some other way than
    # the command exiting; +reason+ says how). Waits in steps of at most 20 s, the most the api
    # holds one open; TimeoutError past +timeout_s+.
    def wait(timeout_s: nil)
      deadline = timeout_s && (Sandbox.now + timeout_s)
      loop do
        left_ms = deadline ? ((deadline - Sandbox.now) * 1000).to_i : 20_000
        raise TimeoutError, "sandbox #{id} still #{phase} after #{timeout_s} s" if left_ms <= 0

        out = @c.request("POST", "#{base}/wait", body: { timeout_ms: [left_ms, 20_000].min }) || {}
        @doc = @doc.merge(out.slice("phase", "reason", "exit_code", "message"))
        return exit_code if phase == "terminated"
      end
    end

    # Delete the record (204); a sandbox still running dies with it.
    def delete
      @c.request("DELETE", base)
      nil
    end

    def inspect
      "#<Kloudlite::Sandbox #{id} #{phase}#{reason ? " #{reason}" : ''}>"
    end

    private

    def base
      "/v1/sandboxes/#{seg(id)}"
    end

    def poll_until(timeout_s, what)
      deadline = timeout_s && (Sandbox.now + timeout_s)
      delay = 0.5
      loop do
        refresh
        return self if yield
        left = deadline && (deadline - Sandbox.now)
        raise TimeoutError, "sandbox #{id} was not #{what} within #{timeout_s}s (phase #{phase})" if left && left <= 0
        sleep(left ? [delay, left].min : delay)
        delay = [delay * 2, 2.0].min
      end
    end

    # ::Process — a bare name would be ambiguous with nothing here, but keep it explicit.
    def self.now
      ::Process.clock_gettime(::Process::CLOCK_MONOTONIC)
    end
  end

  # Pipelines, over /v1/pipelines: a JSON definition stored as numbered versions; a run executes
  # one version, each job in its own sandbox. +run+, +runs+ and +get_run+ answer Run handles.
  #
  #   kl.pipelines.create(owner: "acme", definition: definition)
  #   run = kl.pipelines.run(owner: "acme", pipeline: "test", inputs: { "target" => "staging" })
  #   run.follow(job: "test") { |chunk| print chunk }
  #   run.wait   # => "success"
  class Pipelines
    include Names

    def initialize(client)
      @c = client
    end

    # Save +definition+ (its "name" is the pipeline's) as version 1. A webhook trigger's URL is
    # in the answer's "webhookUrl", this once. 409 if the name exists.
    def create(owner:, definition:)
      @c.request("POST", "/v1/pipelines/#{seg(owner)}", body: definition)
    end

    # Replace the definition: a changed one is the next version, an equal one saves nothing. To
    # roll back, send an old definition. A concurrent save is an ApiError 409 ("changed; retry").
    def update(owner:, name:, definition:)
      @c.request("PUT", "/v1/pipelines/#{seg(owner)}/#{seg(name)}", body: definition)
    end

    # +owner+'s pipelines by name: "name", "latestVersion", "updatedAtMs", "triggers".
    def list(owner:)
      @c.request("GET", "/v1/pipelines/#{seg(owner)}")
    end

    # The pipeline document of +version+ (default the latest), "definition" included.
    def get(owner:, name:, version: nil)
      @c.request("GET", "/v1/pipelines/#{seg(owner)}/#{seg(name)}", query: { version: version })
    end

    # The versions, oldest first: "version", "createdBy", "createdAtMs", "definitionSha256".
    def versions(owner:, name:)
      @c.request("GET", "/v1/pipelines/#{seg(owner)}/#{seg(name)}/versions")
    end

    # Delete the pipeline and its versions; finished runs stay. 409 while a run is queued or running.
    def delete(owner:, name:)
      @c.request("DELETE", "/v1/pipelines/#{seg(owner)}/#{seg(name)}")
      nil
    end

    # Delete one version; 409 for the latest or one a queued or running run uses.
    def delete_version(owner:, name:, version:)
      @c.request("DELETE", "/v1/pipelines/#{seg(owner)}/#{seg(name)}/versions/#{Integer(version)}")
      nil
    end

    # A new webhook secret; the old URL stops at once. {"webhookUrl"}, shown only here.
    def rotate_webhook(owner:, name:)
      @c.request("POST", "/v1/pipelines/#{seg(owner)}/#{seg(name)}/webhook/rotate")
    end

    # Start +pipeline+ by hand and answer the Run at once (202), queued. +version+ defaults to the
    # newest; +inputs+ are string values of the inputs it declares. +environment+ runs every job
    # that names none in that environment; +clone+ instead in a per-run copy of it — one or the
    # other. A version that cannot run is an ApiError 422 whose message lists every reason, and
    # nothing is recorded.
    def run(owner:, pipeline:, version: nil, inputs: nil, environment: nil, clone: nil)
      raise ArgumentError, "pass environment: or clone:, not both" if environment && clone

      body = { version: version, inputs: inputs,
               environment: clone ? { clone: clone } : environment }.compact
      Run.new(@c, @c.request("POST", "/v1/pipelines/#{seg(owner)}/#{seg(pipeline)}/runs", body: body))
    end

    # The pipeline's runs, newest first (+limit+ default 50, at most 200).
    def runs(owner:, pipeline:, limit: nil)
      docs = @c.request("GET", "/v1/pipelines/#{seg(owner)}/#{seg(pipeline)}/runs", query: { limit: limit }) || []
      docs.map { |doc| Run.new(@c, doc) }
    end

    # One run by its id ("1760400000000-0a1b2c").
    def get_run(owner:, pipeline:, run:)
      Run.new(@c, @c.request("GET", "/v1/pipelines/#{seg(owner)}/#{seg(pipeline)}/runs/#{seg(run)}"))
    end
  end

  # One pipeline run: the doc the server last answered (camelCase, as the wire spells it), plus
  # the verbs. Fields read as run["state"] or through the named readers; +state+ is queued,
  # running, success, failure or cancelled. Each of +jobs+ is a Hash with "key", "job", "matrix",
  # "needs", "state" (pending, running, success, failure, skipped, cancelled), "sandbox",
  # "startedAtMs", "finishedAtMs", "exitCode", "failedStep" and "detail"; +logs+ and +follow+
  # name one by its index or its "key".
  class Run
    include Names

    TERMINAL = %w[success failure cancelled].freeze

    attr_reader :doc

    def initialize(client, doc)
      @c = client
      @doc = doc || {}
    end

    def id = @doc["id"]
    def number = @doc["number"]
    def pipeline = @doc["pipeline"]
    def owner = @doc["owner"]
    def pipeline_version = @doc["pipelineVersion"]
    def definition_sha256 = @doc["definitionSha256"]
    def event = @doc["event"]
    def inputs = @doc["inputs"] || {}
    def environment = @doc["environment"]
    def requested_by = @doc["requestedBy"]
    def requested_at_ms = @doc["requestedAtMs"]
    def state = @doc["state"]
    def cancel_requested? = @doc["cancelRequested"] == true
    def started_at_ms = @doc["startedAtMs"]
    def finished_at_ms = @doc["finishedAtMs"]
    def detail = @doc["detail"]
    def jobs = @doc["jobs"] || []

    def [](field)
      @doc[field.to_s]
    end

    # Re-read the doc; returns self.
    def refresh
      @doc = @c.request("GET", base)
      self
    end

    # Ask it to stop: a queued run is cancelled at once, a running one once its worker has ended
    # its sandboxes — +wait+ sees it reach cancelled. Returns self with the answered doc.
    def cancel
      @doc = @c.request("POST", "#{base}/cancel") || @doc
      self
    end

    # Poll every +poll_s+ until the run has ended and answer its final state; +timeout_s+ nil
    # waits as long as it takes, else TimeoutError once it passes.
    def wait(timeout_s: nil, poll_s: 5)
      deadline = timeout_s && (Run.now + timeout_s)
      until TERMINAL.include?(state)
        refresh
        break if TERMINAL.include?(state)

        left = deadline && (deadline - Run.now)
        raise TimeoutError, "run #{id} still #{state} after #{timeout_s}s" if left && left <= 0

        sleep(left ? [poll_s, left].min : poll_s)
      end
      state
    end

    # The job's whole log so far (combined stdout and stderr), every page of it. +job+ is its
    # index in +jobs+ or its "key" ("test (postgres16)").
    def logs(job:)
      index = job_index(job)
      offset = 0
      text = +""
      loop do
        page = log_page(index, offset)
        text << page["data"].to_s
        nxt = page["next"].to_i
        return text if page["complete"] || nxt <= offset

        offset = nxt
      end
    end

    # Yield the job's log in chunks as it is written — from the first byte, polling every
    # +poll_s+ while nothing new has landed — until the job has ended and every byte was
    # yielded. No block answers an Enumerator.
    def follow(job:, poll_s: 5)
      return enum_for(:follow, job: job, poll_s: poll_s) unless block_given?

      index = job_index(job)
      offset = 0
      loop do
        page = log_page(index, offset)
        data = page["data"].to_s
        yield data unless data.empty?
        return self if page["complete"]

        nxt = page["next"].to_i
        sleep(poll_s) if nxt <= offset
        offset = [offset, nxt].max
      end
    end

    def inspect
      "#<Kloudlite::Run #{owner}/#{pipeline}##{number} #{state}>"
    end

    def self.now
      ::Process.clock_gettime(::Process::CLOCK_MONOTONIC)
    end

    private

    def base
      "/v1/pipelines/#{seg(owner)}/#{seg(pipeline)}/runs/#{seg(id)}"
    end

    # {"data", "next", "complete", "state"}: at most a page of the log from byte +offset+.
    def log_page(index, offset)
      @c.request("GET", "#{base}/jobs/#{index}/logs", query: { offset: offset })
    end

    def job_index(job)
      keys = jobs.map { |j| j["key"] }
      if job.is_a?(Integer)
        raise ArgumentError, "run #{id} has #{keys.size} jobs, no job #{job}" unless job >= 0 && job < keys.size

        return job
      end
      keys.index(job.to_s) or raise ArgumentError, "run #{id} has no job #{job.inspect}; its jobs are #{keys.inspect}"
    end
  end
end
