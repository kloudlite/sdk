// Kloudlite SDK for Java — one file, no dependencies (java.net.http, Java 11+).
//
// Auth is a Bearer token (a session or CLI token). Every module hangs off the
// one client, and every repository is named by ONE "owner/name" string — the
// owner half being a person's handle or a team slug:
//
//   Kloudlite kl = new Kloudlite("https://api.example.com", "…");
//   String repo = kl.codeRepos().create("acme/payments", null, null);
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
import java.util.List;

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

    private final String baseUrl;
    private final String hubUrl; // null = no hub listener configured
    private final String token; // null = anonymous
    private final HttpClient http;
    private final CodeRepos codeRepos = new CodeRepos();
    private final HubRepos modelRepos = new HubRepos("model", "models");
    private final HubRepos dataRepos = new HubRepos("dataset", "datasets");
    private final Workspaces workspaces = new Workspaces();
    private final Environments environments = new Environments();
    private final Volumes volumes = new Volumes();

    /** token may be null for anonymous access to public data. */
    public Kloudlite(String baseUrl, String token) {
        this(baseUrl, null, token);
    }

    /** The two-listener form: {@code hubUrl} is where models and datasets live. */
    public Kloudlite(String baseUrl, String hubUrl, String token) {
        this.baseUrl = baseUrl.replaceAll("/+$", "");
        this.hubUrl = hubUrl == null ? null : hubUrl.replaceAll("/+$", "");
        this.token = token;
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

    /** The volumes behind workspaces and environments: {@code /v1/volumes}. */
    public Volumes volumes() {
        return volumes;
    }

    /** The ceiling and what is against it; a null owner means the caller's own. */
    public String quota(String owner) throws IOException, InterruptedException {
        return request("GET", owner == null ? "/v1/quota" : "/v1/quota?owner=" + seg(owner), null);
    }

    /**
     * One HTTP round trip. {@code jsonBody} may be null; returns the answer's raw
     * JSON body, and throws {@link ApiException} on any non-2xx status.
     */
    public String request(String method, String path, String jsonBody) throws IOException, InterruptedException {
        return send(baseUrl, method, path, jsonBody);
    }

    /** The same round trip against the hub listener (models and datasets). */
    public String hubRequest(String method, String path, String jsonBody) throws IOException, InterruptedException {
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
     * Every {@code repo} argument is one "owner/name" string; only owner-scoped
     * calls ({@link #list}) take a bare handle.
     */
    public class CodeRepos {
        // repos
        public String create(String repo, String visibility, String description)
                throws IOException, InterruptedException {
            String[] on = split(repo);
            return request("POST", "/v1/repos", obj(
                "owner", on[0], "name", on[1],
                "visibility", visibility == null ? "private" : visibility,
                "description", description == null ? "" : description));
        }

        public String list(String owner) throws IOException, InterruptedException {
            return request("GET", "/v1/repos?owner=" + seg(owner), null);
        }

        public String get(String repo) throws IOException, InterruptedException {
            return request("GET", path(repo, ""), null);
        }

        /** Either argument may be null to leave it unchanged. */
        public String update(String repo, String visibility, String description)
                throws IOException, InterruptedException {
            return request("PATCH", path(repo, ""), obj("visibility", visibility, "description", description));
        }

        public String delete(String repo) throws IOException, InterruptedException {
            return request("DELETE", path(repo, ""), null);
        }

        /**
         * A new repo {@code dst} copied from {@code src}: objects and refs, never its pull
         * requests or settings. A null visibility means private, whatever the source was.
         */
        public String fork(String src, String dst, String visibility, String description)
                throws IOException, InterruptedException {
            String[] on = split(dst);
            return request("POST", path(src, "/fork"), obj(
                "owner", on[0], "name", on[1],
                "visibility", visibility == null ? "private" : visibility,
                "description", description == null ? "" : description));
        }

        /**
         * A new repo whose contents are fetched from a PUBLIC git remote — {@code git} is a
         * strict "https://" URL (GitHub is one host among any). Answers 202: the repo exists but
         * the fetch runs in the background; poll {@link #importStatus}.
         */
        public String importGit(String repo, String git, String visibility, String description)
                throws IOException, InterruptedException {
            String[] on = split(repo);
            return request("POST", "/v1/repos/import", obj(
                "owner", on[0], "name", on[1], "git", git,
                "visibility", visibility == null ? "private" : visibility,
                "description", description == null ? "" : description));
        }

        /** {from, state: queued|running|done|failed, detail?} for an imported repo. */
        public String importStatus(String repo) throws IOException, InterruptedException {
            return request("GET", path(repo, "/import"), null);
        }

        // branches
        public String branches(String repo) throws IOException, InterruptedException {
            return request("GET", path(repo, "/branches"), null);
        }

        /** {@code oid} is the head the caller saw; the delete is refused if the branch moved. */
        public String deleteBranch(String repo, String branch, String oid)
                throws IOException, InterruptedException {
            return request("DELETE", path(repo, "/branches/" + seg(branch) + "?oid=" + seg(oid)), null);
        }

        // protection
        public String protection(String repo) throws IOException, InterruptedException {
            return request("GET", path(repo, "/protection"), null);
        }

        public String protect(String repo, String pattern, boolean remove)
                throws IOException, InterruptedException {
            return request("POST", path(repo, "/protection"),
                "{\"pattern\":" + quote(pattern) + ",\"remove\":" + remove + "}");
        }

        // pull requests
        public String pulls(String repo) throws IOException, InterruptedException {
            return request("GET", path(repo, "/pulls"), null);
        }

        public String pull(String repo, long number) throws IOException, InterruptedException {
            return request("GET", path(repo, "/pulls/" + number), null);
        }

        public String openPull(String repo, String title, String head, String base, String description)
                throws IOException, InterruptedException {
            return openPull(repo, title, head, base, description, null);
        }

        /**
         * {@code headRepo} (another "owner/name") makes it a cross-repo pull: {@code head} then
         * names a branch of that fork, proposed onto this repo's {@code base}.
         */
        public String openPull(String repo, String title, String head, String base,
                               String description, String headRepo) throws IOException, InterruptedException {
            if (headRepo != null) split(headRepo);
            return request("POST", path(repo, "/pulls"), obj(
                "title", title, "head", head, "base", base,
                "description", description == null ? "" : description,
                "head_repo", headRepo));
        }

        public String comment(String repo, long number, String body)
                throws IOException, InterruptedException {
            return request("POST", path(repo, "/pulls/" + number + "/comments"), obj("body", body));
        }

        /** The merge is a job (202); strategy is fast-forward, squash, merge or rebase. */
        public String merge(String repo, long number, String strategy)
                throws IOException, InterruptedException {
            return request("POST", path(repo, "/pulls/" + number + "/merge?strategy=" + seg(strategy)), null);
        }

        public String close(String repo, long number) throws IOException, InterruptedException {
            return request("POST", path(repo, "/pulls/" + number + "/close"), null);
        }

        // commits
        public String compare(String repo, String base, String head)
                throws IOException, InterruptedException {
            return request("GET", path(repo, "/compare?base=" + seg(base) + "&head=" + seg(head)), null);
        }

        /** {@code commitJson} is the whole body: branch, message, changes, expect?, newBranch?. */
        public String commit(String repo, String commitJson)
                throws IOException, InterruptedException {
            return request("POST", path(repo, "/commits"), commitJson);
        }

        public String signature(String repo, String sha) throws IOException, InterruptedException {
            return request("GET", path(repo, "/commits/" + seg(sha) + "/signature"), null);
        }

        private String path(String repo, String tail) {
            String[] on = split(repo);
            return "/v1/repos/" + seg(on[0]) + "/" + seg(on[1]) + tail;
        }
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

        /** {@code privateRepo} is sent explicitly rather than trusting a server default. */
        public String create(String repo, boolean privateRepo) throws IOException, InterruptedException {
            String[] on = split(repo);
            return hubRequest("POST", "/api/repos/create",
                "{\"name\":" + quote(on[1]) + ",\"organization\":" + quote(on[0])
                    + ",\"type\":" + quote(kind) + ",\"private\":" + privateRepo + "}");
        }

        public String get(String repo) throws IOException, InterruptedException {
            return hubRequest("GET", path(repo, ""), null);
        }

        /** Branches and tags, as the hub spells them. */
        public String refs(String repo) throws IOException, InterruptedException {
            return hubRequest("GET", path(repo, "/refs"), null);
        }

        public String setPrivate(String repo, boolean privateRepo) throws IOException, InterruptedException {
            return hubRequest("PUT", path(repo, "/settings"), "{\"private\":" + privateRepo + "}");
        }

        public String delete(String repo) throws IOException, InterruptedException {
            String[] on = split(repo);
            return hubRequest("DELETE", "/api/repos/delete",
                obj("name", on[1], "organization", on[0], "type", kind));
        }

        /**
         * A new repo whose contents — large files included, through git-lfs — are fetched from a
         * PUBLIC git remote; {@code git} is a strict "https://" URL (the Hugging Face Hub is one
         * host among any). Answers 202; poll {@link #importStatus}.
         */
        public String importGit(String repo, String git, boolean privateRepo)
                throws IOException, InterruptedException {
            return hubRequest("POST", path(repo, "/import"),
                "{\"from\":" + quote(git) + ",\"private\":" + privateRepo + "}");
        }

        /** {from, state: queued|running|done|failed, detail?} for an imported repo. */
        public String importStatus(String repo) throws IOException, InterruptedException {
            return hubRequest("GET", path(repo, "/import"), null);
        }

        private String path(String repo, String tail) {
            String[] on = split(repo);
            return "/api/" + plural + "/" + seg(on[0]) + "/" + seg(on[1]) + tail;
        }
    }

    /**
     * Btrfs-backed dev workspaces, over {@code /v1/workspaces}. A workspace is named by its id
     * (the name given at create); pushes cut snapshots, kept on the workspace's VOLUME — see
     * {@link Volumes} for history, restore targets and deletion. The create and restore bodies
     * are raw JSON, like every other structured body in this client.
     */
    public class Workspaces {
        /** {@code createJson}: name, quota_gb, team?, image?, repo?, branch?, packages, region?
         * (region matters only on a multi-region fleet — absent, the deployment's one is used). */
        public String create(String createJson) throws IOException, InterruptedException {
            return request("POST", "/v1/workspaces", createJson);
        }

        /** The caller's workspaces; a non-null team scopes to their workspaces in that team. */
        public String list(String team) throws IOException, InterruptedException {
            return request("GET", team == null ? "/v1/workspaces" : "/v1/workspaces?team=" + seg(team), null);
        }

        public String get(String id) throws IOException, InterruptedException {
            return request("GET", "/v1/workspaces/" + seg(id), null);
        }

        /** Runs the worktree finalizer: the volume survives while a pushed snapshot references it. */
        public String delete(String id) throws IOException, InterruptedException {
            return request("DELETE", "/v1/workspaces/" + seg(id), null);
        }

        public String start(String id) throws IOException, InterruptedException {
            return request("POST", "/v1/workspaces/" + seg(id) + "/start", null);
        }

        /** A stop cuts a sync point and tears the pod down — seconds, never waiting on a replica. */
        public String stop(String id) throws IOException, InterruptedException {
            return request("POST", "/v1/workspaces/" + seg(id) + "/stop", null);
        }

        /** Cut a SNAPSHOT of the working copy — the one durable, restorable state. */
        public String push(String id, String message) throws IOException, InterruptedException {
            return request("POST", "/v1/workspaces/" + seg(id) + "/push",
                message == null ? null : obj("message", message));
        }

        /**
         * A new workspace from this one's state, cut at the moment of the request; the answer's
         * {@code based_on} names exactly what it grafted onto.
         */
        public String clone_(String id, String name, String task) throws IOException, InterruptedException {
            return request("POST", "/v1/workspaces/" + seg(id) + "/clone", obj("name", name, "task", task));
        }

        /** {@code restoreJson}: name, snapshot_id, image?, packages?, quota_gb?. */
        public String restore(String restoreJson) throws IOException, InterruptedException {
            return request("POST", "/v1/workspaces/restore", restoreJson);
        }

        /** {@code packagesJson}: {"packages": [...]} — pinned entries are locked before writing. */
        public String setPackages(String id, String packagesJson) throws IOException, InterruptedException {
            return request("PATCH", "/v1/workspaces/" + seg(id), packagesJson);
        }

        /** Re-resolve every pinned package — the only call that moves existing locks. */
        public String updatePackages(String id) throws IOException, InterruptedException {
            return request("POST", "/v1/workspaces/" + seg(id) + "/packages/update", null);
        }

        /** The workspace's tool-server address and token — answered to the owner alone. */
        public String tools(String id) throws IOException, InterruptedException {
            return request("GET", "/v1/workspaces/" + seg(id) + "/tools", null);
        }

        /**
         * An {@link Ide} bound to this workspace's tool server. The address is a pod IP:
         * reachable from inside the fleet (a bench); from a laptop, tunnel with
         * {@code kl-connect ws ide} and use {@code new Ide("http://127.0.0.1:7788", token)}.
         */
        public Ide ide(String id) throws IOException, InterruptedException {
            String doc = tools(id);
            String address = field(doc, "address");
            if (address == null) {
                throw new ApiException(502, "the tools answer named no address");
            }
            return new Ide("http://" + address, field(doc, "token"));
        }
    }

    /**
     * A workspace's tool server ({@code kl ide serve}): the full tool set over plain HTTP —
     * {@link #list} answers every tool with its JSON schema (the authority on arguments) and
     * {@link #call} runs one with a raw JSON args object; the helpers cover the everyday files
     * and exec calls, and watch_* and container_* go through {@code call} with the
     * arguments the listing names.
     */
    public static class Ide {
        private final String url;
        private final String token; // null = none
        private final HttpClient http = HttpClient.newBuilder().connectTimeout(Duration.ofSeconds(30)).build();

        public Ide(String url, String token) {
            this.url = url.replaceAll("/+$", "");
            this.token = token;
        }

        /** Every tool, with its JSON schema. */
        public String list() throws IOException, InterruptedException {
            return send("GET", "/tools", null);
        }

        /** Run one tool by name; {@code argsJson} is the JSON object its schema names. */
        public String call(String name, String argsJson) throws IOException, InterruptedException {
            return send("POST", "/tools/" + seg(name), argsJson == null ? "{}" : argsJson);
        }

        public String read(String path) throws IOException, InterruptedException {
            return call("read", obj("path", path));
        }

        public String write(String path, String content) throws IOException, InterruptedException {
            return call("write", obj("path", path, "content", content));
        }

        /** Apply a unified diff; it never applies in part. A null cwd means the working directory. */
        public String patch(String diff, String cwd) throws IOException, InterruptedException {
            return call("patch", obj("diff", diff, "cwd", cwd));
        }

        public String glob(String pattern, String cwd) throws IOException, InterruptedException {
            return call("glob", obj("pattern", pattern, "cwd", cwd));
        }

        public String grep(String pattern, String cwd) throws IOException, InterruptedException {
            return call("grep", obj("pattern", pattern, "cwd", cwd));
        }

        /** Run a shell command and wait; the schema's other knobs go through {@link #call}. */
        public String exec(String cmd) throws IOException, InterruptedException {
            return call("exec", obj("cmd", cmd));
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

    /**
     * One container of an environment, built fluently and rendered with every field the server
     * requires: {@code new Service("db", "postgres:16").port(5432).env("POSTGRES_PASSWORD", "dev")
     * .mount("pgdata", "/var/lib/postgresql/data")}. A mount's folder is a folder of the
     * environment's own disk.
     */
    public static class Service {
        private final String name;
        private final String image;
        private final List<String> ports = new ArrayList<>();
        private final List<String> env = new ArrayList<>();
        private final List<String> command = new ArrayList<>();
        private final List<String> mounts = new ArrayList<>();

        public Service(String name, String image) {
            this.name = name;
            this.image = image;
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
            return "{\"name\":" + quote(name) + ",\"image\":" + quote(image)
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
        public String create(String env, Service... services) throws IOException, InterruptedException {
            return create(env, null, null, services);
        }

        /**
         * A null {@code quotaGb} means the default. {@code snapshotId} grows it from a pushed
         * snapshot — also one of a deleted environment — and then an empty {@code services}
         * means the ones that snapshot froze.
         */
        public String create(String env, Long quotaGb, String snapshotId, Service... services)
            throws IOException, InterruptedException {
            return request("POST", "/v1/environments", body(env, quotaGb, snapshotId, services, true));
        }

        /** The caller's environments and their teams'; a non-null owner narrows to one handle or team. */
        public String list(String owner) throws IOException, InterruptedException {
            return request("GET", owner == null ? "/v1/environments" : "/v1/environments?owner=" + seg(owner), null);
        }

        public String get(String env) throws IOException, InterruptedException {
            return request("GET", path(env, ""), null);
        }

        /** Replace the WHOLE service list. A removed service keeps its folders, so re-adding finds its data. */
        public String setServices(String env, Service... services) throws IOException, InterruptedException {
            return request("PATCH", path(env, ""), "{\"services\":" + array(services) + "}");
        }

        public String start(String env) throws IOException, InterruptedException {
            return request("POST", path(env, "/start"), null);
        }

        public String stop(String env) throws IOException, InterruptedException {
            return request("POST", path(env, "/stop"), null);
        }

        /** Reset the idle clock — call it periodically while something depends on the services. */
        public String hold(String env) throws IOException, InterruptedException {
            return request("POST", path(env, "/hold"), null);
        }

        /** The volume survives while a pushed snapshot references it. */
        public String delete(String env) throws IOException, InterruptedException {
            return request("DELETE", path(env, ""), null);
        }

        /** Cut a SNAPSHOT of every service's data — the one durable, restorable state. */
        public String push(String env, String message) throws IOException, InterruptedException {
            return request("POST", path(env, "/push"), message == null ? null : obj("message", message));
        }

        /** A new environment {@code to} ("name" or "team/name"), copied from the live disk. */
        public String clone_(String env, String to) throws IOException, InterruptedException {
            return clone_(env, to, null);
        }

        /** As above, but from one of this environment's snapshots when {@code snapshotId} is set. */
        public String clone_(String env, String to, String snapshotId) throws IOException, InterruptedException {
            return request("POST", path(env, "/clone"), body(to, null, snapshotId, new Service[0], false));
        }

        /**
         * Put THIS environment back to one of its own snapshots, services and data (202). A
         * persistent service the snapshot lacks is a 409 naming it; list it in {@code discard}
         * to accept losing its data.
         */
        public String restore(String env, String snapshotId, String... discard) throws IOException, InterruptedException {
            List<String> d = new ArrayList<>();
            for (String x : discard) d.add(quote(x));
            return request("POST", path(env, "/restore"),
                "{\"snapshot_id\":" + quote(snapshotId) + ",\"discard\":[" + String.join(",", d) + "]}");
        }

        private String path(String env, String tail) {
            String[] on = owned(env);
            return "/v1/environments/" + seg(on[1]) + tail + (on[0] == null ? "" : "?owner=" + seg(on[0]));
        }

        private String body(String env, Long quotaGb, String snapshotId, Service[] services, boolean always) {
            String[] on = owned(env);
            String b = obj("name", on[1], "owner", on[0], "snapshot_id", snapshotId);
            StringBuilder out = new StringBuilder(b.substring(0, b.length() - 1));
            if (quotaGb != null) out.append(",\"quota_gb\":").append(quotaGb);
            if (always || services.length > 0) out.append(",\"services\":").append(array(services));
            return out.append('}').toString();
        }

        private String array(Service[] services) {
            List<String> parts = new ArrayList<>();
            for (Service sv : services) parts.add(sv.toJson());
            return "[" + String.join(",", parts) + "]";
        }

        /** "name" → {null, name}; "team/name" → {team, name}. */
        private String[] owned(String s) {
            int i = s.lastIndexOf('/');
            if (i < 0) return new String[] {null, s};
            String owner = s.substring(0, i), name = s.substring(i + 1);
            if (owner.isEmpty() || name.isEmpty() || owner.contains("/")) {
                throw new IllegalArgumentException("expected name or team/name, got " + s);
            }
            return new String[] {owner, name};
        }
    }

    /**
     * The btrfs volumes behind workspaces and environments: snapshot history, restore targets,
     * and the two deletes — over {@code /v1/volumes}.
     */
    public class Volumes {
        /** Either filter may be null: kind is "workspace" or "environment", owner a handle or team slug. */
        public String list(String kind, String owner) throws IOException, InterruptedException {
            StringBuilder p = new StringBuilder("/v1/volumes");
            char sep = '?';
            if (kind != null) {
                p.append(sep).append("kind=").append(seg(kind));
                sep = '&';
            }
            if (owner != null) {
                p.append(sep).append("owner=").append(seg(owner));
            }
            return request("GET", p.toString(), null);
        }

        /** The chain of PUSHED snapshots, newest first — each row a restore target. */
        public String history(String name) throws IOException, InterruptedException {
            return request("GET", "/v1/volumes/" + seg(name) + "/history", null);
        }

        public String refs(String name) throws IOException, InterruptedException {
            return request("GET", "/v1/volumes/" + seg(name) + "/refs", null);
        }

        /**
         * Refused (409) for a sync point or a running worktree's base; deleting a detached
         * volume's last snapshot deletes the volume.
         */
        public String deleteSnapshot(String name, String snapshotId) throws IOException, InterruptedException {
            return request("DELETE", "/v1/volumes/" + seg(name) + "/snapshots/" + seg(snapshotId), null);
        }

        /** Takes a DETACHED volume with all its snapshots; refused while a working copy remains. */
        public String delete(String name) throws IOException, InterruptedException {
            return request("DELETE", "/v1/volumes/" + seg(name), null);
        }
    }

    // ── small helpers, so a one-file SDK needs no JSON dependency ──────────

    /** "owner/name" → its two halves, refused client-side when malformed. */
    static String[] split(String repo) {
        int at = repo == null ? -1 : repo.indexOf('/');
        if (at <= 0 || at == repo.length() - 1 || repo.indexOf('/', at + 1) >= 0) {
            throw new IllegalArgumentException("a repository is named owner/name, got " + repo);
        }
        return new String[] { repo.substring(0, at), repo.substring(at + 1) };
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
