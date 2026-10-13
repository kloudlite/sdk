# The fork workflow, end to end: fork a repo, commit onto a new branch of the
# fork, propose it back upstream as a cross-repo pull request, and merge it.
#
#   KLOUDLITE_URL=https://api.example.com KLOUDLITE_TOKEN=… ruby example.rb

require "base64"
require_relative "kloudlite"

kl = Kloudlite::Client.new(ENV.fetch("KLOUDLITE_URL"), token: ENV.fetch("KLOUDLITE_TOKEN"))

up = %w[acme payments] # the repo being contributed to
me = "dev"             # the handle the fork lands under

begin
  # 1. Fork: objects and refs, never the upstream's pull requests or settings.
  fork = kl.repos.fork(*up, owner: me, name: "payments")
  puts "forked into #{fork['owner']}/#{fork['name']}"

  # 2. Commit a change onto a NEW branch of the fork (the fork's main stays put).
  content = Base64.strict_encode64("# Payments\n\nFixed rounding.\n")
  out = kl.repos.commit(me, "payments",
                        branch: "main", new_branch: "fix-rounding",
                        message: "Fix rounding in invoice totals",
                        changes: [{ path: "README.md", contentBase64: content }])
  puts "committed #{out['commit']} on #{out['branch']}"

  # 3. Propose it back: head names the FORK's branch, head_repo says where it lives.
  pr = kl.repos.open_pull(*up,
                          title: "Fix rounding in invoice totals",
                          head: "fix-rounding", base: "main",
                          head_repo: "#{me}/payments")
  puts "opened #{pr['_id']}"

  # 4. The diff upstream reviews is the mirror: pull/{n}/head.
  kl.repos.compare(*up, base: "main", head: "pull/#{pr['number']}/head")
  kl.repos.comment(*up, pr["number"], "Ready for review")

  # 5. Merge. 202 — a job; the pull reports where it got to.
  kl.repos.merge(*up, pr["number"], strategy: "squash")
  30.times do
    state = kl.repos.pull(*up, pr["number"])["state"]
    if state != "open"
      puts "pull is #{state}"
      break
    end
    sleep 2
  end
rescue Kloudlite::ApiError => e
  warn "failed with #{e.status}: #{e.message}"
end
