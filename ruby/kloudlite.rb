# Kloudlite SDK for Ruby — stdlib only (net/http + json).
#
# Auth is a Bearer token (a session or CLI token). Every module hangs off the
# one client, and every repository is named by ONE "owner/name" string — the
# owner half being a person's handle or a team slug:
#
#   require_relative "kloudlite"
#   kl = Kloudlite::Client.new(base_url: "https://api.example.com", token: "…")
#   repo = kl.code_repos.create(repo: "acme/payments")
#
# Every public method takes KEYWORD arguments, so no caller has to remember an order; a
# resource is always the keyword named after it (repo:, workspace:, environment:, session:,
# volume:, snapshot:, sandbox:, run:), and a new object made from an existing one is to:.

require "json"
require "net/http"
require "timeout"
require "uri"

module Kloudlite
  VERSION = "0.1.0"
  USER_AGENT = "kloudlite-sdk-ruby/#{VERSION}".freeze

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

    # "owner/name" → its two halves, refused client-side when malformed.
    def split(repo)
      owner, name = repo.to_s.split("/", 2)
      if owner.nil? || owner.empty? || name.nil? || name.empty? || name.include?("/")
        raise ArgumentError, "a repository is named owner/name, got #{repo.inspect}"
      end
      [owner, name]
    end

    def seg(s)
      URI.encode_uri_component(s.to_s)
    end
  end

  class Client
    attr_reader :code_repos, :model_repos, :data_repos, :workspaces, :environments, :sessions, :volumes,
                :secrets, :sandboxes, :pipelines

    def initialize(base_url:, token: nil, hub_url: nil, timeout: 30)
      @base_url = base_url.sub(%r{/+\z}, "")
      @hub_url = hub_url&.sub(%r{/+\z}, "")
      @token = token
      @timeout = timeout
      @code_repos = CodeRepos.new(self)
      @model_repos = HubRepos.new(self, "model")
      @data_repos = HubRepos.new(self, "dataset")
      @workspaces = Workspaces.new(self)
      @environments = Environments.new(self)
      @sessions = Sessions.new(self)
      @volumes = Volumes.new(self)
      @secrets = Secrets.new(self)
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
      send_to(@base_url, method, path, body: body, query: query)
    end

    # The same round trip against the hub listener (models and datasets).
    def hub_request(method, path, body: nil, query: nil)
      raise ArgumentError, "this client has no hub_url; pass one to reach models and datasets" unless @hub_url
      send_to(@hub_url, method, path, body: body, query: query)
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

    def send_to(base, method, path, body: nil, query: nil)
      uri, req = build(base, method, path, body: body, query: query)
      resp = Net::HTTP.start(uri.hostname, uri.port,
                             use_ssl: uri.scheme == "https",
                             open_timeout: @timeout, read_timeout: @timeout) { |http| http.request(req) }
      raise ApiError.new(resp.code.to_i, self.class.error_text(resp.body)) unless resp.is_a?(Net::HTTPSuccess)
      resp.body.nil? || resp.body.empty? ? nil : JSON.parse(resp.body)
    end

    def build(base, method, path, body: nil, query: nil)
      uri = URI.parse(base + path)
      if query
        q = query.compact
        uri.query = URI.encode_www_form(q) unless q.empty?
      end
      req = Net::HTTP.const_get(method.capitalize).new(uri)
      req["User-Agent"] = USER_AGENT
      req["Accept"] = "application/json"
      req["Authorization"] = "Bearer #{@token}" if @token
      if body
        req["Content-Type"] = "application/json"
        req.body = JSON.generate(body)
      end
      [uri, req]
    end

    # GET an NDJSON stream, yielding each line parsed. No read timeout: a process may be silent
    # for longer than any bound we could pick, and the server ends the body after the exit frame.
    # Chunks split lines anywhere, so bytes are buffered until a whole line is in hand. Reached
    # by Sandbox and SandboxProcess through __send__ — it is not part of the public surface.
    def stream_ndjson(path, query: nil)
      uri, req = build(@base_url, "GET", path, query: query)
      req["Accept"] = "application/x-ndjson"
      Net::HTTP.start(uri.hostname, uri.port,
                      use_ssl: uri.scheme == "https",
                      open_timeout: @timeout, read_timeout: nil) do |http|
        http.request(req) do |resp|
          raise ApiError.new(resp.code.to_i, self.class.error_text(resp.read_body)) unless resp.is_a?(Net::HTTPSuccess)
          buf = String.new(encoding: Encoding::BINARY)
          resp.read_body do |chunk|
            buf << chunk.b
            while (i = buf.index("\n"))
              line = buf.slice!(0, i + 1).strip
              yield JSON.parse(line.force_encoding(Encoding::UTF_8)) unless line.empty?
            end
          end
          tail = buf.strip
          yield JSON.parse(tail.force_encoding(Encoding::UTF_8)) unless tail.empty?
        end
      end
    end
  end

  # Code repositories: /v1/repos. Every +repo:+ is one "owner/name" string; only owner-scoped
  # calls (+list+) take a bare handle.
  class CodeRepos
    include Names

    def initialize(client)
      @c = client
    end

    # repos
    def create(repo:, visibility: "private", description: "")
      owner, name = split(repo)
      @c.request("POST", "/v1/repos",
                 body: { owner: owner, name: name, visibility: visibility, description: description })
    end

    def list(owner:)
      @c.request("GET", "/v1/repos", query: { owner: owner })
    end

    def get(repo:)
      @c.request("GET", path(repo))
    end

    def update(repo:, description: nil, visibility: nil)
      @c.request("PATCH", path(repo), body: { description: description, visibility: visibility })
    end

    def delete(repo:)
      @c.request("DELETE", path(repo))
    end

    # A new repo +to+ copied from +repo+: objects and refs, never its pull requests or
    # settings. Private by default, whatever the source was.
    def fork(repo:, to:, visibility: "private", description: "")
      owner, name = split(to)
      @c.request("POST", path(repo, "/fork"),
                 body: { owner: owner, name: name, visibility: visibility, description: description })
    end

    # A new repo whose contents are fetched from a PUBLIC git remote — +url+ is a strict
    # "https://" URL (GitHub is one host among any; the wire field is +git+). Answers 202: the
    # repo exists but the fetch runs in the background; poll +import_status+.
    def import_git(repo:, url:, visibility: "private", description: "")
      owner, name = split(repo)
      @c.request("POST", "/v1/repos/import",
                 body: { owner: owner, name: name, git: url,
                         visibility: visibility, description: description })
    end

    # {"from", "state" (queued|running|done|failed), "detail"?} for an imported repo.
    def import_status(repo:)
      @c.request("GET", path(repo, "/import"))
    end

    # branches
    def branches(repo:)
      @c.request("GET", path(repo, "/branches"))
    end

    # +oid+ is the head the caller saw; the delete is refused if the branch moved.
    def delete_branch(repo:, branch:, oid:)
      @c.request("DELETE", path(repo, "/branches/#{seg(branch)}"), query: { oid: oid })
    end

    # protection
    def protection(repo:)
      @c.request("GET", path(repo, "/protection"))
    end

    def protect(repo:, pattern:, remove: false, no_force: true, no_delete: true)
      @c.request("POST", path(repo, "/protection"),
                 body: { pattern: pattern, remove: remove, no_force: no_force, no_delete: no_delete })
    end

    # pull requests
    def pulls(repo:, state: nil, limit: nil)
      @c.request("GET", path(repo, "/pulls"), query: { state: state, limit: limit })
    end

    def pull(repo:, number:)
      @c.request("GET", path(repo, "/pulls/#{number.to_i}"))
    end

    # +head_repo+ (another "owner/name") makes it a cross-repo pull: +head+ then
    # names a branch of that fork, proposed onto this repo's +base+.
    def open_pull(repo:, title:, head:, base:, description: "", head_repo: nil)
      split(head_repo) unless head_repo.nil?
      @c.request("POST", path(repo, "/pulls"),
                 body: { title: title, head: head, base: base, description: description,
                         head_repo: head_repo })
    end

    def comment(repo:, number:, body:)
      @c.request("POST", path(repo, "/pulls/#{number.to_i}/comments"), body: { body: body })
    end

    # strategy: fast-forward, squash, merge or rebase. Answers 202 — the merge is a job.
    def merge(repo:, number:, strategy: "fast-forward")
      @c.request("POST", path(repo, "/pulls/#{number.to_i}/merge"), query: { strategy: strategy })
    end

    def close(repo:, number:)
      @c.request("POST", path(repo, "/pulls/#{number.to_i}/close"))
    end

    # commits
    def compare(repo:, base:, head:)
      @c.request("GET", path(repo, "/compare"), query: { base: base, head: head })
    end

    # One commit of file changes; each change is {path:, contentBase64:, executable:, delete:}.
    # +expect+ refuses the commit if the branch moved.
    def commit(repo:, branch:, message:, changes:, expect: nil, new_branch: nil)
      @c.request("POST", path(repo, "/commits"),
                 body: { branch: branch, message: message, changes: changes,
                         expect: expect, newBranch: new_branch })
    end

    def signature(repo:, sha:)
      @c.request("GET", path(repo, "/commits/#{seg(sha)}/signature"))
    end

    private

    def path(repo, tail = "")
      owner, name = split(repo)
      "/v1/repos/#{seg(owner)}/#{seg(name)}#{tail}"
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

    # +workspace+ ("name" or "team/name") is split into the body's name and team. +repo+ seeds
    # from a PLATFORM repo ("owner/name", with +branch+); +packages+ are nixpkgs attributes,
    # optionally pinned ("attr@version"). +ttl_secs+ (600..=2592000) deletes it after that
    # long unused; absent, never. +env+ maps variables to literals or "${{ secrets.NAME }}" references.
    def create(workspace:, quota_gb:, image: nil, repo: nil, branch: nil, packages: [], ttl_secs: nil,
               env: nil)
      team, name = owned(workspace)
      body = { name: name, quota_gb: quota_gb,
               team: team, repo: repo, branch: branch, packages: packages, ttl_secs: ttl_secs }
      body[:image] = image unless image.nil?
      body[:env] = env unless env.nil?
      @c.request("POST", "/v1/workspaces", body: body)
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

    # A stop cuts a sync point and tears the pod down — seconds, never waiting on a replica.
    def stop(workspace:)
      at("POST", workspace, "/stop")
    end

    # Cut a SNAPSHOT of the working copy — the one durable, restorable state.
    def push(workspace:, message: nil)
      at("POST", workspace, "/push", body: message.nil? ? nil : { message: message })
    end

    # A new workspace +to+ from this one's state, cut at the moment of the request; the
    # answer's +based_on+ names exactly what it grafted onto.
    def clone(workspace:, to:, task: nil)
      at("POST", workspace, "/clone", body: { name: to, task: task })
    end

    # A new workspace +to+ grafted onto a PUSHED +snapshot+ — also one whose workspace is long
    # gone. Absent overrides mean "whatever the snapshot froze".
    def restore(snapshot:, to:, image: nil, packages: nil, quota_gb: nil)
      @c.request("POST", "/v1/workspaces/restore",
                 body: { name: to, snapshot_id: snapshot,
                         image: image, packages: packages, quota_gb: quota_gb })
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

    # Replace the whole env (literals or "${{ secrets.NAME }}" references); takes effect on next start.
    def set_env(workspace:, env:)
      at("PUT", workspace, "/env", body: { env: env })
    end

    # Re-resolve every pinned package — the only call that moves existing locks.
    def update_packages(workspace:)
      at("POST", workspace, "/packages/update")
    end

    # The workspace's tool-server address and token — answered to the owner alone.
    def tools(workspace:)
      at("GET", workspace, "/tools")
    end

    # An Ide bound to this workspace's tool server. The address is a pod IP: reachable from
    # inside the fleet (a bench); from a laptop, tunnel with `kl-connect ws ide` and use
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
      http("POST", "/tools/#{URI.encode_uri_component(tool)}", body)
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
    def create(environment:, services: [], quota_gb: nil, snapshot: nil, ttl_secs: nil)
      owner, name = owned(environment)
      @c.request("POST", "/v1/environments",
                 body: { name: name, owner: owner, services: fill(services),
                         quota_gb: quota_gb, snapshot_id: snapshot, ttl_secs: ttl_secs })
    end

    # The caller's environments and their teams'; +owner+ narrows to one handle or team.
    def list(owner: nil)
      @c.request("GET", "/v1/environments", query: { owner: owner })
    end

    def get(environment:)
      at("GET", environment)
    end

    # Replace the WHOLE service list. A removed service keeps its folders, so re-adding finds its data.
    def set_services(environment:, services:)
      at("PATCH", environment, body: { services: fill(services) })
    end

    def start(environment:)
      at("POST", environment, "/start")
    end

    def stop(environment:)
      at("POST", environment, "/stop")
    end

    # Seconds the environment may sit unused (600..=2592000) before it is deleted; nil clears.
    def set_ttl(environment:, ttl_secs:)
      at("PUT", environment, "/ttl", body: { ttl_secs: ttl_secs })
    end

    # The volume survives while a pushed snapshot references it.
    def delete(environment:)
      at("DELETE", environment)
    end

    # Cut a SNAPSHOT of every service's data — the one durable, restorable state.
    def push(environment:, message: nil)
      at("POST", environment, "/push", body: message.nil? ? nil : { message: message })
    end

    # A new environment +to+ ("name" or "team/name"): a copy of the live disk, or with
    # +snapshot+ of one of this environment's snapshots.
    def clone(environment:, to:, snapshot: nil, services: nil, quota_gb: nil)
      owner, name = owned(to)
      at("POST", environment, "/clone",
         body: { name: name, owner: owner, snapshot_id: snapshot,
                 services: services && fill(services), quota_gb: quota_gb })
    end

    # Put THIS environment back to one of its own snapshots, services and data (202). A
    # persistent service the snapshot lacks is a 409 naming it; list it in +discard+ to accept
    # losing its data.
    def restore(environment:, snapshot:, discard: [])
      at("POST", environment, "/restore", body: { snapshot_id: snapshot, discard: discard })
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

    # A workspace is in at most one session; while in one it resolves that session's environment.
    def add(session:, workspace:)
      at("POST", session, "/workspaces", body: { workspace: workspace })
    end

    # Releases whatever the workspace intercepts in this session first.
    def remove(session:, workspace:)
      at("DELETE", session, "/workspaces/#{seg(workspace)}")
    end

    # Send everything that calls +service+ in the environment to +workspace+ (a member), +ports+
    # mapping each service port to the workspace port that answers it ({ 8080 => 3000 }).
    def intercept(session:, service:, workspace:, ports: {})
      at("POST", session, "/intercepts",
         body: { service: service, workspace: workspace,
                 ports: ports.map { |s, w| { service: Integer(s), workspace: Integer(w) } } })
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
  class Secrets
    include Names

    def initialize(client)
      @c = client
    end

    def set(owner:, name:, value:)
      @c.request("PUT", "/v1/secrets/#{seg(owner)}/#{seg(name)}", body: { value: value })
      nil
    end

    def list(owner:)
      @c.request("GET", "/v1/secrets/#{seg(owner)}")
    end

    # 409 while a workspace or environment still references it; the body names them.
    def delete(owner:, name:)
      @c.request("DELETE", "/v1/secrets/#{seg(owner)}/#{seg(name)}")
      nil
    end
  end

  # The btrfs volumes behind workspaces and environments: snapshot history, restore targets, and
  # the two deletes — over /v1/volumes.
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
    def delete(volume:)
      @c.request("DELETE", "/v1/volumes/#{seg(volume)}")
    end
  end

  # Short-lived isolated containers you send commands to, over /v1/sandboxes — Modal-shaped:
  # create, exec as often as you like, terminate. Not a workspace: no ssh, no sync, no wake;
  # when it ends only its record (phase, reason, timings) stays. Read files out before
  # terminating — the disk goes with it.
  #
  #   sb = kl.sandboxes.create(image: "python:3.12", timeout_s: 600)
  #   p = sb.exec(cmd: ["bash", "-c", "while read line; do echo $line; done"])
  #   p.stdin.write("foo bar\n"); p.stdin.write_eof; p.stdin.drain
  #   p.wait            # => 0
  #   p.stdout.read     # => "foo bar\n"
  #   sb.terminate
  class Sandboxes
    include Names

    def initialize(client)
      @c = client
    end

    # Returns at once, before the sandbox runs (Sandbox#exec waits for that). +from_workspace+
    # ("name", "team/name" or an id) starts from a copy of that workspace's disk, +from_snapshot+
    # from a pushed snapshot; +network+ is "internet" (default) or "none"; +cpu+ cores and the
    # *_gb sizes are integers. +session+ (an id, "name" or "team/name" of your session in the
    # sandbox's team) makes it a member from birth -- the only way it reaches an environment, whose
    # services then resolve by bare name inside it. Unset keywords are omitted, so the server's
    # defaults apply.
    def create(image: nil, packages: nil, from_workspace: nil, from_snapshot: nil, team: nil,
               timeout_s: nil, idle_timeout_s: nil, network: nil, env: nil, cpu: nil, memory_gb: nil,
               disk_gb: nil, session: nil)
      body = { team: team, image: image, packages: packages,
               from_workspace: from_workspace, from_snapshot: from_snapshot,
               timeout_secs: timeout_s, idle_timeout_secs: idle_timeout_s, network: network,
               env: env, cpu: cpu, memory_gb: memory_gb, disk_gb: disk_gb,
               session: session }.compact
      Sandbox.new(@c, @c.request("POST", "/v1/sandboxes", body: body))
    end

    def get(sandbox:)
      Sandbox.new(@c, @c.request("GET", "/v1/sandboxes/#{seg(sandbox)}"))
    end

    # The caller's sandboxes, newest first; +team+ scopes to theirs in that team.
    def list(team: nil)
      (@c.request("GET", "/v1/sandboxes", query: { team: team }) || []).map { |doc| Sandbox.new(@c, doc) }
    end

    # One command in a fresh sandbox: create, wait until running, exec, wait, read all output,
    # and ALWAYS terminate. +env+ and +timeout_s+ are the COMMAND's (timeout_s bounds the wait
    # for it to exit; nil waits until it does); every other keyword is create's. Answers
    # {"exit_code", "stdout", "stderr", "sandbox"}.
    def run(cmd:, cwd: nil, env: nil, timeout_s: nil, **create_args)
      # A command allowed longer than the sandbox's default lifetime must not be cut by it: the
      # sandbox outlives the wait by a minute, within the server's 10 s..24 h.
      create_args = create_args.merge(timeout_s: (timeout_s.ceil + 60).clamp(10, 86_400)) if timeout_s
      sb = create(**create_args)
      done = false
      begin
        p = sb.exec(cmd: cmd, cwd: cwd, env: env)
        code = p.wait(timeout_s: timeout_s)
        out, err, = p.__send__(:collect)
        done = true
        { "exit_code" => code, "stdout" => out, "stderr" => err, "sandbox" => sb.id }
      ensure
        begin
          sb.terminate
        rescue ApiError, SystemCallError, IOError
          # The command's own failure is the one worth reporting; a terminate that fails
          # after success is not swallowed.
          raise if done
        end
      end
    end
  end

  # One sandbox: the doc the server last answered, plus the verbs. Fields read as sb["phase"]
  # or through the named readers.
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

    # Start +cmd+ — an argv Array, or a String run by the shell — and answer its SandboxProcess.
    # Always detached: the process outlives this call, and its output and exit are read back.
    def exec(cmd:, cwd: nil, env: nil)
      wait_running unless phase == "running"
      out = call(tool: "exec", cmd: cmd, cwd: cwd, env: env, detach: true)
      SandboxProcess.new(@c, self, out["id"].to_s)
    end

    # Paths are relative to ~/workspace; both answer the tool's JSON as is.
    def read(path:)
      call(tool: "read", path: path)
    end

    def write(path:, content:)
      call(tool: "write", path: path, content: content)
    end

    # Any tool the sandbox's tool server lists, arguments as keywords (nil ones dropped). The
    # sandbox must be running — anything else is a 409.
    def call(tool:, **args)
      @c.request("POST", "#{base}/tools/#{seg(tool)}", body: args.compact.transform_keys(&:to_s))
    end

    # Ask it to end (202); the record stays. Returns self with the answered doc.
    def terminate
      @doc = @c.request("POST", "#{base}/terminate") || @doc
      self
    end

    # Poll until terminated and answer the reason; +timeout_s+ nil waits as long as it takes.
    def wait(timeout_s: nil)
      poll_until(timeout_s, "terminated") { phase == "terminated" }
      reason
    end

    # Delete the record (and, if it still runs, the sandbox with it).
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

    # ::Process — this module's own SandboxProcess is the one a bare name would suggest.
    def self.now
      ::Process.clock_gettime(::Process::CLOCK_MONOTONIC)
    end
  end

  # A process started by Sandbox#exec. +stdin+ takes bytes, +stdout+/+stderr+ read the output
  # (whole, or line by line as it arrives), +wait+ answers the exit code.
  class SandboxProcess
    include Names

    attr_reader :id, :sandbox, :stdin, :stdout, :stderr

    # The longest single process_wait the api will hold open (it clamps to this).
    WAIT_SLICE_MS = 20_000

    def initialize(client, sandbox, id)
      @c = client
      @sandbox = sandbox
      @id = id
      @returncode = nil
      @stdin = Stdin.new(self)
      @stdout = Output.new(self, "stdout")
      @stderr = Output.new(self, "stderr")
    end

    # The exit code last seen (by wait, poll or the end of an output stream); nil until then,
    # and nil for a process that ended without one.
    attr_reader :returncode

    # Block until it exits and answer the exit code, in polls of at most 20 s; +timeout_s+ nil
    # waits as long as it takes, else TimeoutError once it passes.
    def wait(timeout_s: nil)
      deadline = timeout_s && (Sandbox.now + timeout_s)
      loop do
        left = deadline && (deadline - Sandbox.now)
        ms = left ? [[(left * 1000).ceil, 0].max, WAIT_SLICE_MS].min : WAIT_SLICE_MS
        out = tool("process_wait", timeout_ms: ms)
        return @returncode = out["exit_code"] if out["state"] == "exited"
        raise TimeoutError, "process #{@id} in #{@sandbox.id} still running after #{timeout_s}s" if left && Sandbox.now >= deadline
      end
    end

    # The exit code if it has exited, else nil — never blocks.
    def poll
      out = tool("process_wait", timeout_ms: 0)
      out["state"] == "exited" ? (@returncode = out["exit_code"]) : nil
    end

    # +signal+ is the tool's own default (TERM) when nil.
    def kill(signal: nil)
      tool("process_kill", signal: signal)
    end

    def inspect
      "#<Kloudlite::SandboxProcess #{@id} in #{@sandbox.id}>"
    end

    private

    def tool(name, **args)
      @sandbox.call(tool: name, id: @id, **args)
    end

    # Follow the NDJSON stream from the start, yielding [stream, data] per output frame; ends
    # after the exit frame, whose code it records. A body that ends without one was cut off.
    def follow
      exited = false
      @c.__send__(:stream_ndjson, "/v1/sandboxes/#{seg(@sandbox.id)}/processes/#{seg(@id)}/stream",
                      query: { since: 0, since_err: 0 }) do |frame|
        if frame.key?("exit")
          @returncode = frame["exit"]
          exited = true
        elsif frame["stream"]
          yield frame["stream"], frame["data"].to_s
        end
      end
      raise IOError, "the output stream of process #{@id} ended before it exited" unless exited
    end

    # Both streams in one pass: [stdout, stderr, exit code].
    def collect
      out = +""
      err = +""
      follow { |name, data| (name == "stderr" ? err : out) << data }
      [out, err, @returncode]
    end

    # The process's stdin. Writes are delivered when the call returns, so +drain+ has nothing
    # to wait for; it exists so code written against an asyncio-style writer reads the same.
    # +write+ takes its data positionally, the way IO#write does.
    class Stdin
      def initialize(process)
        @p = process
      end

      # A String in any encoding is sent as UTF-8; a binary String's bytes go as they are.
      # Answers the number of bytes written.
      def write(data)
        s = data.to_s
        s = s.encode(Encoding::UTF_8) unless s.encoding == Encoding::BINARY
        @p.__send__(:tool, "process_write", data_base64: [s].pack("m0"))
        s.bytesize
      end

      # Close the process's stdin, so a reader sees end of file.
      def write_eof
        @p.__send__(:tool, "process_close_stdin")
        nil
      end

      def drain
        nil
      end
    end

    # stdout or stderr: +read+ the whole stream once the process exits, or +each_line+ as it
    # arrives. Each call follows the stream from the beginning.
    class Output
      include Enumerable

      def initialize(process, name)
        @p = process
        @name = name
      end

      # Everything this stream wrote, once the process has exited.
      def read
        text = +""
        @p.__send__(:follow) { |name, data| text << data if name == @name }
        text
      end

      # Lines as they arrive, each with its "\n" (like IO#each_line; +chomp: true+ drops it); a
      # last line with no newline is yielded at the end. No block answers an Enumerator.
      def each_line(chomp: false)
        return enum_for(:each_line, chomp: chomp) unless block_given?

        pending = +""
        @p.__send__(:follow) do |name, data|
          next unless name == @name

          pending << data
          while (i = pending.index("\n"))
            line = pending.slice!(0, i + 1)
            yield(chomp ? line.chomp : line)
          end
        end
        yield pending unless pending.empty?
        self
      end

      def each(&block)
        each_line(&block)
      end
    end
  end

  # CI pipelines, over /v1/pipelines: definitions live as YAML in an owner's "{owner}/pipelines"
  # repository and run against that owner's repositories, each job in its own sandbox. A run is
  # recorded on the repository it is about, so it is read back by that +repo:+ ("owner/name").
  # +run+, +runs+ and +get_run+ answer Run handles.
  #
  #   run = kl.pipelines.run(owner: "acme", pipeline: "ci", repo: "acme/payments")
  #   run.follow(job: "test") { |chunk| print chunk }
  #   run.wait   # => "success"
  class Pipelines
    include Names

    def initialize(client)
      @c = client
    end

    # The definitions on {owner}/pipelines's main branch, broken ones included:
    # {"repo", "sha", "pipelines" => [{"name", "file", "valid", "errors", "repos", "on", "jobs"}],
    # "errors"}. +owner+ (a handle or team slug) defaults to the caller.
    def list(owner: nil)
      @c.request("GET", "/v1/pipelines", query: { owner: owner })
    end

    # Start +owner+'s +pipeline+ by hand on +repo+ (a name, or "owner/name" — one of +owner+'s) at
    # +ref+ (a branch, tag, full ref or sha; default main) and answer the Run at once (202),
    # queued. +inputs+ are the ones the pipeline's on.manual.inputs declares. +environment+ runs
    # every job that names none in that environment; +clone+ instead in a per-run copy of it,
    # deleted when the run ends — one or the other. A definition that cannot run is an ApiError
    # 422 whose message lists every reason, and nothing is recorded.
    def run(owner:, pipeline:, repo:, ref: nil, inputs: nil, environment: nil, clone: nil)
      raise ArgumentError, "pass environment: or clone:, not both" if environment && clone

      body = { repo: repo, ref: ref, inputs: inputs,
               environment: clone ? { clone: clone } : environment }.compact
      Run.new(@c, @c.request("POST", "/v1/pipelines/#{seg(owner)}/#{seg(pipeline)}/runs", body: body))
    end

    # +repo+'s runs, newest first; +pipeline+ keeps one pipeline's.
    def runs(repo:, pipeline: nil, limit: nil)
      docs = @c.request("GET", Run.path_of(repo), query: { pipeline: pipeline, limit: limit }) || []
      docs.map { |doc| Run.new(@c, doc) }
    end

    # One run of +repo+ by its id ("1760400000000-0a1b2c").
    def get_run(repo:, run:)
      Run.new(@c, @c.request("GET", Run.path_of(repo, "/#{seg(run)}")))
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
    extend Names

    TERMINAL = %w[success failure cancelled].freeze

    attr_reader :doc

    def initialize(client, doc)
      @c = client
      @doc = doc || {}
    end

    def id = @doc["id"]
    def number = @doc["number"]
    def pipeline = @doc["pipeline"]
    def pipelines_repo = @doc["pipelinesRepo"]
    def pipelines_sha = @doc["pipelinesSha"]
    def repo = @doc["repo"]
    def ref = @doc["ref"]
    def sha = @doc["sha"]
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
      "#<Kloudlite::Run #{repo}##{number} #{pipeline} #{state}>"
    end

    # "owner/name" → the repo's runs path. Shared with Pipelines.
    def self.path_of(repo, tail = "")
      owner, name = split(repo)
      "/v1/repos/#{seg(owner)}/#{seg(name)}/runs#{tail}"
    end

    def self.now
      ::Process.clock_gettime(::Process::CLOCK_MONOTONIC)
    end

    private

    def base
      Run.path_of(repo, "/#{seg(id)}")
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

  # Model and dataset repositories, on the hub listener (Hugging Face Hub API).
  # Management level: create, read, refs, visibility, delete — files move with
  # git/huggingface_hub against the same hub URL. Every +repo:+ is one
  # "owner/name" string, exactly as in CodeRepos.
  class HubRepos
    include Names

    def initialize(client, kind)
      @c = client
      @kind = kind
      @plural = { "model" => "models", "dataset" => "datasets" }.fetch(kind)
    end

    # +private+ is sent explicitly rather than trusting a server default.
    def create(repo:, private: true)
      @c.hub_request("POST", "/api/repos/create", body: body_of(repo, private: private))
    end

    def get(repo:)
      @c.hub_request("GET", path(repo))
    end

    # Branches and tags, as the hub spells them.
    def refs(repo:)
      @c.hub_request("GET", path(repo, "/refs"))
    end

    def set_private(repo:, private:)
      @c.hub_request("PUT", path(repo, "/settings"), body: { private: private })
    end

    def delete(repo:)
      @c.hub_request("DELETE", "/api/repos/delete", body: body_of(repo))
    end

    # A new repo whose contents — large files included, through git-lfs — are fetched from a
    # PUBLIC git remote; +url+ is a strict "https://" URL (the Hugging Face Hub is one host among
    # any). Answers 202; poll +import_status+.
    def import_git(repo:, url:, private: true)
      @c.hub_request("POST", path(repo, "/import"), body: { from: url, private: private })
    end

    # {"from", "state" (queued|running|done|failed), "detail"?} for an imported repo.
    def import_status(repo:)
      @c.hub_request("GET", path(repo, "/import"))
    end

    private

    def body_of(repo, extra = {})
      owner, name = split(repo)
      { name: name, organization: owner, type: @kind }.merge(extra)
    end

    def path(repo, tail = "")
      owner, name = split(repo)
      "/api/#{@plural}/#{seg(owner)}/#{seg(name)}#{tail}"
    end
  end
end
