# Repo and image tools for agents

Date: 2026-10-11. Status: approved design, plan next.

## Goal

Every agent role (main session, workspace session, laptop `kl-workspace` and `kl-subagent`) can manage
code repositories and container images through `kl` tools: list, create, update, branch, compare,
open, comment on, merge and close pull requests, protect branches, list images and tags, change image
visibility, and delete repos, branches, images and tags. Deletes run only after the person says yes.

## Owner rulings (quoted)

- Roles: "Everything". Every role gets every tool, merge and delete included.
- Approach: thin `kl` tools over the routes that already exist (approach 1). No new server API.
- Deletes: "agent can delete freely but it is destructive action it should ask the user's permission".
- Laptop: "in claude code always try to use [the model], not hooks". The laptop ask is a skill rule
  the model follows, not a `PreToolUse` hook.
- Repo binding: "every workspace don't have unique repo and branch ... like mono repo. let agent set
  it later ... sometimes it will be setup in the workspace and later it will create a repo".
  `Workspace.spec.repo`/`branch` stay an optional seed; the agent links or creates the repo itself.

## Facts this design rests on (checked 2026-10-11)

- Both live workspaces (`ws-73edf65d9b542786`, `ws-879b4a9e8779e8b3`) can pull and push over SSH:
  `git ls-remote`, `git fetch --dry-run` and `git push --dry-run` to `kl:karthik1729/frontend` pass.
  The SSH server checks `may_act(user, owner)` once per connection for both upload-pack and
  receive-pack (`crates/git/src/ssh.rs:173-183`), so the dry-run push proves push access. Neither
  workspace has a git remote, because neither has `spec.repo`: the seed clone never ran.
- A bench-tool token is re-minted as the person's own session on the routes in
  `BENCH_PERSON_ROUTES` (`crates/api/src/lib.rs:437`, `bench_session`), and refused everywhere else.
  The layer covers every route of this tier, the GET fallback (`handle`) included.
- Already admitted: `GET/POST /v1/repos`, `GET /v1/repos/{o}/{n}`, `GET branches`, `GET compare`,
  `GET/POST pulls`, `GET pulls/{n}`, `POST pulls/{n}/{comments,merge,close}`, `POST commits`, team reads.
- Not admitted (401 today): `PATCH`/`DELETE /v1/repos/{o}/{n}`, `GET/POST .../protection`,
  `DELETE .../branches/{b}`, `GET /api/{o}/images`, `GET /api/{o}/{image}/imagetags`,
  `POST /api/{o}/{image}/{imagetagdelete,imagedelete,imagevisibility}`.
- Image routes authorize through `browse_caller`, which maps a verified team member to the owner, so
  a team member passes the `who == owner` check. No team gap.
- No harness tool calls any repo or image route yet.

## Tools

New file `harness/packages/tools/src/repos.ts` exporting `repoTools(): ToolDef[]`, built with the same
`def`/`api`/`seg`/`qs` helpers as `platform.ts` (exported from there). `platformTools` adds
`repoTools()` to the list of every kind (`main`, `workspace`, `all`). `owner` is always a required
parameter: the agent reads it from `workspace_get` or the person. Shapes are the routes' own.

| Tool | Route | Parameters |
|---|---|---|
| `repo_list` | `GET /v1/repos?owner=` | `owner` |
| `repo_get` | `GET /v1/repos/{o}/{n}` | `owner`, `name` |
| `repo_create` | `POST /v1/repos` | `owner`, `name`, `visibility?` (`public`/`private`, default private), `description?` |
| `repo_update` | `PATCH /v1/repos/{o}/{n}` | `owner`, `name`, `visibility?`, `description?` |
| `repo_delete` * | `DELETE /v1/repos/{o}/{n}` | `owner`, `name` |
| `protection_get` | `GET .../protection` | `owner`, `name` |
| `protection_set` | `POST .../protection` | `owner`, `name`, `pattern`, `remove?`, `no_force?`, `no_delete?` |
| `branch_list` | `GET .../branches` | `owner`, `name` |
| `branch_delete` * | `DELETE .../branches/{b}?oid=` | `owner`, `name`, `branch`. The tool reads the branch's oid from `branch_list` and sends it; a moved branch is a 409 the agent sees. |
| `compare` | `GET .../compare?base=&head=` | `owner`, `name`, `base`, `head` |
| `pr_list` | `GET .../pulls` | `owner`, `name`, `state?`, `limit?` |
| `pr_get` | `GET .../pulls/{n}` | `owner`, `name`, `number` |
| `pr_create` | `POST .../pulls` | `owner`, `name`, `title`, `base`, `head`, `body?` |
| `pr_comment` | `POST .../pulls/{n}/comments` | `owner`, `name`, `number`, `body` |
| `pr_merge` | `POST .../pulls/{n}/merge?strategy=` | `owner`, `name`, `number`, `strategy?` (`fast-forward`/`squash`/`merge`/`rebase`). Returns 202; poll `pr_get`. |
| `pr_close` | `POST .../pulls/{n}/close` | `owner`, `name`, `number` |
| `image_list` | `GET /api/{o}/images` | `owner` |
| `image_tags` | `GET /api/{o}/{image}/imagetags` | `owner`, `image` |
| `image_visibility` | `POST /api/{o}/{image}/imagevisibility?visibility=` | `owner`, `image`, `visibility` |
| `image_tag_delete` * | `POST /api/{o}/{image}/imagetagdelete` | `owner`, `image`, `tag`. The body is the tag as raw text, not JSON; `api` gains a raw-text body option for this one call. |
| `image_delete` * | `POST /api/{o}/{image}/imagedelete` | `owner`, `image` |

\* destructive. Descriptions are written in the skills' ASD-STE100 English and say, for each
destructive tool, that it deletes data that cannot be recovered and that the person must say yes first.

## Server change

Add to `BENCH_PERSON_ROUTES`:

```
("PATCH",  "/v1/repos/{owner}/{name}"),
("DELETE", "/v1/repos/{owner}/{name}"),
("GET",    "/v1/repos/{owner}/{name}/protection"),
("POST",   "/v1/repos/{owner}/{name}/protection"),
("DELETE", "/v1/repos/{owner}/{name}/branches/{branch}"),
("GET",    "/api/{owner}/images"),
("GET",    "/api/{owner}/{image}/imagetags"),
("POST",   "/api/{owner}/{image}/imagetagdelete"),
("POST",   "/api/{owner}/{image}/imagedelete"),
("POST",   "/api/{owner}/{image}/imagevisibility"),
```

The table's doc comment records the 2026-10-11 ruling (agents manage repos and images as the person;
deletes ask). The bench adds no authority: every admitted write is still gated by `settings_caller` /
`browse_caller` on the owner, as from a browser. `/v2` (registry) stays closed to the bench token.
The test at the end of `bench_person_route`'s tests checks that every table entry is spelled as a
literal route in `lib.rs`. `imagetags` has no literal route (the GET fallback `handle` serves it), so
that test exempts exactly that one entry, with a comment that names the fallback.

## Asking before a delete

- **Bench sessions** (main and workspace): add `repo_delete`, `branch_delete`, `image_tag_delete`,
  `image_delete` to `DESTRUCTIVE` (`harness/packages/backend/src/local.ts:45`). `asking` then gives
  them a required `because` and `mustAsk` asks the person before the tool runs, as for
  `workspace_delete`.
- **Laptop** (`kl-connect mcp`, no ask step in code): a skill rule. The main session asks the person
  with `AskUserQuestion` before it calls a destructive tool and names exactly what will be deleted.
  `kl-workspace` and `kl-subagent` cannot ask the person: before a destructive tool they stop and
  report `NEED` with the exact target; the main session asks and passes the yes back. No hook.

## Skills (ASD-STE100)

Add a "Repos and images" section to both `kloudlite` skills
(`harness/laptop-plugin/plugin/skills/kloudlite/SKILL.md`,
`harness/packages/agent/claude-plugin/skills/kloudlite/SKILL.md`), and rows to the tools tables of
`kl-workspace.md` and `workspace-session/SKILL.md`. Add the new tool names to the `tools:` frontmatter
of `kl-workspace.md` and `kl-subagent.md`. The rules, one instruction per sentence:

- Before you push code, run `git remote -v`. If it shows no `origin`, link a repo:
  `git remote add origin kl:<owner>/<repo>`. If the repo does not exist, create it with `repo_create`
  first.
- A repo can hold the code of more than one workspace. Each workspace uses its own branch.
- To merge work, open a pull request with `pr_create`. Merge it with `pr_merge` only when the person
  or the main session tells you to.
- Before `repo_delete`, `branch_delete`, `image_tag_delete` or `image_delete`, get a yes from the
  person (laptop wording: ask with `AskUserQuestion`; agent wording: report `NEED`).

## Tests

- `harness/packages/tools/src/repos.test.ts`: each tool sends its method, path, query and body
  (`image_tag_delete` sends raw text; `branch_delete` reads the oid then deletes with it; an unknown
  branch is an error without a DELETE).
- `platform.test.ts`: every kind includes the repo tools.
- `local.test.ts`: the four deletes are in `DESTRUCTIVE` and carry a required `because`.
- `crates/api/src/lib.rs` tests: a bench token on each new route reaches the handler as the person
  (not 401); a bench token on `/v2/...` and on a key route is still refused.
- Live check after ship: from a workspace session, `repo_create` a scratch repo, link it, push a
  branch, `pr_create`, `pr_comment`, `compare`, `image_list`, `image_tags`; then `branch_delete` and
  `repo_delete` show the ask and run after yes.

## Known gaps (not fixed here)

- `DELETE /v1/repos/{o}/{n}` refuses nothing beyond auth (no open-PR or protection check). The ask is
  the only guard.
- `DELETE branch` refuses an open PR whose head is the branch, not one whose base is it.
- The default branch is hard-coded `main` (`repos.rs:507`); `repo_update` cannot change it.
- `image_list` and `compare` drop paging (`n`, `last`) at this tier.
- Making a repo or image public is not gated by an ask; it is reversible.
