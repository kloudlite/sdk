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
// Methods return the raw JSON body as a String; bring your own JSON mapper. The two
// exceptions are sandboxes() and pipelines(), whose calls hand back live handles
// (SandboxHandle, SandboxProcess, Run) — each still exposes its raw JSON.

import java.io.IOException;
import java.io.InterruptedIOException;
import java.io.UncheckedIOException;
import java.net.URI;
import java.net.URLEncoder;
import java.net.http.HttpClient;
import java.net.http.HttpRequest;
import java.net.http.HttpResponse;
import java.net.http.HttpTimeoutException;
import java.nio.charset.StandardCharsets;
import java.time.Duration;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.Base64;
import java.util.Iterator;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.NoSuchElementException;
import java.util.Spliterator;
import java.util.Spliterators;
import java.util.stream.Collectors;
import java.util.stream.Stream;
import java.util.stream.StreamSupport;

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
    private final Secrets secrets = new Secrets();
    private final Sandboxes sandboxes = new Sandboxes();
    private final Pipelines pipelines = new Pipelines();

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

    /** An owner's named secrets: {@code /v1/secrets}. */
    public Secrets secrets() {
        return secrets;
    }

    /** The volumes behind workspaces and environments: {@code /v1/volumes}. */
    public Volumes volumes() {
        return volumes;
    }

    /** Throwaway gVisor sandboxes with a tool server: {@code /v1/sandboxes}. */
    public Sandboxes sandboxes() {
        return sandboxes;
    }

    /** CI pipelines and their runs: {@code /v1/pipelines}, runs under {@code /v1/repos}. */
    public Pipelines pipelines() {
        return pipelines;
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
         * seeds it from a PLATFORM repo (with {@code branch}).
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
                .num("ttl_secs", p.ttlSecs)
                .raw("env", p.env == null ? null : stringMap(p.env))
                .toString());
        }

        /** Replace the whole env (literals or {@code "${{ secrets.NAME }}"}); takes effect on the next start. */
        public String setEnv(String workspace, Map<String, String> env) throws IOException, InterruptedException {
            return api("PUT", path(need(workspace, "workspace"), "/env"), "{\"env\":" + stringMap(env) + "}");
        }

        /** Seconds unused before the workspace is deleted (600..=2592000); {@code null} clears it. Not for a sandbox or a bench. */
        public String setTtl(String workspace, Long ttlSecs) throws IOException, InterruptedException {
            return api("PUT", path(need(workspace, "workspace"), "/ttl"), "{\"ttl_secs\":" + ttlSecs + "}");
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
        private String workspace, image, repo, branch;
        private Long quotaGb, ttlSecs;
        private List<String> packages;
        private Map<String, String> env;

        public CreateWorkspace workspace(String v) { workspace = v; return this; }
        public CreateWorkspace quotaGb(long v) { quotaGb = v; return this; }
        public CreateWorkspace image(String v) { image = v; return this; }
        public CreateWorkspace repo(String v) { repo = v; return this; }
        public CreateWorkspace branch(String v) { branch = v; return this; }
        /** nixpkgs attributes, optionally pinned ({@code attr@version}). */
        public CreateWorkspace packages(String... v) { packages = Arrays.asList(v); return this; }
        /** Seconds unused (600..=2592000) before the workspace is deleted; unset, never. */
        public CreateWorkspace ttlSecs(long v) { ttlSecs = v; return this; }
        /** Variable to a literal value or a {@code "${{ secrets.NAME }}"} reference to the owner's secrets. */
        public CreateWorkspace env(Map<String, String> v) { env = v; return this; }
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
                body(need(p.environment, "environment"), p.quotaGb, p.snapshot, p.services, true, p.ttlSecs));
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

        /** Seconds unused before the environment is deleted (600..=2592000); {@code null} clears it. */
        public String setTtl(String environment, Long ttlSecs) throws IOException, InterruptedException {
            return api("PUT", path(need(environment, "environment"), "/ttl"), "{\"ttl_secs\":" + ttlSecs + "}");
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
            return api("POST", path(env, "/clone"), body(need(p.to, "to"), p.quotaGb, p.snapshot, p.services, false, null));
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

        private String body(String env, Long quotaGb, String snapshot, List<Service> services, boolean always, Long ttlSecs) {
            String[] on = owned(env);
            String b = obj("name", on[1], "owner", on[0], "snapshot_id", snapshot);
            StringBuilder out = new StringBuilder(b.substring(0, b.length() - 1));
            if (quotaGb != null) out.append(",\"quota_gb\":").append(quotaGb);
            if (always || !services.isEmpty()) out.append(",\"services\":").append(array(services));
            if (ttlSecs != null) out.append(",\"ttl_secs\":").append(ttlSecs);
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
        private Long ttlSecs;
        private List<Service> services = List.of();
        private Long quotaGb;

        public CreateEnvironment environment(String v) { environment = v; return this; }
        public CreateEnvironment services(Service... v) { services = Arrays.asList(v); return this; }
        public CreateEnvironment quotaGb(long v) { quotaGb = v; return this; }
        public CreateEnvironment snapshot(String v) { snapshot = v; return this; }
        /** Seconds unused (600..=2592000) before the environment is deleted; unset, never. */
        public CreateEnvironment ttlSecs(long v) { ttlSecs = v; return this; }
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
        /** {@code environment}, when set, must be the session's team's: an id, a name or "team/name"; without one the session connects to nothing until {@link #connect}. */
        public String create(CreateSession p) throws IOException, InterruptedException {
            String[] on = owned(need(p.session, "session"));
            return api("POST", "/v1/sessions", new Json()
                .str("name", on[1])
                .str("team", on[0])
                .str("environment", p.environment)
                .num("ttl_secs", p.ttlSecs)
                .toString());
        }

        /** Seconds unused before the session is deleted (600..=2592000); {@code null} clears it. */
        public String setTtl(String session, Long ttlSecs) throws IOException, InterruptedException {
            return api("PUT", path(need(session, "session"), "/ttl"), "{\"ttl_secs\":" + ttlSecs + "}");
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

        /**
         * Sets or switches the session's environment (an id, a name or "team/name"); {@code null}
         * clears it. Its intercepts in the old environment are released first; deleting an
         * environment disconnects its sessions the same way.
         */
        public String connect(String session, String environment) throws IOException, InterruptedException {
            return api("PUT", path(need(session, "session"), "/environment"),
                environment == null ? "{\"environment\":null}" : obj("environment", environment));
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
        private Long ttlSecs;

        public CreateSession session(String v) { session = v; return this; }
        public CreateSession environment(String v) { environment = v; return this; }
        /** Seconds unused (600..=2592000) before the session is deleted; unset, never. */
        public CreateSession ttlSecs(long v) { ttlSecs = v; return this; }
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
     * An owner's named secrets, over {@code /v1/secrets}. A value goes in and never comes back:
     * {@code list} answers names with who and when. Reference one from a workspace env as
     * {@code "${{ secrets.NAME }}"}.
     */
    public class Secrets {
        public void set(String owner, String name, String value) throws IOException, InterruptedException {
            api("PUT", "/v1/secrets/" + seg(need(owner, "owner")) + "/" + seg(need(name, "name")),
                "{\"value\":" + quote(need(value, "value")) + "}");
        }

        public String list(String owner) throws IOException, InterruptedException {
            return api("GET", "/v1/secrets/" + seg(need(owner, "owner")), null);
        }

        /** A 409 while a workspace or environment still references it; the body names them. */
        public void delete(String owner, String name) throws IOException, InterruptedException {
            api("DELETE", "/v1/secrets/" + seg(need(owner, "owner")) + "/" + seg(need(name, "name")), null);
        }
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

    /**
     * Throwaway sandboxes, over {@code /v1/sandboxes}: one pod under gVisor running a tool server,
     * gone when the caller terminates it or its own timeout passes. A sandbox is named by its id.
     * Unlike every other module these calls hand back handles — {@link SandboxHandle} and
     * {@link SandboxProcess} — because a process is driven, not read once; each still exposes
     * its raw JSON.
     *
     * <pre>
     *   Kloudlite.SandboxHandle sb = kl.sandboxes().create(new Kloudlite.CreateSandbox().image("python:3.12"));
     *   Kloudlite.SandboxProcess p = sb.exec(new Kloudlite.SandboxExec().cmd(List.of("bash", "-c", "while read l; do echo $l; done")));
     *   p.stdin().write("foo bar\n");
     *   p.stdin().writeEof();
     *   int code = p.waitFor(null);
     *   String out = p.stdout().read();
     *   sb.terminate();
     * </pre>
     */
    public class Sandboxes {
        /** Returns at once, the sandbox still {@code starting}; {@link SandboxHandle#exec} waits for it. */
        public SandboxHandle create(CreateSandbox p) throws IOException, InterruptedException {
            return new SandboxHandle(api("POST", "/v1/sandboxes", p.body()));
        }

        public SandboxHandle get(String sandbox) throws IOException, InterruptedException {
            return new SandboxHandle(api("GET", "/v1/sandboxes/" + seg(need(sandbox, "sandbox")), null));
        }

        /** The caller's sandboxes, newest first; a non-null team scopes to their sandboxes in that team. */
        public List<SandboxHandle> list(String team) throws IOException, InterruptedException {
            String raw = api("GET", "/v1/sandboxes" + query("team", team), null);
            List<SandboxHandle> out = new ArrayList<>();
            List<String> docs;
            try {
                docs = JsonReader.elements(raw);
            } catch (IllegalArgumentException e) {
                throw new ApiException(502, "the sandbox listing was not a JSON array");
            }
            for (String doc : docs) out.add(new SandboxHandle(doc));
            return out;
        }

        /**
         * Create a sandbox, run {@code cmd} in it, wait for the command, read all its output — and
         * terminate the sandbox whatever happened, a failed terminate surfacing beside (never
         * instead of) the failure that came first.
         */
        public RunResult run(RunSandbox p) throws IOException, InterruptedException {
            p.exec.body(); // a missing cmd is refused before the sandbox exists
            // A command allowed longer than the sandbox's default lifetime must not be cut by it:
            // the sandbox outlives the wait by a minute, within the server's 10 s..24 h.
            if (p.timeout != null && !p.timeout.isZero()) {
                long secs = Math.min(86_400, Math.max(10, (p.timeout.toMillis() + 999) / 1000 + 60));
                p.create.timeout(Duration.ofSeconds(secs));
            }
            SandboxHandle sb = create(p.create);
            Throwable failed = null;
            try {
                SandboxProcess proc = sb.exec(p.exec);
                int code = proc.waitFor(p.timeout);
                StringBuilder out = new StringBuilder(), err = new StringBuilder();
                proc.follow(out, err);
                return new RunResult(code, out.toString(), err.toString(), sb.id());
            } catch (Throwable t) {
                failed = t;
                throw t;
            } finally {
                try {
                    sb.terminate();
                } catch (IOException | InterruptedException | RuntimeException e) {
                    if (failed == null) throw e;
                    failed.addSuppressed(e);
                }
            }
        }
    }

    /**
     * One sandbox: the doc as last read ({@link #json}, with {@link #id}, {@link #phase} —
     * {@code pending|starting|running|terminated} — and, once terminated, {@link #reason}).
     * Nothing re-reads it behind the caller's back except the waits; {@link #refresh} does.
     */
    public final class SandboxHandle {
        private volatile String json;
        private volatile Map<String, Object> doc;

        SandboxHandle(String json) throws ApiException {
            set(json);
        }

        private void set(String raw) throws ApiException {
            Map<String, Object> d = JsonReader.parsed(raw);
            if (text(d.get("id")) == null) throw new ApiException(502, "the answer named no sandbox id");
            doc = d;
            json = raw;
        }

        public String id() { return text(doc.get("id")); }
        public String phase() { return text(doc.get("phase")); }
        /** Terminated, Timeout, Idle, Exited, Failed, Unplaceable, SourceGone, Lost or EnvironmentGone; null until terminated. */
        public String reason() { return text(doc.get("reason")); }
        public String message() { return text(doc.get("message")); }
        /** The doc's raw JSON, as the server last answered it. */
        public String json() { return json; }

        /** Re-read the doc; returns this handle. */
        public SandboxHandle refresh() throws IOException, InterruptedException {
            set(api("GET", path(""), null));
            return this;
        }

        /** {@link #waitRunning(Duration)} with the default limit of two minutes. */
        public SandboxHandle waitRunning() throws IOException, InterruptedException {
            return waitRunning(Duration.ofSeconds(120));
        }

        /**
         * Poll until {@code running} (0.5 s, backing off to 2 s). A sandbox that ends instead is an
         * {@link ApiException} (409) naming its reason; the limit passing is an
         * {@link HttpTimeoutException}. A null or zero limit waits as long as it takes.
         */
        public SandboxHandle waitRunning(Duration timeout) throws IOException, InterruptedException {
            long deadline = deadline(timeout);
            for (long pause = 500; ; pause = nap(deadline, pause)) {
                refresh();
                if ("running".equals(phase())) return this;
                if ("terminated".equals(phase())) throw ended();
                if (expired(deadline)) {
                    throw new HttpTimeoutException("sandbox " + id() + " was not running within " + timeout);
                }
            }
        }

        /**
         * Start {@code cmd} as a process — always detached, so nothing here is bounded by a
         * request timeout. Waits for the sandbox to run first unless it is already known running.
         */
        public SandboxProcess exec(SandboxExec p) throws IOException, InterruptedException {
            String body = p.body();
            if (!"running".equals(phase())) waitRunning();
            String pid = text(JsonReader.parsed(tool("exec", body)).get("id"));
            if (pid == null) throw new ApiException(502, "the exec answer named no process id");
            return new SandboxProcess(this, pid);
        }

        /** The read tool's JSON answer; paths are relative to {@code ~/workspace}. */
        public String read(String path) throws IOException, InterruptedException {
            return tool("read", obj("path", need(path, "path")));
        }

        /** The write tool's JSON answer; paths are relative to {@code ~/workspace}. */
        public String write(WriteFile p) throws IOException, InterruptedException {
            String path = need(p.path, "path"), content = need(p.content, "content");
            return tool("write", obj("path", path, "content", content));
        }

        /** Ask for the end (202); the doc turns {@code terminated} shortly after. Returns this handle. */
        public SandboxHandle terminate() throws IOException, InterruptedException {
            set(api("POST", path("/terminate"), null));
            return this;
        }

        /**
         * Poll until {@code terminated} and answer the reason. A null or zero limit waits as long
         * as it takes; the limit passing is an {@link HttpTimeoutException}.
         */
        public String waitFor(Duration timeout) throws IOException, InterruptedException {
            long deadline = deadline(timeout);
            for (long pause = 500; ; pause = nap(deadline, pause)) {
                refresh();
                if ("terminated".equals(phase())) return reason();
                if (expired(deadline)) {
                    throw new HttpTimeoutException("sandbox " + id() + " did not terminate within " + timeout);
                }
            }
        }

        /** Delete the record (204). */
        public void delete() throws IOException, InterruptedException {
            api("DELETE", path(""), null);
        }

        String tool(String name, String argsJson) throws IOException, InterruptedException {
            return api("POST", path("/tools/" + seg(name)), argsJson);
        }

        String path(String tail) {
            return "/v1/sandboxes/" + seg(id()) + tail;
        }

        private ApiException ended() {
            String msg = message();
            return new ApiException(409, "sandbox " + id() + " is terminated (" + or(reason(), "no reason given") + ")"
                + (msg == null || msg.isEmpty() ? "" : ": " + msg));
        }
    }

    /**
     * A process started by {@link SandboxHandle#exec}: {@link #stdin} feeds it, {@link #stdout} and
     * {@link #stderr} follow its output from the start, {@link #waitFor} answers its exit code.
     */
    public final class SandboxProcess {
        private final SandboxHandle sandbox;
        private final String id;
        private final Stdin stdin = new Stdin();
        private final Output stdout = new Output("stdout"), stderr = new Output("stderr");
        private volatile Integer returncode;

        SandboxProcess(SandboxHandle sandbox, String id) {
            this.sandbox = sandbox;
            this.id = id;
        }

        public String id() { return id; }
        public SandboxHandle sandbox() { return sandbox; }
        public Stdin stdin() { return stdin; }
        public Output stdout() { return stdout; }
        public Output stderr() { return stderr; }
        /** The exit code last seen, -1 for an exit with none (a signal); null until it has exited. */
        public Integer returncode() { return returncode; }

        /**
         * Block until the process exits and answer its exit code (-1 when it had none). Polls in
         * slices of at most 20 s, the server's own bound; a null or zero limit waits as long as it
         * takes, and the limit passing is an {@link HttpTimeoutException}.
         */
        public int waitFor(Duration timeout) throws IOException, InterruptedException {
            long deadline = deadline(timeout);
            while (true) {
                long slice = Math.max(0, Math.min(20_000, remainingMs(deadline)));
                Map<String, Object> got = JsonReader.parsed(sandbox.tool("process_wait",
                    new Json().str("id", id).num("timeout_ms", slice).toString()));
                if ("exited".equals(got.get("state"))) return settle(got.get("exit_code"));
                if (expired(deadline)) {
                    throw new HttpTimeoutException("process " + id + " did not exit within " + timeout);
                }
            }
        }

        /** The exit code if the process has exited (-1 when it had none), else null; never blocks. */
        public Integer poll() throws IOException, InterruptedException {
            Map<String, Object> got = JsonReader.parsed(sandbox.tool("process_wait",
                new Json().str("id", id).num("timeout_ms", 0L).toString()));
            return "exited".equals(got.get("state")) ? settle(got.get("exit_code")) : null;
        }

        /** TERM, reaching the process group. */
        public String kill() throws IOException, InterruptedException {
            return kill(null);
        }

        /** {@code signal} is TERM (the tool's default) or KILL; it reaches the process group. */
        public String kill(String signal) throws IOException, InterruptedException {
            return sandbox.tool("process_kill", obj("id", id, "signal", signal));
        }

        private int settle(Object code) {
            int c = code instanceof Number ? ((Number) code).intValue() : -1;
            returncode = c;
            return c;
        }

        /** Both streams in one pass over the process stream, until its exit frame. */
        void follow(StringBuilder out, StringBuilder err) throws IOException, InterruptedException {
            try (Frames frames = new Frames()) {
                for (String[] f = frames.next(); f != null; f = frames.next()) {
                    ("stderr".equals(f[0]) ? err : out).append(f[1]);
                }
            }
        }

        /** The process's stdin. Every write is delivered when the call returns. */
        public final class Stdin {
            public void write(byte[] data) throws IOException, InterruptedException {
                sandbox.tool("process_write", obj("id", id,
                    "data_base64", Base64.getEncoder().encodeToString(need(data, "data"))));
            }

            /** Sent as UTF-8. */
            public void write(String data) throws IOException, InterruptedException {
                write(need(data, "data").getBytes(StandardCharsets.UTF_8));
            }

            /** Close stdin: a read loop in the process sees end-of-file. */
            public void writeEof() throws IOException, InterruptedException {
                sandbox.tool("process_close_stdin", obj("id", id));
            }

            /** Nothing to do — kept so a Modal-shaped caller reads the same. */
            public void drain() {}
        }

        /** One of the process's output streams, followed from its first byte. */
        public final class Output {
            private final String name;

            Output(String name) {
                this.name = name;
            }

            /** Everything the stream carries, blocking until the process exits. */
            public String read() throws IOException, InterruptedException {
                StringBuilder out = new StringBuilder(), err = new StringBuilder();
                follow(out, err);
                return ("stderr".equals(name) ? err : out).toString();
            }

            /**
             * Lines as they arrive, without their {@code \n}; a trailing partial line comes last.
             * The stream ends when the process exits; close it to stop following early. A failure
             * mid-stream is an {@link UncheckedIOException}.
             */
            public Stream<String> lines() throws IOException, InterruptedException {
                Frames frames = new Frames();
                Iterator<String> it = new Iterator<>() {
                    private final StringBuilder buf = new StringBuilder();
                    private String ready;
                    private boolean end;

                    @Override
                    public boolean hasNext() {
                        while (ready == null && !end) {
                            int nl = buf.indexOf("\n");
                            if (nl >= 0) {
                                ready = buf.substring(0, nl);
                                buf.delete(0, nl + 1);
                                break;
                            }
                            String[] f = frames.nextUnchecked();
                            if (f == null) {
                                end = true;
                                if (buf.length() > 0) ready = buf.toString();
                                buf.setLength(0);
                            } else if (name.equals(f[0])) {
                                buf.append(f[1]);
                            }
                        }
                        return ready != null;
                    }

                    @Override
                    public String next() {
                        if (!hasNext()) throw new NoSuchElementException();
                        String line = ready;
                        ready = null;
                        return line;
                    }
                };
                return StreamSupport.stream(Spliterators.spliteratorUnknownSize(it,
                    Spliterator.ORDERED | Spliterator.NONNULL), false).onClose(frames::close);
            }
        }

        /**
         * The NDJSON process stream as {stream, data} frames, ending at the exit frame. A
         * connection that ends before it is reopened from the byte cursors read so far (counting
         * what the server reports dropped from its ring), a few times at most.
         */
        private final class Frames implements AutoCloseable {
            private long since, sinceErr;
            private int reopened;
            private Stream<String> body;
            private Iterator<String> lines;
            private boolean done;

            Frames() throws IOException, InterruptedException {
                open();
            }

            private void open() throws IOException, InterruptedException {
                HttpRequest.Builder b = HttpRequest.newBuilder()
                    .uri(URI.create(baseUrl + sandbox.path("/processes/" + seg(id) + "/stream")
                        + "?since=" + since + "&since_err=" + sinceErr))
                    // No request timeout: the stream lasts as long as the process does.
                    .header("User-Agent", "kloudlite-sdk-java/" + VERSION)
                    .header("Accept", "application/x-ndjson")
                    .GET();
                if (token != null) b.header("Authorization", "Bearer " + token);
                HttpResponse<Stream<String>> resp = http.send(b.build(), HttpResponse.BodyHandlers.ofLines());
                if (resp.statusCode() < 200 || resp.statusCode() > 299) {
                    String text;
                    try (Stream<String> s = resp.body()) {
                        text = s.collect(Collectors.joining("\n"));
                    }
                    throw new ApiException(resp.statusCode(), errorText(text));
                }
                body = resp.body();
                lines = body.iterator();
            }

            /** The next output frame, or null once the process has exited. */
            String[] next() throws IOException, InterruptedException {
                while (!done) {
                    String line = null;
                    try {
                        if (lines.hasNext()) line = lines.next();
                    } catch (UncheckedIOException e) {
                        line = null; // a dropped connection: reopened below like a clean end
                    }
                    if (line == null) {
                        body.close();
                        if (++reopened > 3) {
                            throw new ApiException(502, "the stream of process " + id + " ended before it exited");
                        }
                        open();
                        continue;
                    }
                    if (line.isBlank()) continue;
                    Map<String, Object> f = JsonReader.parsed(line);
                    if (f.containsKey("exit")) {
                        done = true;
                        settle(f.get("exit"));
                        body.close();
                        return null;
                    }
                    if (f.get("dropped") instanceof Number) since += ((Number) f.get("dropped")).longValue();
                    if (f.get("dropped_err") instanceof Number) sinceErr += ((Number) f.get("dropped_err")).longValue();
                    Object stream = f.get("stream"), data = f.get("data");
                    if (stream instanceof String && data instanceof String) {
                        long n = ((String) data).getBytes(StandardCharsets.UTF_8).length;
                        if ("stderr".equals(stream)) sinceErr += n;
                        else since += n;
                        return new String[] {(String) stream, (String) data};
                    }
                }
                return null;
            }

            String[] nextUnchecked() {
                try {
                    return next();
                } catch (IOException e) {
                    throw new UncheckedIOException(e);
                } catch (InterruptedException e) {
                    Thread.currentThread().interrupt();
                    throw new UncheckedIOException(new InterruptedIOException("interrupted following process " + id));
                }
            }

            @Override
            public void close() {
                if (body != null) body.close();
            }
        }
    }

    // ── sandbox params ─────────────────────────────────────────────────────

    /**
     * {@link Sandboxes#create}: every field optional, an unset one being the server's default
     * (a 300 s timeout, 1 core, 2 GB of memory, 10 GB of disk, the internet reachable).
     * {@code fromWorkspace} is an id, a name or "team/name"; {@code fromSnapshot} a snapshot id.
     */
    public static final class CreateSandbox {
        private String image, fromWorkspace, fromSnapshot, team, network, session;
        private List<String> packages;
        private Duration timeout, idleTimeout;
        private Map<String, String> env;
        private Long cpu, memoryGb, diskGb;

        public CreateSandbox image(String v) { image = v; return this; }
        /** nixpkgs attributes, optionally pinned ({@code attr@version}). */
        public CreateSandbox packages(String... v) { packages = Arrays.asList(v); return this; }
        public CreateSandbox fromWorkspace(String v) { fromWorkspace = v; return this; }
        public CreateSandbox fromSnapshot(String v) { fromSnapshot = v; return this; }
        public CreateSandbox team(String v) { team = v; return this; }
        /** The sandbox's whole lifetime, sent in whole seconds (rounded up). */
        public CreateSandbox timeout(Duration v) { timeout = v; return this; }
        /** Terminate once nothing has touched it for this long. */
        public CreateSandbox idleTimeout(Duration v) { idleTimeout = v; return this; }
        /** "internet" or "none". */
        public CreateSandbox network(String v) { network = v; return this; }
        public CreateSandbox env(Map<String, String> v) { env = v; return this; }
        public CreateSandbox cpu(int cores) { cpu = (long) cores; return this; }
        public CreateSandbox memoryGb(int v) { memoryGb = (long) v; return this; }
        public CreateSandbox diskGb(int v) { diskGb = (long) v; return this; }
        /**
         * Make it a member of one of your sessions in {@code team} from birth — an id, a name or
         * "team/name"; the session's environment is the only way a sandbox reaches one.
         */
        public CreateSandbox session(String v) { session = v; return this; }

        String body() {
            return new Json()
                .str("team", team)
                .str("image", image)
                .raw("packages", packages == null ? null : strings(packages))
                .str("from_workspace", fromWorkspace)
                .str("from_snapshot", fromSnapshot)
                .num("timeout_secs", seconds(timeout))
                .num("idle_timeout_secs", seconds(idleTimeout))
                .str("network", network)
                .raw("env", env == null ? null : stringMap(env))
                .num("cpu", cpu)
                .num("memory_gb", memoryGb)
                .num("disk_gb", diskGb)
                .str("session", session)
                .toString();
        }
    }

    /** {@link SandboxHandle#exec}: {@code cmd} — a shell string or an argv list — is required. */
    public static final class SandboxExec {
        private String cmd, cwd; // cmd held as JSON: a string or an array
        private Map<String, String> env;

        public SandboxExec cmd(String shell) { cmd = shell == null ? null : quote(shell); return this; }
        public SandboxExec cmd(List<String> argv) { cmd = argv == null ? null : strings(argv); return this; }
        public SandboxExec cwd(String v) { cwd = v; return this; }
        public SandboxExec env(Map<String, String> v) { env = v; return this; }

        String body() {
            return new Json().raw("cmd", need(cmd, "cmd")).str("cwd", cwd)
                .raw("env", env == null ? null : stringMap(env)).bool("detach", true).toString();
        }
    }

    /**
     * {@link Sandboxes#run}: the command ({@code cmd} required, {@code cwd}, {@code env}), how long
     * to wait for it ({@code timeout}; unset waits until it exits), and every
     * {@link CreateSandbox} field — except that {@code env} and {@code timeout} here are the
     * COMMAND's, as in every other SDK, so the sandbox keeps its default lifetime.
     */
    public static final class RunSandbox {
        private final CreateSandbox create = new CreateSandbox();
        private final SandboxExec exec = new SandboxExec();
        private Duration timeout;

        public RunSandbox cmd(String shell) { exec.cmd(shell); return this; }
        public RunSandbox cmd(List<String> argv) { exec.cmd(argv); return this; }
        public RunSandbox cwd(String v) { exec.cwd(v); return this; }
        public RunSandbox env(Map<String, String> v) { exec.env(v); return this; }
        public RunSandbox timeout(Duration v) { timeout = v; return this; }
        public RunSandbox image(String v) { create.image(v); return this; }
        public RunSandbox packages(String... v) { create.packages(v); return this; }
        public RunSandbox fromWorkspace(String v) { create.fromWorkspace(v); return this; }
        public RunSandbox fromSnapshot(String v) { create.fromSnapshot(v); return this; }
        public RunSandbox team(String v) { create.team(v); return this; }
        public RunSandbox idleTimeout(Duration v) { create.idleTimeout(v); return this; }
        public RunSandbox network(String v) { create.network(v); return this; }
        public RunSandbox cpu(int cores) { create.cpu(cores); return this; }
        public RunSandbox memoryGb(int v) { create.memoryGb(v); return this; }
        public RunSandbox diskGb(int v) { create.diskGb(v); return this; }
        public RunSandbox session(String v) { create.session(v); return this; }
    }

    /** What {@link Sandboxes#run} answers; {@link #toString} is its JSON. */
    public static final class RunResult {
        /** -1 when the process exited without one (a signal). */
        public final int exitCode;
        public final String stdout, stderr;
        /** The sandbox's id — already terminated. */
        public final String sandbox;

        RunResult(int exitCode, String stdout, String stderr, String sandbox) {
            this.exitCode = exitCode;
            this.stdout = stdout;
            this.stderr = stderr;
            this.sandbox = sandbox;
        }

        @Override
        public String toString() {
            return new Json().num("exit_code", (long) exitCode).str("stdout", stdout)
                .str("stderr", stderr).str("sandbox", sandbox).toString();
        }
    }

    /**
     * CI pipelines, over {@code /v1/pipelines}: definitions live as YAML in an owner's
     * "{owner}/pipelines" repository and run against that owner's repositories, each job in its own
     * sandbox. A run is recorded on the repository it is about, so it is read back by that
     * {@code repo} ("owner/name"). {@link #run}, {@link #runs} and {@link #getRun} hand back
     * {@link Run} handles.
     *
     * <pre>
     *   Kloudlite.Run run = kl.pipelines().run(new Kloudlite.RunPipeline()
     *       .owner("acme").pipeline("ci").repo("acme/payments"));
     *   try (Stream&lt;String&gt; log = run.follow("test")) { log.forEach(System.out::print); }
     *   String state = run.waitFor(); // "success"
     * </pre>
     */
    public class Pipelines {
        /**
         * The caller's own definitions; see {@link #list(String)}.
         */
        public String list() throws IOException, InterruptedException {
            return list(null);
        }

        /**
         * The definitions on {owner}/pipelines's main branch, broken ones included:
         * {@code {repo, sha, pipelines: [{name, file, valid, errors, repos, on, jobs}], errors}}.
         * A null owner means the caller's own.
         */
        public String list(String owner) throws IOException, InterruptedException {
            return api("GET", "/v1/pipelines" + query("owner", owner), null);
        }

        /**
         * Start a run by hand and answer it at once (202), {@code queued}. A definition that cannot
         * run is an {@link ApiException} 422 whose message lists every reason, and nothing is
         * recorded.
         */
        public Run run(RunPipeline p) throws IOException, InterruptedException {
            String body = p.body();
            String owner = need(p.owner, "owner"), pipeline = need(p.pipeline, "pipeline");
            return new Run(api("POST", "/v1/pipelines/" + seg(owner) + "/" + seg(pipeline) + "/runs", body));
        }

        /** {@code repo}'s runs, newest first; a set {@code pipeline} keeps one pipeline's. */
        public List<Run> runs(ListRuns p) throws IOException, InterruptedException {
            String limit = p.limit == null ? null : p.limit.toString();
            String raw = api("GET", runsPath(need(p.repo, "repo"), "") + query("pipeline", p.pipeline, "limit", limit), null);
            List<String> docs;
            try {
                docs = JsonReader.elements(raw);
            } catch (IllegalArgumentException e) {
                throw new ApiException(502, "the run listing was not a JSON array");
            }
            List<Run> out = new ArrayList<>();
            for (String doc : docs) out.add(new Run(doc));
            return out;
        }

        /** One run of {@code repo} by its id ("1760400000000-0a1b2c"). */
        public Run getRun(GetRun p) throws IOException, InterruptedException {
            String repo = need(p.repo, "repo"), run = need(p.run, "run");
            return new Run(api("GET", runsPath(repo, "/" + seg(run)), null));
        }
    }

    static String runsPath(String repo, String tail) {
        String[] on = split(repo);
        return "/v1/repos/" + seg(on[0]) + "/" + seg(on[1]) + "/runs" + tail;
    }

    /**
     * One pipeline run: the doc as last read ({@link #json}, with named readers for its fields;
     * {@link #state} is {@code queued|running|success|failure|cancelled}). Each of {@link #jobs}
     * is a map with key, job, matrix, needs, state ({@code pending|running|success|failure|
     * skipped|cancelled}), sandbox, startedAtMs, finishedAtMs, exitCode, failedStep and detail;
     * {@link #logs} and {@link #follow} name one by its index or its key. Nothing re-reads the doc
     * behind the caller's back except {@link #waitFor}; {@link #refresh} does.
     */
    public final class Run {
        private volatile String json;
        private volatile Map<String, Object> doc;

        Run(String json) throws ApiException {
            set(json);
        }

        private void set(String raw) throws ApiException {
            Map<String, Object> d = JsonReader.parsed(raw);
            if (text(d.get("id")) == null || text(d.get("repo")) == null) {
                throw new ApiException(502, "the answer named no run id and repo");
            }
            doc = d;
            json = raw;
        }

        public String id() { return text(doc.get("id")); }
        /** Per repo, from 1: the name people say out loud. */
        public Long number() { return longOf(doc.get("number")); }
        public String pipeline() { return text(doc.get("pipeline")); }
        public String pipelinesRepo() { return text(doc.get("pipelinesRepo")); }
        public String pipelinesSha() { return text(doc.get("pipelinesSha")); }
        /** "owner/name" of the repo the run is about. */
        public String repo() { return text(doc.get("repo")); }
        public String ref() { return text(doc.get("ref")); }
        public String sha() { return text(doc.get("sha")); }
        /** manual, push, pull_request, schedule or pipeline. */
        public String event() { return text(doc.get("event")); }
        @SuppressWarnings("unchecked")
        public Map<String, Object> inputs() {
            Object v = doc.get("inputs");
            return v instanceof Map ? (Map<String, Object>) v : Map.of();
        }
        /** As requested: a String, a Map {@code {clone: name}}, or null for the pipeline's own. */
        public Object environment() { return doc.get("environment"); }
        public String requestedBy() { return text(doc.get("requestedBy")); }
        public Long requestedAtMs() { return longOf(doc.get("requestedAtMs")); }
        public String state() { return text(doc.get("state")); }
        public boolean cancelRequested() { return Boolean.TRUE.equals(doc.get("cancelRequested")); }
        public Long startedAtMs() { return longOf(doc.get("startedAtMs")); }
        public Long finishedAtMs() { return longOf(doc.get("finishedAtMs")); }
        public String detail() { return text(doc.get("detail")); }
        @SuppressWarnings("unchecked")
        public List<Map<String, Object>> jobs() {
            Object v = doc.get("jobs");
            List<Map<String, Object>> out = new ArrayList<>();
            if (v instanceof List) for (Object j : (List<Object>) v) if (j instanceof Map) out.add((Map<String, Object>) j);
            return out;
        }
        /** The doc's raw JSON, as the server last answered it. */
        public String json() { return json; }

        /** Whether the run has ended (success, failure or cancelled) as of the last read. */
        public boolean done() {
            String st = state();
            return "success".equals(st) || "failure".equals(st) || "cancelled".equals(st);
        }

        /** Re-read the doc; returns this handle. */
        public Run refresh() throws IOException, InterruptedException {
            set(api("GET", path(""), null));
            return this;
        }

        /**
         * Ask it to stop: a queued run is cancelled at once, a running one once its worker has
         * ended its sandboxes — {@link #waitFor} sees it reach {@code cancelled}. Returns this handle.
         */
        public Run cancel() throws IOException, InterruptedException {
            set(api("POST", path("/cancel"), null));
            return this;
        }

        /** {@link #waitFor(WaitRun)} with no limit, polling every 5 s. */
        public String waitFor() throws IOException, InterruptedException {
            return waitFor(new WaitRun());
        }

        /**
         * Poll until the run has ended and answer its final state. An unset or zero timeout waits
         * as long as it takes; the limit passing is an {@link HttpTimeoutException}.
         */
        public String waitFor(WaitRun p) throws IOException, InterruptedException {
            long deadline = deadline(p.timeout);
            long poll = millisOr(p.poll, 5000);
            while (!done()) {
                refresh();
                if (done()) break;
                if (expired(deadline)) {
                    throw new HttpTimeoutException("run " + id() + " still " + state() + " after " + p.timeout);
                }
                Thread.sleep(Math.max(0, Math.min(poll, remainingMs(deadline))));
            }
            return state();
        }

        /** The job's whole log so far (combined stdout and stderr), every page of it; by index. */
        public String logs(int job) throws IOException, InterruptedException {
            return allLogs(index(job));
        }

        /** {@link #logs(int)} for the job whose key is {@code job} ("test (postgres16)"). */
        public String logs(String job) throws IOException, InterruptedException {
            return allLogs(index(job));
        }

        /** {@link #follow(FollowLogs)} for job {@code job} by index, polling every 5 s. */
        public Stream<String> follow(int job) {
            return follow(new FollowLogs().job(job));
        }

        /** {@link #follow(FollowLogs)} for the job whose key is {@code job}, polling every 5 s. */
        public Stream<String> follow(String job) {
            return follow(new FollowLogs().job(job));
        }

        /**
         * The job's log in chunks as it is written — from the first byte, polling while nothing new
         * has landed — ending once the job has ended and every byte was delivered. Lazy: each
         * page is read as the stream is consumed. A failure mid-stream is an
         * {@link UncheckedIOException}.
         */
        public Stream<String> follow(FollowLogs p) {
            int index = p.job instanceof Integer ? index((int) (Integer) p.job) : index((String) need(p.job, "job"));
            long poll = millisOr(p.poll, 5000);
            Iterator<String> it = new Iterator<>() {
                private long offset;
                private boolean end;
                private String ready;

                @Override
                public boolean hasNext() {
                    while (ready == null && !end) {
                        try {
                            Map<String, Object> page = page(index, offset);
                            String data = text(page.get("data"));
                            Long next = longOf(page.get("next"));
                            long to = next == null ? offset : next;
                            if (data != null && !data.isEmpty()) ready = data;
                            if (Boolean.TRUE.equals(page.get("complete"))) end = true;
                            else if (to <= offset) Thread.sleep(poll);
                            offset = Math.max(offset, to);
                        } catch (IOException e) {
                            throw new UncheckedIOException(e);
                        } catch (InterruptedException e) {
                            Thread.currentThread().interrupt();
                            throw new UncheckedIOException(new InterruptedIOException("interrupted following run " + id()));
                        }
                    }
                    return ready != null;
                }

                @Override
                public String next() {
                    if (!hasNext()) throw new NoSuchElementException();
                    String chunk = ready;
                    ready = null;
                    return chunk;
                }
            };
            return StreamSupport.stream(Spliterators.spliteratorUnknownSize(it,
                Spliterator.ORDERED | Spliterator.NONNULL), false);
        }

        private String allLogs(int index) throws IOException, InterruptedException {
            StringBuilder out = new StringBuilder();
            for (long offset = 0; ; ) {
                Map<String, Object> page = page(index, offset);
                out.append(or(text(page.get("data")), ""));
                Long next = longOf(page.get("next"));
                long to = next == null ? offset : next;
                if (Boolean.TRUE.equals(page.get("complete")) || to <= offset) return out.toString();
                offset = to;
            }
        }

        /** {@code {data, next, complete, state}}: at most a page of the log from byte {@code offset}. */
        private Map<String, Object> page(int index, long offset) throws IOException, InterruptedException {
            return JsonReader.parsed(api("GET", path("/jobs/" + index + "/logs?offset=" + offset), null));
        }

        private int index(int job) {
            int n = jobs().size();
            if (job < 0 || job >= n) throw new IllegalArgumentException("run " + id() + " has " + n + " jobs, no job " + job);
            return job;
        }

        private int index(String key) {
            List<String> keys = new ArrayList<>();
            for (Map<String, Object> j : jobs()) keys.add(text(j.get("key")));
            int i = keys.indexOf(key);
            if (i < 0) throw new IllegalArgumentException("run " + id() + " has no job \"" + key + "\"; its jobs are " + keys);
            return i;
        }

        private String path(String tail) {
            return runsPath(repo(), "/" + seg(id()) + tail);
        }

        @Override
        public String toString() {
            return "Run " + repo() + "#" + number() + " " + pipeline() + " " + state();
        }
    }

    // ── pipeline params ────────────────────────────────────────────────────

    /**
     * {@link Pipelines#run}: {@code owner}'s {@code pipeline} on {@code repo} (a name, or
     * "owner/name" — one of owner's), all three required, at {@code ref} (a branch, tag, full ref or
     * sha; unset means main). {@code input} adds one of the inputs the pipeline's
     * {@code on.manual.inputs} declares. {@code environment} runs every job that names none in that
     * environment; {@code clone} instead in a per-run copy of it, deleted when the run ends — one or
     * the other.
     */
    public static final class RunPipeline {
        private String owner, pipeline, repo, ref, environment, clone;
        private Map<String, String> inputs;

        public RunPipeline owner(String v) { owner = v; return this; }
        public RunPipeline pipeline(String v) { pipeline = v; return this; }
        public RunPipeline repo(String v) { repo = v; return this; }
        public RunPipeline ref(String v) { ref = v; return this; }
        /** Every input at once, replacing any set before. */
        public RunPipeline inputs(Map<String, String> v) { inputs = v == null ? null : new LinkedHashMap<>(v); return this; }
        public RunPipeline input(String key, String value) {
            if (inputs == null) inputs = new LinkedHashMap<>();
            inputs.put(need(key, "input key"), need(value, "input value"));
            return this;
        }
        public RunPipeline environment(String v) { environment = v; return this; }
        /** Run in a per-run copy of this environment, deleted when the run ends. */
        public RunPipeline clone(String v) { clone = v; return this; }

        String body() {
            need(owner, "owner");
            need(pipeline, "pipeline");
            if (environment != null && clone != null) {
                throw new IllegalArgumentException("pass environment or clone, not both");
            }
            return new Json()
                .str("repo", need(repo, "repo"))
                .str("ref", ref)
                .raw("inputs", inputs == null ? null : stringMap(inputs))
                .raw("environment", clone != null ? obj("clone", clone) : environment == null ? null : quote(environment))
                .toString();
        }
    }

    /** {@link Pipelines#runs}: {@code repo} required; {@code pipeline} and {@code limit} filter. */
    public static final class ListRuns {
        private String repo, pipeline;
        private Integer limit;

        public ListRuns repo(String v) { repo = v; return this; }
        public ListRuns pipeline(String v) { pipeline = v; return this; }
        public ListRuns limit(int v) { limit = v; return this; }
    }

    /** {@link Pipelines#getRun}: both required. */
    public static final class GetRun {
        private String repo, run;

        public GetRun repo(String v) { repo = v; return this; }
        public GetRun run(String v) { run = v; return this; }
    }

    /** {@link Run#waitFor}: an unset {@code timeout} waits as long as it takes; {@code poll} defaults to 5 s. */
    public static final class WaitRun {
        private Duration timeout, poll;

        public WaitRun timeout(Duration v) { timeout = v; return this; }
        public WaitRun poll(Duration v) { poll = v; return this; }
    }

    /** {@link Run#follow}: {@code job} (its index or its key) required; {@code poll} defaults to 5 s. */
    public static final class FollowLogs {
        private Object job;
        private Duration poll;

        public FollowLogs job(int index) { job = index; return this; }
        public FollowLogs job(String key) { job = key; return this; }
        public FollowLogs poll(Duration v) { poll = v; return this; }
    }

    static Long longOf(Object v) {
        return v instanceof Number ? ((Number) v).longValue() : null;
    }

    static long millisOr(Duration d, long fallback) {
        return d == null || d.isZero() || d.isNegative() ? fallback : d.toMillis();
    }

    /** Whole seconds, rounded up so a sub-second limit is never sent as none. */
    static Long seconds(Duration d) {
        if (d == null) return null;
        long ms = d.toMillis();
        return (ms + 999) / 1000;
    }

    /** A {@link System#nanoTime} deadline; Long.MAX_VALUE stands for "no limit" (null or zero). */
    static long deadline(Duration d) {
        return d == null || d.isZero() || d.isNegative() ? Long.MAX_VALUE : System.nanoTime() + d.toNanos();
    }

    static long remainingMs(long deadline) {
        return deadline == Long.MAX_VALUE ? Long.MAX_VALUE : (deadline - System.nanoTime()) / 1_000_000;
    }

    static boolean expired(long deadline) {
        return deadline != Long.MAX_VALUE && deadline - System.nanoTime() <= 0;
    }

    /** Sleep {@code pause} (never past the deadline) and answer the next, doubled up to 2 s. */
    static long nap(long deadline, long pause) throws InterruptedException {
        long left = remainingMs(deadline);
        if (left > 0) Thread.sleep(Math.min(pause, left));
        return Math.min(pause * 2, 2000);
    }

    /** A scalar field as text: a string as is, a number in its plain spelling, null for absent or null. */
    static String text(Object v) {
        return v == null ? null : v.toString();
    }

    /**
     * Just enough of a JSON reader for the sandbox handles, which must see a doc's phase and a
     * stream frame's data with every escape undone: objects become LinkedHashMaps, arrays Lists,
     * integers Longs, other numbers Doubles. Malformed input is an IllegalArgumentException.
     */
    static final class JsonReader {
        private final String s;
        private int i;

        private JsonReader(String s) {
            this.s = s;
        }

        /** A JSON object, or a 502 — the server answered something that is not one. */
        static Map<String, Object> parsed(String json) throws ApiException {
            try {
                JsonReader r = new JsonReader(json == null ? "" : json);
                Object v = r.value();
                r.ws();
                if (r.i != r.s.length() || !(v instanceof Map)) throw r.bad();
                @SuppressWarnings("unchecked")
                Map<String, Object> m = (Map<String, Object>) v;
                return m;
            } catch (IllegalArgumentException e) {
                throw new ApiException(502, "expected a JSON object, got " + json);
            }
        }

        /** Each element of a top-level array, as its own raw JSON text. */
        static List<String> elements(String json) {
            JsonReader r = new JsonReader(json);
            List<String> out = new ArrayList<>();
            r.ws();
            r.expect('[');
            r.ws();
            if (r.peek() == ']') {
                r.i++;
                return out;
            }
            while (true) {
                r.ws();
                int start = r.i;
                r.value();
                out.add(json.substring(start, r.i));
                r.ws();
                char c = r.take();
                if (c == ']') return out;
                if (c != ',') throw r.bad();
            }
        }

        private Object value() {
            ws();
            char c = peek();
            if (c == '{') {
                i++;
                Map<String, Object> m = new LinkedHashMap<>();
                ws();
                if (peek() == '}') {
                    i++;
                    return m;
                }
                while (true) {
                    ws();
                    expect('"');
                    String k = string();
                    ws();
                    expect(':');
                    m.put(k, value());
                    ws();
                    char d = take();
                    if (d == '}') return m;
                    if (d != ',') throw bad();
                }
            }
            if (c == '[') {
                i++;
                List<Object> a = new ArrayList<>();
                ws();
                if (peek() == ']') {
                    i++;
                    return a;
                }
                while (true) {
                    a.add(value());
                    ws();
                    char d = take();
                    if (d == ']') return a;
                    if (d != ',') throw bad();
                }
            }
            if (c == '"') {
                i++;
                return string();
            }
            if (s.startsWith("true", i)) { i += 4; return Boolean.TRUE; }
            if (s.startsWith("false", i)) { i += 5; return Boolean.FALSE; }
            if (s.startsWith("null", i)) { i += 4; return null; }
            int start = i;
            while (i < s.length() && "+-0123456789.eE".indexOf(s.charAt(i)) >= 0) i++;
            String num = s.substring(start, i);
            if (num.isEmpty()) throw bad();
            try {
                return num.matches("-?\\d+") ? (Object) Long.parseLong(num) : (Object) Double.parseDouble(num);
            } catch (NumberFormatException e) {
                return Double.parseDouble(num);
            }
        }

        /** The rest of a string whose opening quote is consumed. */
        private String string() {
            StringBuilder b = new StringBuilder();
            while (true) {
                char c = take();
                if (c == '"') return b.toString();
                if (c != '\\') {
                    b.append(c);
                    continue;
                }
                char e = take();
                switch (e) {
                    case 'n' -> b.append('\n');
                    case 't' -> b.append('\t');
                    case 'r' -> b.append('\r');
                    case 'b' -> b.append('\b');
                    case 'f' -> b.append('\f');
                    case 'u' -> {
                        if (i + 4 > s.length()) throw bad();
                        b.append((char) Integer.parseInt(s.substring(i, i + 4), 16));
                        i += 4;
                    }
                    default -> b.append(e); // \" \\ \/
                }
            }
        }

        private void ws() {
            while (i < s.length() && Character.isWhitespace(s.charAt(i))) i++;
        }

        private char peek() {
            if (i >= s.length()) throw bad();
            return s.charAt(i);
        }

        private char take() {
            char c = peek();
            i++;
            return c;
        }

        private void expect(char c) {
            if (take() != c) throw bad();
        }

        private IllegalArgumentException bad() {
            return new IllegalArgumentException("malformed JSON at offset " + i);
        }
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
