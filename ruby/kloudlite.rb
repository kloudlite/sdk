# Kloudlite SDK for Ruby — stdlib only (net/http + json).
#
# Auth is a Bearer token (a session or CLI token). Every module hangs off the
# one client, and every repository is named by ONE "owner/name" string — the
# owner half being a person's handle or a team slug:
#
#   require_relative "kloudlite"
#   kl = Kloudlite::Client.new("https://api.example.com", token: "…")
#   repo = kl.code_repos.create("acme/payments")

require "json"
require "net/http"
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

  class Client
    attr_reader :code_repos, :model_repos, :data_repos, :workspaces, :volumes

    def initialize(base_url, token: nil, hub_url: nil, timeout: 30)
      @base_url = base_url.sub(%r{/+\z}, "")
      @hub_url = hub_url&.sub(%r{/+\z}, "")
      @token = token
      @timeout = timeout
      @code_repos = CodeRepos.new(self)
      @model_repos = HubRepos.new(self, "model")
      @data_repos = HubRepos.new(self, "dataset")
      @workspaces = Workspaces.new(self)
      @volumes = Volumes.new(self)
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

    def send_to(base, method, path, body: nil, query: nil)
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
      resp = Net::HTTP.start(uri.hostname, uri.port,
                             use_ssl: uri.scheme == "https",
                             open_timeout: @timeout, read_timeout: @timeout) { |http| http.request(req) }
      raise ApiError.new(resp.code.to_i, self.class.error_text(resp.body)) unless resp.is_a?(Net::HTTPSuccess)
      resp.body.nil? || resp.body.empty? ? nil : JSON.parse(resp.body)
    end

    # The server answers errors as {"error": …} or plain text; take either.
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
  end

  # Code repositories: /v1/repos. Every +repo+ argument is one "owner/name"
  # string; only owner-scoped calls (+list+) take a bare handle.
  class CodeRepos
    def initialize(client)
      @c = client
    end

    # repos
    def create(repo, visibility: "private", description: "")
      owner, name = split(repo)
      @c.request("POST", "/v1/repos",
                 body: { owner: owner, name: name, visibility: visibility, description: description })
    end

    def list(owner)
      @c.request("GET", "/v1/repos", query: { owner: owner })
    end

    def get(repo)
      @c.request("GET", path(repo))
    end

    def update(repo, description: nil, visibility: nil)
      @c.request("PATCH", path(repo), body: { description: description, visibility: visibility })
    end

    def delete(repo)
      @c.request("DELETE", path(repo))
    end

    # A new repo +dst+ copied from +src+: objects and refs, never its pull
    # requests or settings. Private by default, whatever the source was.
    def fork(src, dst, visibility: "private", description: "")
      owner, name = split(dst)
      @c.request("POST", path(src, "/fork"),
                 body: { owner: owner, name: name, visibility: visibility, description: description })
    end

    # A new repo whose contents are fetched from a PUBLIC git remote — +git+ is a strict
    # "https://" URL (GitHub is one host among any). Answers 202: the repo exists but the fetch
    # runs in the background; poll +import_status+.
    def import_git(repo, git, visibility: "private", description: "")
      owner, name = split(repo)
      @c.request("POST", "/v1/repos/import",
                 body: { owner: owner, name: name, git: git,
                         visibility: visibility, description: description })
    end

    # {"from", "state" (queued|running|done|failed), "detail"?} for an imported repo.
    def import_status(repo)
      @c.request("GET", path(repo, "/import"))
    end

    # branches
    def branches(repo)
      @c.request("GET", path(repo, "/branches"))
    end

    # +oid+ is the head the caller saw; the delete is refused if the branch moved.
    def delete_branch(repo, branch, oid)
      @c.request("DELETE", path(repo, "/branches/#{seg(branch)}"), query: { oid: oid })
    end

    # protection
    def protection(repo)
      @c.request("GET", path(repo, "/protection"))
    end

    def protect(repo, pattern:, remove: false, no_force: true, no_delete: true)
      @c.request("POST", path(repo, "/protection"),
                 body: { pattern: pattern, remove: remove, no_force: no_force, no_delete: no_delete })
    end

    # pull requests
    def pulls(repo, state: nil, limit: nil)
      @c.request("GET", path(repo, "/pulls"), query: { state: state, limit: limit })
    end

    def pull(repo, number)
      @c.request("GET", path(repo, "/pulls/#{number.to_i}"))
    end

    # +head_repo+ (another "owner/name") makes it a cross-repo pull: +head+ then
    # names a branch of that fork, proposed onto this repo's +base+.
    def open_pull(repo, title:, head:, base:, description: "", head_repo: nil)
      split(head_repo) unless head_repo.nil?
      @c.request("POST", path(repo, "/pulls"),
                 body: { title: title, head: head, base: base, description: description,
                         head_repo: head_repo })
    end

    def comment(repo, number, body)
      @c.request("POST", path(repo, "/pulls/#{number.to_i}/comments"), body: { body: body })
    end

    # strategy: fast-forward, squash, merge or rebase. Answers 202 — the merge is a job.
    def merge(repo, number, strategy: "fast-forward")
      @c.request("POST", path(repo, "/pulls/#{number.to_i}/merge"), query: { strategy: strategy })
    end

    def close(repo, number)
      @c.request("POST", path(repo, "/pulls/#{number.to_i}/close"))
    end

    # commits
    def compare(repo, base:, head:)
      @c.request("GET", path(repo, "/compare"), query: { base: base, head: head })
    end

    # One commit of file changes; each change is {path:, contentBase64:, executable:, delete:}.
    # +expect+ refuses the commit if the branch moved.
    def commit(repo, branch:, message:, changes:, expect: nil, new_branch: nil)
      @c.request("POST", path(repo, "/commits"),
                 body: { branch: branch, message: message, changes: changes,
                         expect: expect, newBranch: new_branch })
    end

    def signature(repo, sha)
      @c.request("GET", path(repo, "/commits/#{seg(sha)}/signature"))
    end

    private

    def path(repo, tail = "")
      owner, name = split(repo)
      "/v1/repos/#{seg(owner)}/#{seg(name)}#{tail}"
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

  # Btrfs-backed dev workspaces, over /v1/workspaces. A workspace is named by its id (the +name+
  # given at create); pushes cut snapshots, kept on the workspace's VOLUME — see Volumes for
  # history, restore targets and deletion.
  class Workspaces
    def initialize(client)
      @c = client
    end

    # +repo+ seeds from a PLATFORM repo ("owner/name", with +branch+); +packages+ are nixpkgs
    # attributes, optionally pinned ("attr@version"). +region+ matters only on a multi-region
    # fleet — absent, the deployment's one region is used.
    def create(name:, quota_gb:, team: nil, image: nil, repo: nil, branch: nil, packages: [], region: nil)
      body = { name: name, region: region, quota_gb: quota_gb,
               team: team, repo: repo, branch: branch, packages: packages }
      body[:image] = image unless image.nil?
      @c.request("POST", "/v1/workspaces", body: body)
    end

    # The caller's workspaces; +team+ scopes to their workspaces in that team.
    def list(team: nil)
      @c.request("GET", "/v1/workspaces", query: { team: team })
    end

    def get(id)
      @c.request("GET", "/v1/workspaces/#{seg(id)}")
    end

    # Runs the worktree finalizer: the volume survives while a pushed snapshot references it.
    def delete(id)
      @c.request("DELETE", "/v1/workspaces/#{seg(id)}")
    end

    def start(id)
      @c.request("POST", "/v1/workspaces/#{seg(id)}/start")
    end

    # A stop cuts a sync point and tears the pod down — seconds, never waiting on a replica.
    def stop(id)
      @c.request("POST", "/v1/workspaces/#{seg(id)}/stop")
    end

    # Cut a SNAPSHOT of the working copy — the one durable, restorable state.
    def push(id, message: nil)
      @c.request("POST", "/v1/workspaces/#{seg(id)}/push",
                 body: message.nil? ? nil : { message: message })
    end

    # A new workspace from this one's state, cut at the moment of the request; the answer's
    # +based_on+ names exactly what it grafted onto.
    def clone(id, name, task: nil)
      @c.request("POST", "/v1/workspaces/#{seg(id)}/clone", body: { name: name, task: task })
    end

    # A new workspace grafted onto a PUSHED snapshot — also one whose workspace is long gone.
    # Absent overrides mean "whatever the snapshot froze".
    def restore(name:, snapshot_id:, image: nil, packages: nil, quota_gb: nil)
      @c.request("POST", "/v1/workspaces/restore",
                 body: { name: name, snapshot_id: snapshot_id,
                         image: image, packages: packages, quota_gb: quota_gb })
    end

    # Replace the package list; pinned entries are locked before anything is written.
    def set_packages(id, packages)
      @c.request("PATCH", "/v1/workspaces/#{seg(id)}", body: { packages: packages })
    end

    # Re-resolve every pinned package — the only call that moves existing locks.
    def update_packages(id)
      @c.request("POST", "/v1/workspaces/#{seg(id)}/packages/update")
    end

    # The workspace's tool-server address — answered to the owner alone.
    def tools(id)
      @c.request("GET", "/v1/workspaces/#{seg(id)}/tools")
    end

    private

    def seg(s)
      URI.encode_uri_component(s.to_s)
    end
  end

  # The btrfs volumes behind workspaces and environments: snapshot history, restore targets, and
  # the two deletes — over /v1/volumes.
  class Volumes
    def initialize(client)
      @c = client
    end

    # +kind+ is "workspace" or "environment"; +owner+ a handle or team slug.
    def list(kind: nil, owner: nil)
      @c.request("GET", "/v1/volumes", query: { kind: kind, owner: owner })
    end

    # The chain of PUSHED snapshots, newest first — each row a restore target.
    def history(name)
      @c.request("GET", "/v1/volumes/#{seg(name)}/history")
    end

    def refs(name)
      @c.request("GET", "/v1/volumes/#{seg(name)}/refs")
    end

    # Refused (409) for a sync point or a running worktree's base; deleting a detached volume's
    # last snapshot deletes the volume.
    def delete_snapshot(name, snapshot_id)
      @c.request("DELETE", "/v1/volumes/#{seg(name)}/snapshots/#{seg(snapshot_id)}")
    end

    # Takes a DETACHED volume with all its snapshots; refused while a working copy remains.
    def delete(name)
      @c.request("DELETE", "/v1/volumes/#{seg(name)}")
    end

    private

    def seg(s)
      URI.encode_uri_component(s.to_s)
    end
  end

  # Model and dataset repositories, on the hub listener (Hugging Face Hub API).
  # Management level: create, read, refs, visibility, delete — files move with
  # git/huggingface_hub against the same hub URL. Every +repo+ is one
  # "owner/name" string, exactly as in CodeRepos.
  class HubRepos
    def initialize(client, kind)
      @c = client
      @kind = kind
      @plural = { "model" => "models", "dataset" => "datasets" }.fetch(kind)
    end

    # +private+ is sent explicitly rather than trusting a server default.
    def create(repo, private: true)
      @c.hub_request("POST", "/api/repos/create", body: body_of(repo, private: private))
    end

    def get(repo)
      @c.hub_request("GET", path(repo))
    end

    # Branches and tags, as the hub spells them.
    def refs(repo)
      @c.hub_request("GET", path(repo, "/refs"))
    end

    def set_private(repo, private)
      @c.hub_request("PUT", path(repo, "/settings"), body: { private: private })
    end

    def delete(repo)
      @c.hub_request("DELETE", "/api/repos/delete", body: body_of(repo))
    end

    # A new repo whose contents — large files included, through git-lfs — are fetched from a
    # PUBLIC git remote; +git+ is a strict "https://" URL (the Hugging Face Hub is one host among
    # any). Answers 202; poll +import_status+.
    def import_git(repo, git, private: true)
      @c.hub_request("POST", path(repo, "/import"), body: { from: git, private: private })
    end

    # {"from", "state" (queued|running|done|failed), "detail"?} for an imported repo.
    def import_status(repo)
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
end
