"""The fork workflow, end to end: fork a repo, commit onto a new branch of the
fork, propose it back upstream as a cross-repo pull request, and merge it.

    KLOUDLITE_URL=https://api.example.com KLOUDLITE_TOKEN=… python3 example.py
"""

import base64
import os
import time

from kloudlite import ApiError, Kloudlite

kl = Kloudlite(os.environ["KLOUDLITE_URL"], token=os.environ["KLOUDLITE_TOKEN"])

UPSTREAM = ("acme", "payments")  # the repo being contributed to
ME = "dev"                       # the handle the fork lands under

try:
    # 1. Fork: objects and refs, never the upstream's pull requests or settings.
    fork = kl.repos.fork(*UPSTREAM, owner=ME, name="payments")
    print("forked into", fork["owner"] + "/" + fork["name"])

    # 2. Commit a change onto a NEW branch of the fork (the fork's main stays put).
    content = base64.b64encode(b"# Payments\n\nFixed rounding.\n").decode()
    out = kl.repos.commit(ME, "payments",
                          branch="main", new_branch="fix-rounding",
                          message="Fix rounding in invoice totals",
                          changes=[{"path": "README.md", "contentBase64": content}])
    print("committed", out["commit"], "on", out["branch"])

    # 3. Propose it back: head names the FORK's branch, head_repo says where it lives.
    pr = kl.repos.open_pull(*UPSTREAM,
                            title="Fix rounding in invoice totals",
                            head="fix-rounding", base="main",
                            head_repo=f"{ME}/payments")
    print("opened", pr["_id"])

    # 4. The diff upstream reviews is the mirror: pull/{n}/head.
    diff = kl.repos.compare(*UPSTREAM, base="main", head=f"pull/{pr['number']}/head")
    kl.repos.comment(*UPSTREAM, pr["number"], "Ready for review")

    # 5. Merge. 202 — a job; the pull reports where it got to.
    kl.repos.merge(*UPSTREAM, pr["number"], strategy="squash")
    for _ in range(30):
        state = kl.repos.pull(*UPSTREAM, pr["number"])["state"]
        if state != "open":
            print("pull is", state)
            break
        time.sleep(2)
except ApiError as e:
    print(f"failed with {e.status}: {e.message}")
