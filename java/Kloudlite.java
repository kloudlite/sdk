// Kloudlite SDK for Java — one file, no dependencies (java.net.http, Java 11+).
//
// Auth is a Bearer token (a session or CLI token). Every module hangs off the
// one client, and every repository is named by ONE "owner/name" string — the
// owner half being a person's handle or a team slug.
//
// Java has no named arguments, so no call ever takes two inputs by position: a
// method whose only input is the resource itself takes that one String
// (get("acme/payments"), start("acme/dev")), and every other method takes ONE
// params object — a nested class named <Verb><Noun> with a fluent setter per
// field. A missing required field is an IllegalArgumentException("<field> is
// required") before anything is sent.
//
//   Kloudlite kl = new Kloudlite(new Kloudlite.Options().baseUrl("https://api.example.com").token("…"));
//   String repo = kl.codeRepos().create(new Kloudlite.CreateRepo().repo("acme/payments"));
//
// Methods return the raw JSON body as a String; bring your own JSON mapper.

import java.io.IOException;
import java.net.URI;
import java.net.URLEncoder;
import java.net.http.HttpClient;
import java.net.http.HttpRequest;
import java.net.http.HttpResponse;
import java.nio.charset.StandardCharsets;
import java.time.Duration;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.List;
import java.util.Map;

public class Kloudlite {
    public static final String VERSION = "0.1.0";

    /** A non-2xx answer: {@code status} is the HTTP code, {@code message} the server's text. */
    public static class ApiException extends IOException {
        public final int status;
        public final String message;

        public ApiException(int status, String message) {
            super(status + ": " + message);
            this.status = status;
            this.message = message;
        }
    }

    /**
     * How to reach the API: {@code baseUrl} is required; {@code token} null means anonymous
     * (public data only); {@code hubUrl} is the hub listener models and datasets live on.
     */
    public static final class Options {
        private String baseUrl, hubUrl, token;

        public Options baseUrl(String v) { baseUrl = v; return this; }
        public Options hubUrl(String v) { hubUrl = v; return this; }
        public Options token(String v) { token = v; return this; }
    }

    private final String baseUrl;
    private final String hubUrl; // null = no hub listener configured
    private final String token; // null = anonymous
    private final HttpClient http;
    private final CodeRepos codeRepos = new CodeRepos();
    private final HubRepos modelRepos = new HubRepos("model", "models");
    private final HubRepos dataRepos = new HubRepos("dataset", "datasets");
    private final Workspaces workspaces = new Workspaces();
    private final Environments environments = new Environments();
    private final Sessions sessions = new Sessions();
    private final Volumes volumes = new Volumes();

    public Kloudlite(Options opts) {
        this.baseUrl = need(opts.baseUrl, "baseUrl").replaceAll("/+$", "");
        this.hubUrl = opts.hubUrl == null ? null : opts.hubUrl.replaceAll("/+$", "");
        this.token = opts.token;
        this.http = HttpClient.newBuilder().connectTimeout(Duration.ofSeconds(30)).build();
    }

    /** Code repositories: {@code /v1/repos}. */
    public CodeRepos codeRepos() {
        return codeRepos;
    }

    /** Model repositories, on the hub listener. */
    public HubRepos modelRepos() {
        return modelRepos;
    }

    /** Dataset repositories, on the hub listener. */
    public HubRepos dataRepos() {
        return dataRepos;
    }

    /** Btrfs-backed dev workspaces: {@code /v1/workspaces}. */
    public Workspaces workspaces() {
        return workspaces;
    }

    /** Multi-service environments: {@code /v1/environments}. */
    public Environments environments() {
        return environments;
    }

    /** Working sessions: {@code /v1/sessions}. */
    public Sessions sessions() {
        return sessions;
    }

    /** The volumes behind workspaces and environments: {@code /v1/volumes}. */
    public Volumes volumes() {
        return volumes;
    }

    /** The caller's own ceiling and what is against it. */
    public String quota() throws IOException, InterruptedException {
        return quota(null);
    }

    /** The ceiling and what is against it; a null owner means the caller's own. */
    public String quota(String owner) throws IOException, InterruptedException {
        return api("GET", owner == null ? "/v1/quota" : "/v1/quota?owner=" + seg(owner), null);
    }

    /**
     * The low-level escape hatch: one HTTP round trip with any body or query. Returns the
     * answer's raw JSON body, and throws {@link ApiException} on any non-2xx status.
     */
    public static final class RawRequest {
        private String method, path, body;
        private final List<String> query = new ArrayList<>();

        public RawRequest method(String v) { method = v; return this; }
        public RawRequest path(String v) { path = v; return this; }
        /** The raw JSON body; null sends none. */
        public RawRequest body(String v) { body = v; return this; }
        /** One query parameter, appended in call order; a null value drops it. */
        public RawRequest query(String key, String value) { query.add(key); query.add(value); return this; }

        private String target() {
            return need(path, "path") + Kloudlite.query(query.toArray(new String[0]));
        }
    }

    /** A bodiless round trip — method and path are the one positional pair, being HTTP's own. */
    public String request(String method, String path) throws IOException, InterruptedException {
        return send(baseUrl, method, path, null);
    }

    public String request(RawRequest r) throws IOException, InterruptedException {
        return send(baseUrl, need(r.method, "method"), r.target(), r.body);
    }

    /** The same round trips against the hub listener (models and datasets). */
    public String hubRequest(String method, String path) throws IOException, InterruptedException {
        return hub(method, path, null);
    }

    public String hubRequest(RawRequest r) throws IOException, InterruptedException {
        return hub(need(r.method, "method"), r.target(), r.body);
    }

    private String api(String method, String path, String jsonBody) throws IOException, InterruptedException {
        return send(baseUrl, method, path, jsonBody);
    }

    private String hub(String method, String path, String jsonBody) throws IOException, InterruptedException {
        if (hubUrl == null) {
            throw new IllegalStateException("this client has no hub URL; pass one to reach models and datasets");
        }
        return send(hubUrl, method, path, jsonBody);
    }

    private String send(String base, String method, String path, String jsonBody)
            throws IOException, InterruptedException {
        HttpRequest.Builder b = HttpRequest.newBuilder()
            .uri(URI.create(base + path))
            .timeout(Duration.ofSeconds(30))
            .header("User-Agent", "kloudlite-sdk-java/" + VERSION)
            .header("Accept", "application/json")
            .method(method, jsonBody == null
                ? HttpRequest.BodyPublishers.noBody()
                : HttpRequest.BodyPublishers.ofString(jsonBody));
        if (jsonBody != null) b.header("Content-Type", "application/json");
        if (token != null) b.header("Authorization", "Bearer " + token);
        HttpResponse<String> resp = http.send(b.build(), HttpResponse.BodyHandlers.ofString());
        if (resp.statusCode() < 200 || resp.statusCode() > 299) {
            throw new ApiException(resp.statusCode(), errorText(resp.body()));
        }
        return resp.body();
    }

    /**
     * Every {@code repo} is one "owner/name" string; only owner-scoped calls ({@link #list})
     * take a bare handle.
     */
    public class CodeRepos {
        // repos
        public String create(CreateRepo p) throws IOException, InterruptedException {
            String[] on = split(need(p.repo, "repo"));
            return api("POST", "/v1/repos", obj(
                "owner", on[0], "name", on[1],
                "visibility", or(p.visibility, "private"),
                "description", or(p.description, "")));
        }

        public String list(String owner) throws IOException, InterruptedException {
            return api("GET", "/v1/repos?owner=" + seg(owner), null);
        }

        public String get(String repo) throws IOException, InterruptedException {
            return api("GET", path(repo, ""), null);
        }

        /** A field left unset is left unchanged. */
        public String update(UpdateRepo p) throws IOException, InterruptedException {
            return api("PATCH", path(need(p.repo, "repo"), ""),
                obj("visibility", p.visibility, "description", p.description));
        }

        public String delete(String repo) throws IOException, InterruptedException {
            return api("DELETE", path(repo, ""), null);
        }

        /**
         * A new repo {@code to} copied from {@code repo}: objects and refs, never its pull
         * requests or settings. An unset visibility means private, whatever the source was.
         */
        public String fork(ForkRepo p) throws IOException, InterruptedException {
            String src = need(p.repo, "repo");
            String[] on = split(need(p.to, "to"));
            return api("POST", path(src, "/fork"), obj(
                "owner", on[0], "name", on[1],
                "visibility", or(p.visibility, "private"),
                "description", or(p.description, "")));
        }

        /**
         * A new repo whose contents are fetched from a PUBLIC git remote — {@code url} is a
         * strict "https://" URL (GitHub is one host among any). Answers 202: the repo exists but
         * the fetch runs in the background; poll {@link #importStatus}.
         */
        public String importGit(ImportRepo p) throws IOException, InterruptedException {
            String[] on = split(need(p.repo, "repo"));
            return api("POST", "/v1/repos/import", obj(
                "owner", on[0], "name", on[1], "git", need(p.url, "url"),
                "visibility", or(p.visibility, "private"),
                "description", or(p.description, "")));
        }

        /** {from, state: queued|running|done|failed, detail?} for an imported repo. */
        public String importStatus(String repo) throws IOException, InterruptedException {
            return api("GET", path(repo, "/import"), null);
        }

        // branches
        public String branches(String repo) throws IOException, InterruptedException {
            return api("GET", path(repo, "/branches"), null);
        }

        /** {@code oid} is the head the caller saw; the delete is refused if the branch moved. */
        public String deleteBranch(DeleteBranch p) throws IOException, InterruptedException {
            String repo = need(p.repo, "repo"), branch = need(p.branch, "branch"), oid = need(p.oid, "oid");
            return api("DELETE", path(repo, "/branches/" + seg(branch) + "?oid=" + seg(oid)), null);
        }

        // protection
        public String protection(String repo) throws IOException, InterruptedException {
            return api("GET", path(repo, "/protection"), null);
        }

        public String protect(ProtectBranch p) throws IOException, InterruptedException {
            String repo = need(p.repo, "repo"), pattern = need(p.pattern, "pattern");
            return api("POST", path(repo, "/protection"),
                "{\"pattern\":" + quote(pattern) + ",\"remove\":" + p.remove
                    + ",\"no_force\":" + p.noForce + ",\"no_delete\":" + p.noDelete + "}");
        }

        // pull requests
        /** Unset filters are the server's defaults; with neither set this is a bare listing. */
        public String pulls(ListPulls p) throws IOException, InterruptedException {
            return api("GET", path(need(p.repo, "repo"), "/pulls"
                + query("state", p.state, "limit", p.limit == null ? null : p.limit.toString())), null);
        }

        public String pull(GetPull p) throws IOException, InterruptedException {
            String repo = need(p.repo, "repo");
            return api("GET", path(repo, "/pulls/" + need(p.number, "number")), null);
        }

        /**
         * {@code headRepo} (another "owner/name") makes it a cross-repo pull: {@code head} then
         * names a branch of that fork, proposed onto this repo's {@code base}.
         */
        public String openPull(OpenPull p) throws IOException, InterruptedException {
            String repo = need(p.repo, "repo");
            String title = need(p.title, "title"), head = need(p.head, "head"), base = need(p.base, "base");
            if (p.headRepo != null) split(p.headRepo);
            return api("POST", path(repo, "/pulls"), obj(
                "title", title, "head", head, "base", base,
                "description", or(p.description, ""),
                "head_repo", p.headRepo));
        }

        public String comment(CommentPull p) throws IOException, InterruptedException {
            String repo = need(p.repo, "repo");
            long number = need(p.number, "number");
            return api("POST", path(repo, "/pulls/" + number + "/comments"), obj("body", need(p.body, "body")));
        }

        /** The merge is a job (202); strategy is fast-forward (the default), squash, merge or rebase. */
        public String merge(MergePull p) throws IOException, InterruptedException {
            String repo = need(p.repo, "repo");
            long number = need(p.number, "number");
            return api("POST", path(repo, "/pulls/" + number + "/merge?strategy="
                + seg(or(p.strategy, "fast-forward"))), null);
        }

        public String close(ClosePull p) throws IOException, InterruptedException {
            String repo = need(p.repo, "repo");
            return api("POST", path(repo, "/pulls/" + need(p.number, "number") + "/close"), null);
        }

        // commits
        public String compare(CompareRefs p) throws IOException, InterruptedException {
            String repo = need(p.repo, "repo"), base = need(p.base, "base"), head = need(p.head, "head");
            return api("GET", path(repo, "/compare?base=" + seg(base) + "&head=" + seg(head)), null);
        }

        /** One commit of file changes onto {@code branch}, or onto a new {@code newBranch} cut from it. */
        public String commit(CommitChanges p) throws IOException, InterruptedException {
            String repo = need(p.repo, "repo");
            return api("POST", path(repo, "/commits"), new Json()
                .str("branch", need(p.branch, "branch"))
                .str("message", need(p.message, "message"))
                .raw("changes", need(p.changes, "changes"))
                .str("expect", p.expect)
                .str("newBranch", p.newBranch)
                .toString());
        }

        public String signature(GetSignature p) throws IOException, InterruptedException {
            String repo = need(p.repo, "repo");
            return api("GET", path(repo, "/commits/" + seg(need(p.sha, "sha")) + "/signature"), null);
        }

        private String path(String repo, String tail) {
            String[] on = split(repo);
            return "/v1/repos/" + seg(on[0]) + "/" + seg(on[1]) + tail;
        }
    }

    // ── code repo params ───────────────────────────────────────────────────

    /** {@link CodeRepos#create}: visibility defaults to private, description to empty. */
    public static final class CreateRepo {
        private String repo, visibility, description;

        public CreateRepo repo(String v) { repo = v; return this; }
        public CreateRepo visibility(String v) { visibility = v; return this; }
        public CreateRepo description(String v) { description = v; return this; }
    }

    /** {@link CodeRepos#update}: only the fields set are changed. */
    public static final class UpdateRepo {
        private String repo, description, visibility;

        public UpdateRepo repo(String v) { repo = v; return this; }
        public UpdateRepo description(String v) { description = v; return this; }
        public UpdateRepo visibility(String v) { visibility = v; return this; }
    }

    /** {@link CodeRepos#fork}: {@code repo} is the source, {@code to} the new "owner/name". */
    public static final class ForkRepo {
        private String repo, to, visibility, description;

        public ForkRepo repo(String v) { repo = v; return this; }
        public ForkRepo to(String v) { to = v; return this; }
        public ForkRepo visibility(String v) { visibility = v; return this; }
        public ForkRepo description(String v) { description = v; return this; }
    }

    /** {@link CodeRepos#importGit}: {@code url} is sent as the wire field {@code git}. */
    public static final class ImportRepo {
        private String repo, url, visibility, description;

        public ImportRepo repo(String v) { repo = v; return this; }
        public ImportRepo url(String v) { url = v; return this; }
        public ImportRepo visibility(String v) { visibility = v; return this; }
        public ImportRepo description(String v) { description = v; return this; }
    }

    public static final class DeleteBranch {
        private String repo, branch, oid;

        public DeleteBranch repo(String v) { repo = v; return this; }
        public DeleteBranch branch(String v) { branch = v; return this; }
        public DeleteBranch oid(String v) { oid = v; return this; }
    }

    /** {@link CodeRepos#protect}: {@code remove(true)} lifts the rule instead of adding it. */
    public static final class ProtectBranch {
        private String repo, pattern;
        private boolean remove;
        // Both on by default, as in every other SDK: a protected branch refuses force-pushes and deletes.
        private boolean noForce = true, noDelete = true;

        public ProtectBranch repo(String v) { repo = v; return this; }
        public ProtectBranch pattern(String v) { pattern = v; return this; }
        public ProtectBranch remove(boolean v) { remove = v; return this; }
        public ProtectBranch noForce(boolean v) { noForce = v; return this; }
        public ProtectBranch noDelete(boolean v) { noDelete = v; return this; }
    }

    public static final class ListPulls {
        private String repo, state;
        private Integer limit;

        public ListPulls repo(String v) { repo = v; return this; }
        public ListPulls state(String v) { state = v; return this; }
        public ListPulls limit(int v) { limit = v; return this; }
    }

    public static final class GetPull {
        private String repo;
        private Long number;

        public GetPull repo(String v) { repo = v; return this; }
        public GetPull number(long v) { number = v; return this; }
    }

    public static final class OpenPull {
        private String repo, title, head, base, headRepo, description;

        public OpenPull repo(String v) { repo = v; return this; }
        public OpenPull title(String v) { title = v; return this; }
        public OpenPull head(String v) { head = v; return this; }
        public OpenPull base(String v) { base = v; return this; }
        public OpenPull headRepo(String v) { headRepo = v; return this; }
        public OpenPull description(String v) { description = v; return this; }
    }

    public static final class CommentPull {
        private String repo, body;
        private Long number;

        public CommentPull repo(String v) { repo = v; return this; }
        public CommentPull number(long v) { number = v; return this; }
        public CommentPull body(String v) { body = v; return this; }
    }

    public static final class MergePull {
        private String repo, strategy;
        private Long number;

        public MergePull repo(String v) { repo = v; return this; }
        public MergePull number(long v) { number = v; return this; }
        public MergePull strategy(String v) { strategy = v; return this; }
    }

    public static final class ClosePull {
        private String repo;
        private Long number;

        public ClosePull repo(String v) { repo = v; return this; }
        public ClosePull number(long v) { number = v; return this; }
    }

    public static final class CompareRefs {
        private String repo, base, head;

        public CompareRefs repo(String v) { repo = v; return this; }
        public CompareRefs base(String v) { base = v; return this; }
        public CompareRefs head(String v) { head = v; return this; }
    }

    /**
     * {@link CodeRepos#commit}: {@code changes} is the raw JSON array, each change
     * {@code {"path", "contentBase64"?, "executable"?, "delete"?}}; {@code expect} is the head
     * the caller saw, refused if the branch moved.
     */
    public static final class CommitChanges {
        private String repo, branch, message, changes, expect, newBranch;

        public CommitChanges repo(String v) { repo = v; return this; }
        public CommitChanges branch(String v) { branch = v; return this; }
        public CommitChanges message(String v) { message = v; return this; }
        public CommitChanges changes(String changesJson) { changes = changesJson; return this; }
        public CommitChanges expect(String v) { expect = v; return this; }
        public CommitChanges newBranch(String v) { newBranch = v; return this; }
    }

    public static final class GetSignature {
        private String repo, sha;

        public GetSignature repo(String v) { repo = v; return this; }
        public GetSignature sha(String v) { sha = v; return this; }
    }

    /**
     * Model and dataset repositories, on the hub listener (Hugging Face Hub API).
     * Management level: create, read, refs, visibility, delete — files move with
     * git/huggingface_hub against the same hub URL. Every {@code repo} is one
     * "owner/name" string, exactly as in {@link CodeRepos}.
     */
    public class HubRepos {
        private final String kind;
        private final String plural;

        HubRepos(String kind, String plural) {
            this.kind = kind;
            this.plural = plural;
        }

        /** Private unless told otherwise; sent explicitly rather than trusting a server default. */
        public String create(CreateHubRepo p) throws IOException, InterruptedException {
            String[] on = split(need(p.repo, "repo"));
            return hub("POST", "/api/repos/create",
                "{\"name\":" + quote(on[1]) + ",\"organization\":" + quote(on[0])
                    + ",\"type\":" + quote(kind) + ",\"private\":" + p.privateRepo + "}");
        }

        public String get(String repo) throws IOException, InterruptedException {
            return hub("GET", path(repo, ""), null);
        }

        /** Branches and tags, as the hub spells them. */
        public String refs(String repo) throws IOException, InterruptedException {
            return hub("GET", path(repo, "/refs"), null);
        }

        public String setPrivate(SetPrivate p) throws IOException, InterruptedException {
            String repo = need(p.repo, "repo");
            return hub("PUT", path(repo, "/settings"), "{\"private\":" + need(p.privateRepo, "privateRepo") + "}");
        }

        public String delete(String repo) throws IOException, InterruptedException {
            String[] on = split(repo);
            return hub("DELETE", "/api/repos/delete",
                obj("name", on[1], "organization", on[0], "type", kind));
        }

        /**
         * A new repo whose contents — large files included, through git-lfs — are fetched from a
         * PUBLIC git remote; {@code url} is a strict "https://" URL (the Hugging Face Hub is one
         * host among any). Answers 202; poll {@link #importStatus}.
         */
        public String importGit(ImportHubRepo p) throws IOException, InterruptedException {
            String repo = need(p.repo, "repo");
            return hub("POST", path(repo, "/import"),
                "{\"from\":" + quote(need(p.url, "url")) + ",\"private\":" + p.privateRepo + "}");
        }

        /** {from, state: queued|running|done|failed, detail?} for an imported repo. */
        public String importStatus(String repo) throws IOException, InterruptedException {
            return hub("GET", path(repo, "/import"), null);
        }

        private String path(String repo, String tail) {
            String[] on = split(repo);
            return "/api/" + plural + "/" + seg(on[0]) + "/" + seg(on[1]) + tail;
        }
    }

    // ── hub repo params (`private` is a Java keyword, so its setter is privateRepo) ──

    public static final class CreateHubRepo {
        private String repo;
        private boolean privateRepo = true;

        public CreateHubRepo repo(String v) { repo = v; return this; }
        public CreateHubRepo privateRepo(boolean v) { privateRepo = v; return this; }
    }

    public static final class SetPrivate {
        private String repo;
        private Boolean privateRepo;

        public SetPrivate repo(String v) { repo = v; return this; }
        public SetPrivate privateRepo(boolean v) { privateRepo = v; return this; }
    }

    /** {@link HubRepos#importGit}: the hub's import route takes {@code url} as {@code from}. */
    public static final class ImportHubRepo {
        private String repo, url;
        private boolean privateRepo = true;

        public ImportHubRepo repo(String v) { repo = v; return this; }
        public ImportHubRepo url(String v) { url = v; return this; }
        public ImportHubRepo privateRepo(boolean v) { privateRepo = v; return this; }
    }

    /**
     * Btrfs-backed dev workspaces, over {@code /v1/workspaces}. A workspace is named by ONE
     * string — {@code "name"} for the caller's own, {@code "team/name"} for a team's — or by its
     * id; pushes cut snapshots, kept on the workspace's VOLUME — see {@link Volumes} for
     * history, restore targets and deletion.
     */
    public class Workspaces {
        /**
         * {@code workspace} is split into the body's {@code name} and {@code team}; {@code repo}
         * seeds it from a PLATFORM repo (with {@code branch}); {@code region} matters only on a
         * multi-region fleet — absent, the deployment's one is used.
         */
        public String create(CreateWorkspace p) throws IOException, InterruptedException {
            String[] on = owned(need(p.workspace, "workspace"));
            return api("POST", "/v1/workspaces", new Json()
                .str("name", on[1])
                .num("quota_gb", need(p.quotaGb, "quotaGb"))
                .str("team", on[0])
                .str("image", p.image)
                .str("repo", p.repo)
                .str("branch", p.branch)
                .raw("packages", strings(p.packages == null ? List.of() : p.packages))
                .str("region", p.region)
                .toString());
        }

        /** The caller's workspaces; a non-null team scopes to their workspaces in that team. */
        public String list(String team) throws IOException, InterruptedException {
            return api("GET", team == null ? "/v1/workspaces" : "/v1/workspaces?team=" + seg(team), null);
        }

        public String get(String workspace) throws IOException, InterruptedException {
            return api("GET", path(workspace, ""), null);
        }

        /** Runs the worktree finalizer: the volume survives while a pushed snapshot references it. */
        public String delete(String workspace) throws IOException, InterruptedException {
            return api("DELETE", path(workspace, ""), null);
        }

        public String start(String workspace) throws IOException, InterruptedException {
            return api("POST", path(workspace, "/start"), null);
        }

        /** A stop cuts a sync point and tears the pod down — seconds, never waiting on a replica. */
        public String stop(String workspace) throws IOException, InterruptedException {
            return api("POST", path(workspace, "/stop"), null);
        }

        /** Cut a SNAPSHOT of the working copy — the one durable, restorable state. */
        public String push(PushWorkspace p) throws IOException, InterruptedException {
            return api("POST", path(need(p.workspace, "workspace"), "/push"),
                p.message == null ? null : obj("message", p.message));
        }

        /**
         * A new workspace {@code to} from this one's state, cut at the moment of the request; the
         * answer's {@code based_on} names exactly what it grafted onto.
         */
        public String clone(CloneWorkspace p) throws IOException, InterruptedException {
            String ws = need(p.workspace, "workspace");
            return api("POST", path(ws, "/clone"), obj("name", need(p.to, "to"), "task", p.task));
        }

        /**
         * A new workspace {@code to} grafted onto a PUSHED snapshot — also one whose workspace is
         * long gone. Unset overrides mean "whatever the snapshot froze".
         */
        public String restore(RestoreWorkspace p) throws IOException, InterruptedException {
            String snapshot = need(p.snapshot, "snapshot"), to = need(p.to, "to");
            return api("POST", "/v1/workspaces/restore", new Json()
                .str("name", to)
                .str("snapshot_id", snapshot)
                .str("image", p.image)
                .raw("packages", p.packages == null ? null : strings(p.packages))
                .num("quota_gb", p.quotaGb)
                .toString());
        }

        /** Replace the package list; pinned entries are locked before anything is written. */
        public String setPackages(SetPackages p) throws IOException, InterruptedException {
            String ws = need(p.workspace, "workspace");
            return api("PATCH", path(ws, ""), "{\"packages\":" + strings(need(p.packages, "packages")) + "}");
        }

        /** Re-resolve every pinned package — the only call that moves existing locks. */
        public String updatePackages(String workspace) throws IOException, InterruptedException {
            return api("POST", path(workspace, "/packages/update"), null);
        }

        /** The workspace's tool-server address and token — answered to the owner alone. */
        public String tools(String workspace) throws IOException, InterruptedException {
            return api("GET", path(workspace, "/tools"), null);
        }

        /**
         * An {@link Ide} bound to this workspace's tool server. The address is a pod IP:
         * reachable from inside the fleet (a bench); from a laptop, tunnel with
         * {@code kl-connect ws ide} and use
         * {@code new Ide(new IdeOptions().url("http://127.0.0.1:7788").token(token))}.
         */
        public Ide ide(String workspace) throws IOException, InterruptedException {
            String doc = tools(workspace);
            String address = field(doc, "address");
            if (address == null) {
                throw new ApiException(502, "the tools answer named no address");
            }
            return new Ide(new IdeOptions().url("http://" + address).token(field(doc, "token")));
        }

        private String path(String workspace, String tail) {
            return at("/v1/workspaces/", workspace, tail);
        }
    }

    // ── workspace params ───────────────────────────────────────────────────

    /** {@link Workspaces#create}: {@code workspace} and {@code quotaGb} are required. */
    public static final class CreateWorkspace {
        private String workspace, image, repo, branch, region;
        private Long quotaGb;
        private List<String> packages;

        public CreateWorkspace workspace(String v) { workspace = v; return this; }
        public CreateWorkspace quotaGb(long v) { quotaGb = v; return this; }
        public CreateWorkspace image(String v) { image = v; return this; }
        public CreateWorkspace repo(String v) { repo = v; return this; }
        public CreateWorkspace branch(String v) { branch = v; return this; }
        /** nixpkgs attributes, optionally pinned ({@code attr@version}). */
        public CreateWorkspace packages(String... v) { packages = Arrays.asList(v); return this; }
        public CreateWorkspace region(String v) { region = v; return this; }
    }

    public static final class PushWorkspace {
        private String workspace, message;

        public PushWorkspace workspace(String v) { workspace = v; return this; }
        public PushWorkspace message(String v) { message = v; return this; }
    }

    public static final class CloneWorkspace {
        private String workspace, to, task;

        public CloneWorkspace workspace(String v) { workspace = v; return this; }
        public CloneWorkspace to(String v) { to = v; return this; }
        public CloneWorkspace task(String v) { task = v; return this; }
    }

    /** {@link Workspaces#restore}: {@code snapshot} is sent as {@code snapshot_id}, {@code to} as {@code name}. */
    public static final class RestoreWorkspace {
        private String snapshot, to, image;
        private List<String> packages;
        private Long quotaGb;

        public RestoreWorkspace snapshot(String v) { snapshot = v; return this; }
        public RestoreWorkspace to(String v) { to = v; return this; }
        public RestoreWorkspace image(String v) { image = v; return this; }
        public RestoreWorkspace packages(String... v) { packages = Arrays.asList(v); return this; }
        public RestoreWorkspace quotaGb(long v) { quotaGb = v; return this; }
    }

    /** {@link Workspaces#setPackages}: the WHOLE new list; {@code packages()} with none clears it. */
    public static final class SetPackages {
        private String workspace;
        private List<String> packages;

        public SetPackages workspace(String v) { workspace = v; return this; }
        public SetPackages packages(String... v) { packages = Arrays.asList(v); return this; }
    }

    /** Where a tool server lives: {@code url} is required, {@code token} null means none. */
    public static final class IdeOptions {
        private String url, token;

        public IdeOptions url(String v) { url = v; return this; }
        public IdeOptions token(String v) { token = v; return this; }
    }

    /**
     * A workspace's tool server ({@code kl ide serve}): the full tool set over plain HTTP —
     * {@link #list} answers every tool with its JSON schema (the authority on arguments) and
     * {@link #call} runs one with a raw JSON args object; the typed methods cover every tool,
     * and an unset optional means "the tool's own default".
     */
    public static class Ide {
        private final String url;
        private final String token; // null = none
        private final HttpClient http = HttpClient.newBuilder().connectTimeout(Duration.ofSeconds(30)).build();

        public Ide(IdeOptions opts) {
            this.url = need(opts.url, "url").replaceAll("/+$", "");
            this.token = opts.token;
        }

        /** Every tool, with its JSON schema. */
        public String list() throws IOException, InterruptedException {
            return send("GET", "/tools", null);
        }

        /** Run one tool by name; {@code argsJson} is the JSON object its schema names. */
        public String call(String tool, String argsJson) throws IOException, InterruptedException {
            return send("POST", "/tools/" + seg(tool), argsJson == null ? "{}" : argsJson);
        }

        // ── files ─────────────────────────────────────────────────────────

        /** One file, the whole of it. */
        public String read(String path) throws IOException, InterruptedException {
            return call("read", obj("path", path));
        }

        public String read(ReadFiles p) throws IOException, InterruptedException {
            return call("read", new Json().str("path", p.path)
                .raw("paths", p.paths == null ? null : strings(p.paths))
                .num("offset", p.offset).num("limit", p.limit).toString());
        }

        public String write(WriteFile p) throws IOException, InterruptedException {
            return call("write", obj("path", need(p.path, "path"), "content", need(p.content, "content")));
        }

        /** All or nothing across the whole call: {@code path} + {@code edits}, or {@code files}. */
        public String edit(EditFiles p) throws IOException, InterruptedException {
            return call("edit", new Json().str("path", p.path).raw("edits", p.edits).raw("files", p.files).toString());
        }

        /** Apply a unified diff; it never applies in part. An unset cwd means the working directory. */
        public String patch(ApplyPatch p) throws IOException, InterruptedException {
            return call("patch", obj("diff", need(p.diff, "diff"), "cwd", p.cwd));
        }

        public String glob(GlobFiles p) throws IOException, InterruptedException {
            return call("glob", obj("pattern", need(p.pattern, "pattern"), "cwd", p.cwd));
        }

        public String grep(GrepFiles p) throws IOException, InterruptedException {
            return call("grep", new Json().str("pattern", need(p.pattern, "pattern")).str("cwd", p.cwd)
                .str("glob", p.glob).str("mode", p.mode).num("context", p.context)
                .bool("ignore_case", p.ignoreCase).num("max", p.max).toString());
        }

        // ── commands and processes ────────────────────────────────────────

        /** Run a shell command and wait. */
        public String exec(String cmd) throws IOException, InterruptedException {
            return call("exec", obj("cmd", cmd));
        }

        /** {@code detach(true)} answers {@code {id}} and the command becomes a process. */
        public String exec(ExecCommand p) throws IOException, InterruptedException {
            return call("exec", new Json().raw("cmd", need(p.cmd, "cmd")).str("cwd", p.cwd)
                .raw("env", p.env == null ? null : stringMap(p.env)).num("timeout_ms", p.timeoutMs)
                .num("head", p.head).num("tail", p.tail).bool("quiet", p.quiet).bool("detach", p.detach)
                .toString());
        }

        public String processList() throws IOException, InterruptedException {
            return call("process_list", "{}");
        }

        public String processOutput(GetProcessOutput p) throws IOException, InterruptedException {
            return call("process_output", new Json().str("id", need(p.process, "process"))
                .num("since", p.since).num("since_err", p.sinceErr).toString());
        }

        public String processWrite(WriteProcess p) throws IOException, InterruptedException {
            return call("process_write", obj("id", need(p.process, "process"), "data", need(p.data, "data")));
        }

        /** {@code signal} is TERM (the tool's default) or KILL; it reaches the process group. */
        public String processKill(KillProcess p) throws IOException, InterruptedException {
            return call("process_kill", obj("id", need(p.process, "process"), "signal", p.signal));
        }

        // ── watches ───────────────────────────────────────────────────────

        public String watch(StartWatch p) throws IOException, InterruptedException {
            return call("watch", new Json().raw("paths", p.paths == null ? null : strings(p.paths))
                .raw("cmd", p.cmd).str("pattern", p.pattern).bool("once", p.once).toString());
        }

        public String watchPoll(PollWatch p) throws IOException, InterruptedException {
            return call("watch_poll", new Json().str("id", need(p.watch, "watch")).num("since", p.since).toString());
        }

        public String watchStop(String watch) throws IOException, InterruptedException {
            return call("watch_stop", obj("id", watch));
        }

        // ── containers (the hidden per-owner builder) ─────────────────────

        public String containerBuild(BuildContainer p) throws IOException, InterruptedException {
            return call("container_build", new Json().raw("tags", strings(need(p.tags, "tags")))
                .str("context", p.context).str("file", p.file)
                .raw("build_args", p.buildArgs == null ? null : strings(p.buildArgs))
                .str("platform", p.platform).bool("no_cache", p.noCache).num("timeout_ms", p.timeoutMs)
                .num("head", p.head).num("tail", p.tail).toString());
        }

        /** Copy an image the registry already holds to more tags — there is no local store. */
        public String containerPush(PushContainer p) throws IOException, InterruptedException {
            return call("container_push", new Json().str("src", need(p.src, "src"))
                .raw("dst", strings(need(p.dst, "dst"))).toString());
        }

        private String send(String method, String path, String jsonBody)
                throws IOException, InterruptedException {
            HttpRequest.Builder b = HttpRequest.newBuilder()
                .uri(URI.create(url + path))
                // exec may legitimately run for ten minutes; the request outlasts it.
                .timeout(Duration.ofSeconds(630))
                .header("User-Agent", "kloudlite-sdk-java/" + VERSION)
                .header("Accept", "application/json")
                .method(method, jsonBody == null
                    ? HttpRequest.BodyPublishers.noBody()
                    : HttpRequest.BodyPublishers.ofString(jsonBody));
            if (jsonBody != null) b.header("Content-Type", "application/json");
            if (token != null) b.header("Authorization", "Bearer " + token);
            HttpResponse<String> resp = http.send(b.build(), HttpResponse.BodyHandlers.ofString());
            if (resp.statusCode() < 200 || resp.statusCode() > 299) {
                throw new ApiException(resp.statusCode(), errorText(resp.body()));
            }
            return resp.body();
        }
    }

    // ── tool server params (a process or watch is sent as the wire field `id`) ──

    public static final class ReadFiles {
        private String path;
        private List<String> paths;
        private Long offset, limit;

        public ReadFiles path(String v) { path = v; return this; }
        public ReadFiles paths(String... v) { paths = Arrays.asList(v); return this; }
        public ReadFiles offset(long v) { offset = v; return this; }
        public ReadFiles limit(long v) { limit = v; return this; }
    }

    public static final class WriteFile {
        private String path, content;

        public WriteFile path(String v) { path = v; return this; }
        public WriteFile content(String v) { content = v; return this; }
    }

    /** {@code edits} and {@code files} are raw JSON arrays, as the tool's schema shapes them. */
    public static final class EditFiles {
        private String path, edits, files;

        public EditFiles path(String v) { path = v; return this; }
        public EditFiles edits(String editsJson) { edits = editsJson; return this; }
        public EditFiles files(String filesJson) { files = filesJson; return this; }
    }

    public static final class ApplyPatch {
        private String diff, cwd;

        public ApplyPatch diff(String v) { diff = v; return this; }
        public ApplyPatch cwd(String v) { cwd = v; return this; }
    }

    public static final class GlobFiles {
        private String pattern, cwd;

        public GlobFiles pattern(String v) { pattern = v; return this; }
        public GlobFiles cwd(String v) { cwd = v; return this; }
    }

    public static final class GrepFiles {
        private String pattern, cwd, glob, mode;
        private Long context, max;
        private Boolean ignoreCase;

        public GrepFiles pattern(String v) { pattern = v; return this; }
        public GrepFiles cwd(String v) { cwd = v; return this; }
        public GrepFiles glob(String v) { glob = v; return this; }
        /** content, files or count. */
        public GrepFiles mode(String v) { mode = v; return this; }
        public GrepFiles context(long v) { context = v; return this; }
        public GrepFiles ignoreCase(boolean v) { ignoreCase = v; return this; }
        public GrepFiles max(long v) { max = v; return this; }
    }

    /** {@code cmd} is a shell string or an argv list — the tool takes either. */
    public static final class ExecCommand {
        private String cmd, cwd; // cmd held as JSON: a string or an array
        private Map<String, String> env;
        private Long timeoutMs, head, tail;
        private Boolean quiet, detach;

        public ExecCommand cmd(String shell) { cmd = shell == null ? null : quote(shell); return this; }
        public ExecCommand cmd(List<String> argv) { cmd = argv == null ? null : strings(argv); return this; }
        public ExecCommand cwd(String v) { cwd = v; return this; }
        public ExecCommand env(Map<String, String> v) { env = v; return this; }
        public ExecCommand timeoutMs(long v) { timeoutMs = v; return this; }
        public ExecCommand head(long v) { head = v; return this; }
        public ExecCommand tail(long v) { tail = v; return this; }
        public ExecCommand quiet(boolean v) { quiet = v; return this; }
        public ExecCommand detach(boolean v) { detach = v; return this; }
    }

    /** {@code since}/{@code sinceErr} are byte offsets into stdout/stderr; pass back {@code next}/{@code next_err}. */
    public static final class GetProcessOutput {
        private String process;
        private Long since, sinceErr;

        public GetProcessOutput process(String v) { process = v; return this; }
        public GetProcessOutput since(long v) { since = v; return this; }
        public GetProcessOutput sinceErr(long v) { sinceErr = v; return this; }
    }

    public static final class WriteProcess {
        private String process, data;

        public WriteProcess process(String v) { process = v; return this; }
        public WriteProcess data(String v) { data = v; return this; }
    }

    public static final class KillProcess {
        private String process, signal;

        public KillProcess process(String v) { process = v; return this; }
        public KillProcess signal(String v) { signal = v; return this; }
    }

    /** Watch {@code paths}, or a {@code cmd}'s output for {@code pattern}; {@code once} ends at the first hit. */
    public static final class StartWatch {
        private List<String> paths;
        private String cmd, pattern; // cmd held as JSON: a string or an array
        private Boolean once;

        public StartWatch paths(String... v) { paths = Arrays.asList(v); return this; }
        public StartWatch cmd(String shell) { cmd = shell == null ? null : quote(shell); return this; }
        public StartWatch cmd(List<String> argv) { cmd = argv == null ? null : strings(argv); return this; }
        public StartWatch pattern(String v) { pattern = v; return this; }
        public StartWatch once(boolean v) { once = v; return this; }
    }

    public static final class PollWatch {
        private String watch;
        private Long since;

        public PollWatch watch(String v) { watch = v; return this; }
        public PollWatch since(long v) { since = v; return this; }
    }

    /** Each of {@code tags} is {@code name[:tag]}; {@code buildArgs} are {@code KEY=VALUE}. */
    public static final class BuildContainer {
        private List<String> tags, buildArgs;
        private String context, file, platform;
        private Boolean noCache;
        private Long timeoutMs, head, tail;

        public BuildContainer tags(String... v) { tags = Arrays.asList(v); return this; }
        public BuildContainer context(String v) { context = v; return this; }
        public BuildContainer file(String v) { file = v; return this; }
        public BuildContainer buildArgs(String... v) { buildArgs = Arrays.asList(v); return this; }
        public BuildContainer platform(String v) { platform = v; return this; }
        public BuildContainer noCache(boolean v) { noCache = v; return this; }
        public BuildContainer timeoutMs(long v) { timeoutMs = v; return this; }
        public BuildContainer head(long v) { head = v; return this; }
        public BuildContainer tail(long v) { tail = v; return this; }
    }

    public static final class PushContainer {
        private String src;
        private List<String> dst;

        public PushContainer src(String v) { src = v; return this; }
        public PushContainer dst(String... v) { dst = Arrays.asList(v); return this; }
    }

    /**
     * One container of an environment, built fluently and rendered with every field the server
     * requires: {@code new Service().name("db").image("postgres:16").port(5432)
     * .env("POSTGRES_PASSWORD", "dev").mount("pgdata", "/var/lib/postgresql/data")}. A mount's
     * folder is a folder of the environment's own disk.
     */
    public static class Service {
        private String name;
        private String image;
        private final List<String> ports = new ArrayList<>();
        private final List<String> env = new ArrayList<>();
        private final List<String> command = new ArrayList<>();
        private final List<String> mounts = new ArrayList<>();

        public Service name(String v) {
            name = v;
            return this;
        }

        public Service image(String v) {
            image = v;
            return this;
        }

        public Service port(int port) {
            ports.add(Integer.toString(port));
            return this;
        }

        public Service env(String key, String value) {
            env.add(quote(key) + ":" + quote(value));
            return this;
        }

        public Service command(String... argv) {
            for (String a : argv) command.add(quote(a));
            return this;
        }

        public Service mount(String folder, String path) {
            mounts.add(obj("folder", folder, "path", path));
            return this;
        }

        String toJson() {
            return "{\"name\":" + quote(need(name, "name")) + ",\"image\":" + quote(need(image, "image"))
                + ",\"ports\":[" + String.join(",", ports) + "]"
                + ",\"env\":{" + String.join(",", env) + "}"
                + ",\"command\":[" + String.join(",", command) + "]"
                + ",\"mounts\":[" + String.join(",", mounts) + "]}";
        }
    }

    /**
     * Multi-service environments, over {@code /v1/environments}: a set of containers sharing one
     * disk. An environment is named by ONE string — {@code "name"} for the caller's own,
     * {@code "team/name"} for a team's — or by its id ({@code env-…}).
     */
    public class Environments {
        /**
         * An unset {@code quotaGb} means the default. {@code snapshot} grows it from a pushed
         * snapshot — also one of a deleted environment — and then no {@code services} means the
         * ones that snapshot froze.
         */
        public String create(CreateEnvironment p) throws IOException, InterruptedException {
            return api("POST", "/v1/environments",
                body(need(p.environment, "environment"), p.quotaGb, p.snapshot, p.services, true));
        }

        /** The caller's environments and their teams'; a non-null owner narrows to one handle or team. */
        public String list(String owner) throws IOException, InterruptedException {
            return api("GET", owner == null ? "/v1/environments" : "/v1/environments?owner=" + seg(owner), null);
        }

        public String get(String environment) throws IOException, InterruptedException {
            return api("GET", path(environment, ""), null);
        }

        /** Replace the WHOLE service list. A removed service keeps its folders, so re-adding finds its data. */
        public String setServices(SetServices p) throws IOException, InterruptedException {
            String env = need(p.environment, "environment");
            return api("PATCH", path(env, ""), "{\"services\":" + array(need(p.services, "services")) + "}");
        }

        public String start(String environment) throws IOException, InterruptedException {
            return api("POST", path(environment, "/start"), null);
        }

        public String stop(String environment) throws IOException, InterruptedException {
            return api("POST", path(environment, "/stop"), null);
        }

        /** Reset the idle clock — call it periodically while something depends on the services. */
        public String hold(String environment) throws IOException, InterruptedException {
            return api("POST", path(environment, "/hold"), null);
        }

        /** The volume survives while a pushed snapshot references it. */
        public String delete(String environment) throws IOException, InterruptedException {
            return api("DELETE", path(environment, ""), null);
        }

        /** Cut a SNAPSHOT of every service's data — the one durable, restorable state. */
        public String push(PushEnvironment p) throws IOException, InterruptedException {
            return api("POST", path(need(p.environment, "environment"), "/push"),
                p.message == null ? null : obj("message", p.message));
        }

        /**
         * A new environment {@code to} ("name" or "team/name"): a copy of the live disk, or of one
         * of this environment's snapshots when {@code snapshot} is set.
         */
        public String clone(CloneEnvironment p) throws IOException, InterruptedException {
            String env = need(p.environment, "environment");
            return api("POST", path(env, "/clone"), body(need(p.to, "to"), p.quotaGb, p.snapshot, p.services, false));
        }

        /**
         * Put THIS environment back to one of its own snapshots, services and data (202). A
         * persistent service the snapshot lacks is a 409 naming it; list it in {@code discard}
         * to accept losing its data.
         */
        public String restore(RestoreEnvironment p) throws IOException, InterruptedException {
            String env = need(p.environment, "environment"), snapshot = need(p.snapshot, "snapshot");
            return api("POST", path(env, "/restore"),
                "{\"snapshot_id\":" + quote(snapshot) + ",\"discard\":" + strings(p.discard) + "}");
        }

        private String path(String environment, String tail) {
            return at("/v1/environments/", environment, tail);
        }

        private String body(String env, Long quotaGb, String snapshot, List<Service> services, boolean always) {
            String[] on = owned(env);
            String b = obj("name", on[1], "owner", on[0], "snapshot_id", snapshot);
            StringBuilder out = new StringBuilder(b.substring(0, b.length() - 1));
            if (quotaGb != null) out.append(",\"quota_gb\":").append(quotaGb);
            if (always || !services.isEmpty()) out.append(",\"services\":").append(array(services));
            return out.append('}').toString();
        }

        private String array(List<Service> services) {
            List<String> parts = new ArrayList<>();
            for (Service sv : services) parts.add(sv.toJson());
            return "[" + String.join(",", parts) + "]";
        }
    }

    // ── environment params ─────────────────────────────────────────────────

    public static final class CreateEnvironment {
        private String environment, snapshot;
        private List<Service> services = List.of();
        private Long quotaGb;

        public CreateEnvironment environment(String v) { environment = v; return this; }
        public CreateEnvironment services(Service... v) { services = Arrays.asList(v); return this; }
        public CreateEnvironment quotaGb(long v) { quotaGb = v; return this; }
        public CreateEnvironment snapshot(String v) { snapshot = v; return this; }
    }

    /** {@link Environments#setServices}: {@code services()} with none empties the environment. */
    public static final class SetServices {
        private String environment;
        private List<Service> services;

        public SetServices environment(String v) { environment = v; return this; }
        public SetServices services(Service... v) { services = Arrays.asList(v); return this; }
    }

    public static final class PushEnvironment {
        private String environment, message;

        public PushEnvironment environment(String v) { environment = v; return this; }
        public PushEnvironment message(String v) { message = v; return this; }
    }

    public static final class CloneEnvironment {
        private String environment, to, snapshot;
        private List<Service> services = List.of();
        private Long quotaGb;

        public CloneEnvironment environment(String v) { environment = v; return this; }
        public CloneEnvironment to(String v) { to = v; return this; }
        public CloneEnvironment snapshot(String v) { snapshot = v; return this; }
        public CloneEnvironment services(Service... v) { services = Arrays.asList(v); return this; }
        public CloneEnvironment quotaGb(long v) { quotaGb = v; return this; }
    }

    public static final class RestoreEnvironment {
        private String environment, snapshot;
        private List<String> discard = List.of();

        public RestoreEnvironment environment(String v) { environment = v; return this; }
        public RestoreEnvironment snapshot(String v) { snapshot = v; return this; }
        /** Persistent services whose data the caller accepts losing. */
        public RestoreEnvironment discard(String... services) { discard = Arrays.asList(services); return this; }
    }

    /**
     * Working sessions, over {@code /v1/sessions}: some of YOUR workspaces in one team, connected
     * to one environment. Many people's sessions connect to the same environment and collaborate
     * through it; an intercept a session takes is environment-wide, and the session owns it — only
     * it can release it, and removing the workspace or deleting the session releases it. A session
     * is named like an environment ({@code "name"} or {@code "team/name"}) or by its id; a
     * workspace by its id, name or {@code "team/name"}, sent verbatim.
     */
    public class Sessions {
        /** {@code environment} must be the session's team's: an id, a name or "team/name". */
        public String create(CreateSession p) throws IOException, InterruptedException {
            String[] on = owned(need(p.session, "session"));
            return api("POST", "/v1/sessions",
                obj("name", on[1], "team", on[0], "environment", need(p.environment, "environment")));
        }

        public String list(String team) throws IOException, InterruptedException {
            return api("GET", team == null ? "/v1/sessions" : "/v1/sessions?team=" + seg(team), null);
        }

        /** The session with its workspaces and the intercepts it holds. */
        public String get(String session) throws IOException, InterruptedException {
            return api("GET", path(session, ""), null);
        }

        /** Releases its intercepts and lets its workspaces go — never deletes a workspace. */
        public String delete(String session) throws IOException, InterruptedException {
            return api("DELETE", path(session, ""), null);
        }

        /** A workspace is in at most one session; while in one it resolves that session's environment. */
        public String add(AddToSession p) throws IOException, InterruptedException {
            String session = need(p.session, "session");
            return api("POST", path(session, "/workspaces"), obj("workspace", need(p.workspace, "workspace")));
        }

        /** Releases whatever the workspace intercepts in this session first. */
        public String remove(RemoveFromSession p) throws IOException, InterruptedException {
            String session = need(p.session, "session");
            return api("DELETE", path(session, "/workspaces/" + seg(need(p.workspace, "workspace"))), null);
        }

        /**
         * Send everything that calls {@code service} in the environment to {@code workspace} (a
         * member). {@code ports} maps a service port to the workspace port answering it.
         */
        public String intercept(InterceptService p) throws IOException, InterruptedException {
            String session = need(p.session, "session");
            String service = need(p.service, "service"), workspace = need(p.workspace, "workspace");
            List<String> pm = new ArrayList<>();
            if (p.ports != null) {
                for (Map.Entry<Integer, Integer> e : p.ports.entrySet()) {
                    pm.add("{\"service\":" + e.getKey() + ",\"workspace\":" + e.getValue() + "}");
                }
            }
            String b = obj("service", service, "workspace", workspace);
            return api("POST", path(session, "/intercepts"),
                b.substring(0, b.length() - 1) + ",\"ports\":[" + String.join(",", pm) + "]}");
        }

        public String release(ReleaseService p) throws IOException, InterruptedException {
            String session = need(p.session, "session");
            return api("DELETE", path(session, "/intercepts/" + seg(need(p.service, "service"))), null);
        }

        private String path(String session, String tail) {
            return at("/v1/sessions/", session, tail);
        }
    }

    // ── session params ─────────────────────────────────────────────────────

    public static final class CreateSession {
        private String session, environment;

        public CreateSession session(String v) { session = v; return this; }
        public CreateSession environment(String v) { environment = v; return this; }
    }

    public static final class AddToSession {
        private String session, workspace;

        public AddToSession session(String v) { session = v; return this; }
        public AddToSession workspace(String v) { workspace = v; return this; }
    }

    public static final class RemoveFromSession {
        private String session, workspace;

        public RemoveFromSession session(String v) { session = v; return this; }
        public RemoveFromSession workspace(String v) { workspace = v; return this; }
    }

    /** {@code ports} keeps the map's own iteration order on the wire — use a LinkedHashMap to fix it. */
    public static final class InterceptService {
        private String session, service, workspace;
        private Map<Integer, Integer> ports;

        public InterceptService session(String v) { session = v; return this; }
        public InterceptService service(String v) { service = v; return this; }
        public InterceptService workspace(String v) { workspace = v; return this; }
        public InterceptService ports(Map<Integer, Integer> v) { ports = v; return this; }
    }

    public static final class ReleaseService {
        private String session, service;

        public ReleaseService session(String v) { session = v; return this; }
        public ReleaseService service(String v) { service = v; return this; }
    }

    /**
     * The btrfs volumes behind workspaces and environments: snapshot history, restore targets,
     * and the two deletes — over {@code /v1/volumes}.
     */
    public class Volumes {
        /** Either filter may be unset: kind is "workspace" or "environment", owner a handle or team slug. */
        public String list(ListVolumes p) throws IOException, InterruptedException {
            return api("GET", "/v1/volumes" + query("kind", p.kind, "owner", p.owner), null);
        }

        /** The chain of PUSHED snapshots, newest first — each row a restore target. */
        public String history(String volume) throws IOException, InterruptedException {
            return api("GET", "/v1/volumes/" + seg(volume) + "/history", null);
        }

        public String refs(String volume) throws IOException, InterruptedException {
            return api("GET", "/v1/volumes/" + seg(volume) + "/refs", null);
        }

        /**
         * Refused (409) for a sync point or a running worktree's base; deleting a detached
         * volume's last snapshot deletes the volume.
         */
        public String deleteSnapshot(DeleteSnapshot p) throws IOException, InterruptedException {
            String volume = need(p.volume, "volume"), snapshot = need(p.snapshot, "snapshot");
            return api("DELETE", "/v1/volumes/" + seg(volume) + "/snapshots/" + seg(snapshot), null);
        }

        /** Takes a DETACHED volume with all its snapshots; refused while a working copy remains. */
        public String delete(String volume) throws IOException, InterruptedException {
            return api("DELETE", "/v1/volumes/" + seg(volume), null);
        }
    }

    // ── volume params ──────────────────────────────────────────────────────

    public static final class ListVolumes {
        private String kind, owner;

        public ListVolumes kind(String v) { kind = v; return this; }
        public ListVolumes owner(String v) { owner = v; return this; }
    }

    public static final class DeleteSnapshot {
        private String volume, snapshot;

        public DeleteSnapshot volume(String v) { volume = v; return this; }
        public DeleteSnapshot snapshot(String v) { snapshot = v; return this; }
    }

    // ── small helpers, so a one-file SDK needs no JSON dependency ──────────

    /** The one required-field check: refused before any request is built, let alone sent. */
    static <T> T need(T v, String field) {
        if (v == null) throw new IllegalArgumentException(field + " is required");
        return v;
    }

    static String or(String v, String fallback) {
        return v == null ? fallback : v;
    }

    /** "owner/name" → its two halves, refused client-side when malformed. */
    static String[] split(String repo) {
        int at = repo == null ? -1 : repo.indexOf('/');
        if (at <= 0 || at == repo.length() - 1 || repo.indexOf('/', at + 1) >= 0) {
            throw new IllegalArgumentException("a repository is named owner/name, got " + repo);
        }
        return new String[] { repo.substring(0, at), repo.substring(at + 1) };
    }

    /** "name" → {null, name}; "team/name" → {team, name}. */
    static String[] owned(String s) {
        int i = s.lastIndexOf('/');
        if (i < 0) return new String[] {null, s};
        String owner = s.substring(0, i), name = s.substring(i + 1);
        if (owner.isEmpty() || name.isEmpty() || owner.contains("/")) {
            throw new IllegalArgumentException("expected name or team/name, got " + s);
        }
        return new String[] {owner, name};
    }

    /**
     * A workspace, environment or session path: "team/name" travels as segment {@code name} plus
     * {@code ?owner=team}, which the server's {@code Named} extractor folds back together.
     */
    static String at(String prefix, String ident, String tail) {
        String[] on = owned(ident);
        return prefix + seg(on[1]) + tail + (on[0] == null ? "" : "?owner=" + seg(on[0]));
    }

    /** "?k=v&…" from key/value pairs, a null value dropping its pair; "" when none remain. */
    static String query(String... kv) {
        StringBuilder q = new StringBuilder();
        for (int i = 0; i + 1 < kv.length; i += 2) {
            if (kv[i + 1] == null) continue;
            q.append(q.length() == 0 ? '?' : '&').append(seg(kv[i])).append('=').append(seg(kv[i + 1]));
        }
        return q.toString();
    }

    static String seg(String s) {
        return URLEncoder.encode(s, StandardCharsets.UTF_8).replace("+", "%20");
    }

    /** A flat JSON object from key/value pairs; a null value drops its pair. */
    static String obj(String... kv) {
        StringBuilder b = new StringBuilder("{");
        for (int i = 0; i + 1 < kv.length; i += 2) {
            if (kv[i + 1] == null) continue;
            if (b.length() > 1) b.append(',');
            b.append(quote(kv[i])).append(':').append(quote(kv[i + 1]));
        }
        return b.append('}').toString();
    }

    /** {@link #obj} for mixed values: same bytes, a null still drops its pair. */
    static final class Json {
        private final StringBuilder b = new StringBuilder("{");

        Json raw(String key, String json) {
            if (json == null) return this;
            if (b.length() > 1) b.append(',');
            b.append(quote(key)).append(':').append(json);
            return this;
        }

        Json str(String key, String v) {
            return raw(key, v == null ? null : quote(v));
        }

        Json num(String key, Long v) {
            return raw(key, v == null ? null : v.toString());
        }

        Json bool(String key, Boolean v) {
            return raw(key, v == null ? null : v.toString());
        }

        @Override
        public String toString() {
            return b + "}";
        }
    }

    /** A JSON array of strings. */
    static String strings(List<String> xs) {
        List<String> q = new ArrayList<>();
        for (String x : xs) q.add(quote(x));
        return "[" + String.join(",", q) + "]";
    }

    /** A JSON object of string values, in the map's own order. */
    static String stringMap(Map<String, String> m) {
        List<String> q = new ArrayList<>();
        for (Map.Entry<String, String> e : m.entrySet()) q.add(quote(e.getKey()) + ":" + quote(e.getValue()));
        return "{" + String.join(",", q) + "}";
    }

    static String quote(String s) {
        StringBuilder b = new StringBuilder("\"");
        for (char c : s.toCharArray()) {
            switch (c) {
                case '"' -> b.append("\\\"");
                case '\\' -> b.append("\\\\");
                case '\n' -> b.append("\\n");
                case '\r' -> b.append("\\r");
                case '\t' -> b.append("\\t");
                default -> {
                    if (c < 0x20) b.append(String.format("\\u%04x", (int) c));
                    else b.append(c);
                }
            }
        }
        return b.append('"').toString();
    }

    /** One string field of a flat JSON object — enough parsing for an address and a token. */
    static String field(String json, String name) {
        String key = "\"" + name + "\"";
        int k = json == null ? -1 : json.indexOf(key);
        if (k < 0) return null;
        int q1 = json.indexOf('"', json.indexOf(':', k + key.length()) + 1);
        int q2 = q1 < 0 ? -1 : json.indexOf('"', q1 + 1);
        return q2 > q1 ? json.substring(q1 + 1, q2) : null;
    }

    /** The server answers errors as {"error": …} or plain text; take either. */
    static String errorText(String raw) {
        String text = raw == null ? "" : raw.trim();
        // Just enough parsing for the one shape the server sends; no JSON dependency.
        if (text.startsWith("{")) {
            int k = text.indexOf("\"error\"");
            if (k >= 0) {
                int q1 = text.indexOf('"', text.indexOf(':', k) + 1);
                int q2 = q1 < 0 ? -1 : text.indexOf('"', q1 + 1);
                if (q2 > q1) return text.substring(q1 + 1, q2);
            }
        }
        return text.isEmpty() ? "request failed" : text;
    }
}
