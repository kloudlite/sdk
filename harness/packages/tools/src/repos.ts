//! Repo, pull request and image tools: thin wrappers over the routes the api tier already serves
//! (design: docs/superpowers/specs/2026-10-11-repo-image-tools-design.md). Every role gets all of
//! them. A bench token reaches these routes as the person (`BENCH_PERSON_ROUTES`), so the server's
//! own owner checks still apply. `owner` is always an explicit parameter: the agent reads it from
//! `workspace_get` or the person. The four destructive tools (`repo_delete`, `branch_delete`,
//! `image_tag_delete`, `image_delete`) are gated in backend/src/local.ts (`DESTRUCTIVE`).
import type { ToolDef } from "./index.ts";
import { api, def, qs, seg } from "./platform.ts";

const S = { type: "string" };
const N = { type: "number" };
const B = { type: "boolean" };
const VIS = { type: "string", enum: ["public", "private"] };
const OWNER = { type: "string", description: "The owner: a user handle or a team slug." };
const clean = (o: Record<string, unknown>) => Object.fromEntries(Object.entries(o).filter(([, v]) => v !== undefined));
const GONE = " This deletes data that cannot be recovered. Get a yes from the person before you call it.";

export function repoTools(): ToolDef[] {
  const R = (a: any) => `/v1/repos/${seg(a.owner)}/${seg(a.name)}`;
  const P = (a: any) => `${R(a)}/pulls/${seg(String(a.number))}`;
  const I = (a: any) => `/api/${seg(a.owner)}/${seg(a.image)}`;
  const repo = { owner: OWNER, name: S };
  const pr = { ...repo, number: N };
  const img = { owner: OWNER, image: S };
  return [
    def("repo_list", "List the repos of an owner.", { owner: OWNER }, ["owner"], async (a) => api("GET", `/v1/repos${qs({ owner: a.owner })}`)),
    def("repo_get", "Get one repo.", repo, ["owner", "name"], async (a) => api("GET", R(a))),
    def("repo_create", "Create a repo. The default visibility is private.", { ...repo, visibility: VIS, description: S }, ["owner", "name"], async (a) =>
      api("POST", "/v1/repos", clean({ owner: a.owner, name: a.name, visibility: a.visibility, description: a.description }))),
    def("repo_update", "Change the visibility or the description of a repo.", { ...repo, visibility: VIS, description: S }, ["owner", "name"], async (a) =>
      api("PATCH", R(a), clean({ visibility: a.visibility, description: a.description }))),
    def("repo_delete", "Delete a repo with all its branches and pull requests." + GONE, repo, ["owner", "name"], async (a) => api("DELETE", R(a))),
    def("protection_get", "List the branch protection rules of a repo.", repo, ["owner", "name"], async (a) => api("GET", `${R(a)}/protection`)),
    def("protection_set", "Add or remove a branch protection rule. `pattern` is a branch name or a glob. `no_force` and `no_delete` default to true. Set `remove` to delete the rule.",
      { ...repo, pattern: S, remove: B, no_force: B, no_delete: B }, ["owner", "name", "pattern"], async (a) =>
      api("POST", `${R(a)}/protection`, clean({ pattern: a.pattern, remove: a.remove, no_force: a.no_force, no_delete: a.no_delete }))),
    def("branch_list", "List the branches of a repo with the commit id of each.", repo, ["owner", "name"], async (a) => api("GET", `${R(a)}/branches`)),
    def("branch_delete", "Delete one branch. The tool reads the current commit of the branch and sends it. The delete fails with 409 if the branch moved." + GONE,
      { ...repo, branch: S }, ["owner", "name", "branch"], async (a) => {
      const list = JSON.parse(await api("GET", `${R(a)}/branches`));
      const hit = Array.isArray(list) ? list.find((b: any) => b.name === a.branch) : undefined;
      if (!hit?.oid) throw new Error(`no such branch: ${a.branch}`);
      return api("DELETE", `${R(a)}/branches/${seg(a.branch)}${qs({ oid: hit.oid })}`);
    }),
    def("compare", "Show what the `head` branch adds to the `base` branch.", { ...repo, base: S, head: S }, ["owner", "name", "base", "head"], async (a) =>
      api("GET", `${R(a)}/compare${qs({ base: a.base, head: a.head })}`)),
    def("pr_list", "List the pull requests of a repo. `state` is open, merged or closed.", { ...repo, state: S, limit: N }, ["owner", "name"], async (a) =>
      api("GET", `${R(a)}/pulls${qs({ state: a.state, limit: a.limit === undefined ? undefined : String(a.limit) })}`)),
    def("pr_get", "Get one pull request, with its comments and merge state.", pr, ["owner", "name", "number"], async (a) => api("GET", P(a))),
    def("pr_create", "Open a pull request that merges the `head` branch into the `base` branch.", { ...repo, title: S, base: S, head: S, body: S }, ["owner", "name", "title", "base", "head"], async (a) =>
      api("POST", `${R(a)}/pulls`, clean({ title: a.title, base: a.base, head: a.head, body: a.body }))),
    def("pr_comment", "Add a comment to a pull request.", { ...pr, body: S }, ["owner", "name", "number", "body"], async (a) =>
      api("POST", `${P(a)}/comments`, { body: a.body })),
    def("pr_merge", "Merge a pull request. The default strategy is fast-forward. It returns 202 immediately. The merge continues after the return. Poll `pr_get`.",
      { ...pr, strategy: { type: "string", enum: ["fast-forward", "squash", "merge", "rebase"] } }, ["owner", "name", "number"], async (a) =>
      api("POST", `${P(a)}/merge${qs({ strategy: a.strategy })}`)),
    def("pr_close", "Close a pull request without a merge.", pr, ["owner", "name", "number"], async (a) => api("POST", `${P(a)}/close`)),
    def("image_list", "List the container images of an owner.", { owner: OWNER }, ["owner"], async (a) => api("GET", `/api/${seg(a.owner)}/images`)),
    def("image_tags", "List the tags of one image.", img, ["owner", "image"], async (a) => api("GET", `${I(a)}/imagetags`)),
    def("image_visibility", "Make an image public or private.", { ...img, visibility: VIS }, ["owner", "image", "visibility"], async (a) =>
      api("POST", `${I(a)}/imagevisibility${qs({ visibility: a.visibility })}`)),
    def("image_tag_delete", "Delete one tag of an image." + GONE, { ...img, tag: S }, ["owner", "image", "tag"], async (a) =>
      api("POST", `${I(a)}/imagetagdelete`, a.tag, true)),
    def("image_delete", "Delete an image with all its tags." + GONE, img, ["owner", "image"], async (a) => api("POST", `${I(a)}/imagedelete`)),
  ];
}
