# Kloudlite SDK for Ruby — stdlib only (net/http + json).
#
# Auth is a Bearer token (a session or CLI token). Every module hangs off the
# one client:
#
#   require_relative "kloudlite"
#   kl = Kloudlite::Client.new("https://api.example.com", token: "…")
#   repo = kl.repos.create(owner: "acme", name: "payments")

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

  # Code repositories: /v1/repos.
  class Repos
    def initialize(client)
      @c = client
    end

    # repos
    def create(owner:, name:, visibility: "private", description: "")
      @c.request("POST", "/v1/repos",
                 body: { owner: owner, name: name, visibility: visibility, description: description })
    end

    def list(owner)
      @c.request("GET", "/v1/repos", query: { owner: owner })
    end

    def get(owner, name)
      @c.request("GET", "/v1/repos/#{seg(owner)}/#{seg(name)}")
    end

    def update(owner, name, description: nil, visibility: nil)
      @c.request("PATCH", "/v1/repos/#{seg(owner)}/#{seg(name)}",
                 body: { description: description, visibility: visibility })
    end

    def delete(owner, name)
      @c.request("DELETE", "/v1/repos/#{seg(owner)}/#{seg(name)}")
    end

    # branches
    def branches(owner, name)
      @c.request("GET", "/v1/repos/#{seg(owner)}/#{seg(name)}/branches")
    end

    # +oid+ is the head the caller saw; the delete is refused if the branch moved.
    def delete_branch(owner, name, branch, oid)
      @c.request("DELETE", "/v1/repos/#{seg(owner)}/#{seg(name)}/branches/#{seg(branch)}",
                 query: { oid: oid })
    end

    # protection
    def protection(owner, name)
      @c.request("GET", "/v1/repos/#{seg(owner)}/#{seg(name)}/protection")
    end

    def protect(owner, name, pattern:, remove: false, no_force: true, no_delete: true)
      @c.request("POST", "/v1/repos/#{seg(owner)}/#{seg(name)}/protection",
                 body: { pattern: pattern, remove: remove, no_force: no_force, no_delete: no_delete })
    end

    # pull requests
    def pulls(owner, name, state: nil, limit: nil)
      @c.request("GET", "/v1/repos/#{seg(owner)}/#{seg(name)}/pulls",
                 query: { state: state, limit: limit })
    end

    def pull(owner, name, number)
      @c.request("GET", "/v1/repos/#{seg(owner)}/#{seg(name)}/pulls/#{number.to_i}")
    end

    def open_pull(owner, name, title:, head:, base:, description: "")
      @c.request("POST", "/v1/repos/#{seg(owner)}/#{seg(name)}/pulls",
                 body: { title: title, head: head, base: base, description: description })
    end

    def comment(owner, name, number, body)
      @c.request("POST", "/v1/repos/#{seg(owner)}/#{seg(name)}/pulls/#{number.to_i}/comments",
                 body: { body: body })
    end

    # strategy: fast-forward, squash, merge or rebase. Answers 202 — the merge is a job.
    def merge(owner, name, number, strategy: "fast-forward")
      @c.request("POST", "/v1/repos/#{seg(owner)}/#{seg(name)}/pulls/#{number.to_i}/merge",
                 query: { strategy: strategy })
    end

    def close(owner, name, number)
      @c.request("POST", "/v1/repos/#{seg(owner)}/#{seg(name)}/pulls/#{number.to_i}/close")
    end

    # commits
    def compare(owner, name, base:, head:)
      @c.request("GET", "/v1/repos/#{seg(owner)}/#{seg(name)}/compare",
                 query: { base: base, head: head })
    end

    # One commit of file changes; each change is {path:, contentBase64:, executable:, delete:}.
    # +expect+ refuses the commit if the branch moved.
    def commit(owner, name, branch:, message:, changes:, expect: nil, new_branch: nil)
      @c.request("POST", "/v1/repos/#{seg(owner)}/#{seg(name)}/commits",
                 body: { branch: branch, message: message, changes: changes,
                         expect: expect, newBranch: new_branch })
    end

    def signature(owner, name, sha)
      @c.request("GET", "/v1/repos/#{seg(owner)}/#{seg(name)}/commits/#{seg(sha)}/signature")
    end

    private

    def seg(s)
      URI.encode_uri_component(s.to_s)
    end
  end
end
