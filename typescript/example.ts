/**
 * The fork workflow, end to end: fork a repo, commit onto a new branch of the
 * fork, propose it back upstream as a cross-repo pull request, and merge it.
 *
 *     KLOUDLITE_URL=https://api.example.com KLOUDLITE_TOKEN=… bun run example.ts
 */

import { ApiError, Kloudlite } from "./index.ts";

// Keeps the example zero-dependency: with @types/node installed, delete this line.
declare const process: { env: Record<string, string | undefined> };

const kl = new Kloudlite({
  baseUrl: process.env.KLOUDLITE_URL!,
  token: process.env.KLOUDLITE_TOKEN,
});

const upstream = "acme/payments"; // the repo being contributed to
const myFork = "dev/payments"; // where the fork lands (a handle or a team slug)

try {
  // 1. Fork: objects and refs, never the upstream's pull requests or settings.
  const fork = await kl.codeRepos.fork(upstream, myFork);
  console.log(`forked into ${fork.owner}/${fork.name}`);

  // 2. Commit a change onto a NEW branch of the fork (the fork's main stays put).
  const out = await kl.codeRepos.commit(myFork, {
    branch: "main",
    newBranch: "fix-rounding",
    message: "Fix rounding in invoice totals",
    changes: [{ path: "README.md", contentBase64: btoa("# Payments\n\nFixed rounding.\n") }],
  });
  console.log(`committed ${out.commit} on ${out.branch}`);

  // 3. Propose it back: head names the FORK's branch, head_repo says where it lives.
  const pr = (await kl.codeRepos.openPull(upstream, {
    title: "Fix rounding in invoice totals",
    head: "fix-rounding",
    base: "main",
    head_repo: myFork,
  })) as { _id: string; number: number };
  console.log(`opened ${pr._id}`);

  // 4. The diff upstream reviews is the mirror: pull/{n}/head.
  await kl.codeRepos.compare(upstream, "main", `pull/${pr.number}/head`);
  await kl.codeRepos.comment(upstream, pr.number, "Ready for review");

  // 5. Merge. 202 — a job; the pull reports where it got to.
  await kl.codeRepos.merge(upstream, pr.number, "squash");
  for (let i = 0; i < 30; i++) {
    const state = ((await kl.codeRepos.pull(upstream, pr.number)) as { state: string }).state;
    if (state !== "open") {
      console.log("pull is", state);
      break;
    }
    await new Promise((r) => setTimeout(r, 2000));
  }
} catch (e) {
  if (e instanceof ApiError) console.error(`failed with ${e.status}: ${e.message}`);
  else throw e;
}
