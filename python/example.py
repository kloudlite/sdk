"""The fork workflow, end to end: fork a repo, commit onto a new branch of the
fork, propose it back upstream as a cross-repo pull request, and merge it.

    KLOUDLITE_URL=https://dev.kloudlite.io KLOUDLITE_TOKEN=… python3 example.py
"""

import base64
import os
import time

from kloudlite import ApiError, Kloudlite

kl = Kloudlite(base_url=os.environ["KLOUDLITE_URL"], token=os.environ["KLOUDLITE_TOKEN"])

UPSTREAM = "acme/payments"  # the repo being contributed to
FORK = "dev/payments"       # where the fork lands (a handle or a team slug)

try:
    # 1. Fork: objects and refs, never the upstream's pull requests or settings.
    fork = kl.code_repos.fork(repo=UPSTREAM, to=FORK)
    print("forked into", fork["owner"] + "/" + fork["name"])

    # 2. Commit a change onto a NEW branch of the fork (the fork's main stays put).
    content = base64.b64encode(b"# Payments\n\nFixed rounding.\n").decode()
    out = kl.code_repos.commit(repo=FORK,
                               branch="main", new_branch="fix-rounding",
                               message="Fix rounding in invoice totals",
                               changes=[{"path": "README.md", "contentBase64": content}])
    print("committed", out["commit"], "on", out["branch"])

    # 3. Propose it back: head names the FORK's branch, head_repo says where it lives.
    pr = kl.code_repos.open_pull(repo=UPSTREAM,
                                 title="Fix rounding in invoice totals",
                                 head="fix-rounding", base="main",
                                 head_repo=FORK)
    print("opened", pr["_id"])

    # 4. The diff upstream reviews is the mirror: pull/{n}/head.
    diff = kl.code_repos.compare(repo=UPSTREAM, base="main", head=f"pull/{pr['number']}/head")
    kl.code_repos.comment(repo=UPSTREAM, number=pr["number"], body="Ready for review")

    # 5. Merge. 202 — a job; the pull reports where it got to.
    kl.code_repos.merge(repo=UPSTREAM, number=pr["number"], strategy="squash")
    for _ in range(30):
        state = kl.code_repos.pull(repo=UPSTREAM, number=pr["number"])["state"]
        if state != "open":
            print("pull is", state)
            break
        time.sleep(2)
except ApiError as e:
    print(f"failed with {e.status}: {e.message}")
