# The fork workflow, end to end: fork a repo, commit onto a new branch of the
# fork, propose it back upstream as a cross-repo pull request, and merge it.
#
#   KLOUDLITE_URL=https://dev.kloudlite.io KLOUDLITE_TOKEN=… ruby example.rb

require "base64"
require_relative "kloudlite"

kl = Kloudlite::Client.new(base_url: ENV.fetch("KLOUDLITE_URL"), token: ENV.fetch("KLOUDLITE_TOKEN"))

upstream = "acme/payments" # the repo being contributed to
my_fork = "dev/payments"   # where the fork lands (a handle or a team slug)

begin
  # 1. Fork: objects and refs, never the upstream's pull requests or settings.
  fork = kl.code_repos.fork(repo: upstream, to: my_fork)
  puts "forked into #{fork['owner']}/#{fork['name']}"

  # 2. Commit a change onto a NEW branch of the fork (the fork's main stays put).
  content = Base64.strict_encode64("# Payments\n\nFixed rounding.\n")
  out = kl.code_repos.commit(repo: my_fork,
                             branch: "main", new_branch: "fix-rounding",
                             message: "Fix rounding in invoice totals",
                             changes: [{ path: "README.md", contentBase64: content }])
  puts "committed #{out['commit']} on #{out['branch']}"

  # 3. Propose it back: head names the FORK's branch, head_repo says where it lives.
  pr = kl.code_repos.open_pull(repo: upstream,
                               title: "Fix rounding in invoice totals",
                               head: "fix-rounding", base: "main",
                               head_repo: my_fork)
  puts "opened #{pr['_id']}"

  # 4. The diff upstream reviews is the mirror: pull/{n}/head.
  kl.code_repos.compare(repo: upstream, base: "main", head: "pull/#{pr['number']}/head")
  kl.code_repos.comment(repo: upstream, number: pr["number"], body: "Ready for review")

  # 5. Merge. 202 — a job; the pull reports where it got to.
  kl.code_repos.merge(repo: upstream, number: pr["number"], strategy: "squash")
  30.times do
    state = kl.code_repos.pull(repo: upstream, number: pr["number"])["state"]
    if state != "open"
      puts "pull is #{state}"
      break
    end
    sleep 2
  end
rescue Kloudlite::ApiError => e
  warn "failed with #{e.status}: #{e.message}"
end
