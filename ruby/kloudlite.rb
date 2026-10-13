# Kloudlite SDK for Ruby — stdlib only (net/http + json).
#
# The client speaks to the browse API with HTTP Basic auth: the username names
# the owner the token belongs to (git's `x` placeholder also works), the token
# is the secret. No credentials is still a valid client — public repositories
# admit anonymous reads.
#
#   require_relative "kloudlite"
#   kl = Kloudlite::Client.new("https://git.example.com", username: "acme", token: "…")

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
    def initialize(base_url, username: "x", token: nil, timeout: 30)
      @base_url = base_url.sub(%r{/+\z}, "")
      @username = username
      @token = token
      @timeout = timeout
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
      req.basic_auth(@username, @token) if @token
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

    private

    def segment(s)
      URI.encode_uri_component(s)
    end
  end
end
