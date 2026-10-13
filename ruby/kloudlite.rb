# Kloudlite SDK for Ruby — stdlib only (net/http + json).
#
# Auth is a Bearer token (a session or CLI token). Every module hangs off the
# one client, and every repository is named by ONE "owner/name" string — the
# owner half being a person's handle or a team slug:
#
#   require_relative "kloudlite"
#   kl = Kloudlite::Client.new("https://api.example.com", token: "…")
#   repo = kl.repos.create("acme/payments")

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
    attr_reader :repos

    def initialize(base_url, token: nil, timeout: 30)
      @base_url = base_url.sub(%r{/+\z}, "")
      @token = token
      @timeout = timeout
      @repos = Repos.new(self)
    end

    # One HTTP round trip. Returns parsed JSON (or nil on an empty body);
    # raises ApiError on any non-2xx status.
    def request(method, path, body: nil, query: nil)
      uri = URI.parse(@base_url + path)
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
  class Repos
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
end
