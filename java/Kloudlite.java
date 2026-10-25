// Kloudlite SDK for Java — one file, no dependencies (java.net.http). Needs Java 11 or later.
//
// Auth is a Bearer token (a CLI token). Every module hangs off the one client:
//
//   Kloudlite kl = new Kloudlite();   // the login of `kloudlite`; see "Discovery" below
//   String workspaces = kl.workspaces().list(null);
//
// Discovery: the API origin and the token each come from the first source that has one:
// 1. Options.baseUrl() / Options.token(); 2. the environment KLOUDLITE_URL / KLOUDLITE_TOKEN;
// 3. "api" / "token" in config.json in KLOUDLITE_CONFIG_DIR, else ~/.config/kloudlite. "/v1" is
// added to the origin. With no token the constructor throws IllegalStateException carrying the
// sentence "no Kloudlite login: pass token(), set KLOUDLITE_TOKEN, or run kloudlite login".
//
// Java has no named arguments, so no call ever takes two inputs by position unless both are
// plain names (get("acme/dev"), connect("dev", "shop")): every other method takes ONE params
// object — a nested class named <Verb><Noun> with a fluent setter per field. A missing
// required field is an IllegalArgumentException("<field> is required") before anything is sent.
//
// Methods return the raw JSON body as a String; bring your own JSON mapper. The two
// exceptions are sandboxes() and pipelines().run/runs/getRun, which hand back live handles
// (SandboxHandle, Run) — each still exposes its raw JSON.

import java.io.IOException;
import java.io.InterruptedIOException;
import java.io.UncheckedIOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.net.URI;
import java.net.URLEncoder;
import java.net.http.HttpClient;
import java.net.http.HttpHeaders;
import java.net.http.HttpRequest;
import java.net.http.HttpResponse;
import java.net.http.HttpTimeoutException;
import java.nio.charset.StandardCharsets;
import java.time.Duration;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.Iterator;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.NoSuchElementException;
import java.util.Spliterator;
import java.util.Spliterators;
import java.util.stream.Stream;
import java.util.stream.StreamSupport;

public class Kloudlite {
    public static final String VERSION = "0.2.0";
    static final String NO_LOGIN = "no Kloudlite login: pass token(), set KLOUDLITE_TOKEN, or run kloudlite login";

    static {
        String v = System.getProperty("java.specification.version", "0");
        if (v.startsWith("1.") || Integer.parseInt(v) < 11) {
            throw new IllegalStateException("Kloudlite.java needs Java 11 or later, found " + v);
        }
    }

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

    /** How to reach the API; every field is optional and falls back as the header's "Discovery" says. */
    public static final class Options {
        private String baseUrl, token;

        public Options baseUrl(String v) { baseUrl = v; return this; }
        public Options token(String v) { token = v; return this; }
    }

    private final String baseUrl;
    private final String token;
    private final HttpClient http;
    private final Workspaces workspaces = new Workspaces();
    private final Environments environments = new Environments();
    private final Sessions sessions = new Sessions();
    private final Volumes volumes = new Volumes();
    private final Secrets secrets = new Secrets();
    private final Connectors connectors = new Connectors();
    private final Sandboxes sandboxes = new Sandboxes();
    private final Pipelines pipelines = new Pipelines();

    /** The login of {@code kloudlite}: argument-free discovery. */
    public Kloudlite() {
        this(new Options());
    }

    public Kloudlite(Options opts) {
        String url = blankToNull(opts.baseUrl), tok = blankToNull(opts.token);
        if (url == null) url = blankToNull(System.getenv("KLOUDLITE_URL"));
        if (tok == null) tok = blankToNull(System.getenv("KLOUDLITE_TOKEN"));
        if (url == null || tok == null) {
            Map<String, Object> cfg = login();
            if (url == null) url = blankToNull(text(cfg.get("api")));
            if (tok == null) tok = blankToNull(text(cfg.get("token")));
        }
        if (tok == null) throw new IllegalStateException(NO_LOGIN);
        if (url == null) {
            throw new IllegalStateException(
                "no Kloudlite API URL: pass baseUrl(), set KLOUDLITE_URL, or run kloudlite login");
        }
        url = url.replaceAll("/+$", "");
        this.baseUrl = url.endsWith("/v1") ? url.substring(0, url.length() - 3) : url; // paths carry /v1
        this.token = tok;
        this.http = HttpClient.newBuilder().connectTimeout(Duration.ofSeconds(30)).build();
    }

    private static String blankToNull(String v) {
        return v == null || v.isEmpty() ? null : v;
    }

    /** {@code config.json} of the {@code kloudlite} login; empty when it is absent or unreadable. */
    private static Map<String, Object> login() {
        String dir = blankToNull(System.getenv("KLOUDLITE_CONFIG_DIR"));
        Path p = dir != null ? Path.of(dir) : Path.of(System.getProperty("user.home"), ".config", "kloudlite");
        try {
            return JsonReader.parsed(Files.readString(p.resolve("config.json")));
        } catch (IOException | RuntimeException e) {
            return Map.of();
        }
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

    /** The provider accounts linked to an owner: {@code /v1/connectors}. */
    public Connectors connectors() {
        return connectors;
    }

    /** Throwaway gVisor sandboxes that run one command: {@code /v1/sandboxes}. */
    public Sandboxes sandboxes() {
        return sandboxes;
    }

    /** Pipelines and their runs: {@code /v1/pipelines}. */
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
        return api(method, path, null);
    }

    public String request(RawRequest r) throws IOException, InterruptedException {
        return api(need(r.method, "method"), r.target(), r.body);
    }

    private String api(String method, String path, String jsonBody) throws IOException, InterruptedException {
        return exchange(method, path, jsonBody, "application/json", Duration.ofSeconds(30)).body();
    }

    /** One round trip; {@code timeout} null means none (a followed log). */
    private HttpResponse<String> exchange(String method, String path, String jsonBody, String accept,
            Duration timeout) throws IOException, InterruptedException {
        HttpRequest.Builder b = HttpRequest.newBuilder()
            .uri(URI.create(baseUrl + path))
            .header("User-Agent", "kloudlite-sdk-java/" + VERSION)
            .header("Accept", accept)
            .method(method, jsonBody == null
                ? HttpRequest.BodyPublishers.noBody()
                : HttpRequest.BodyPublishers.ofString(jsonBody));
        if (timeout != null) b.timeout(timeout);
        if (jsonBody != null) b.header("Content-Type", "application/json");
        b.header("Authorization", "Bearer " + token);
        HttpResponse<String> resp = http.send(b.build(), HttpResponse.BodyHandlers.ofString());
        if (resp.statusCode() < 200 || resp.statusCode() > 299) {
            throw new ApiException(resp.statusCode(), errorText(resp.body()));
        }
        return resp;
    }

    /**
     * Btrfs-backed dev workspaces, over {@code /v1/workspaces}. A workspace is named by ONE
     * string — {@code "name"} for the caller's own, {@code "team/name"} for a team's — or by its
     * id; pushes cut snapshots, kept on the workspace's VOLUME — see {@link Volumes} for
     * history, restore targets and deletion.
     */
    public class Workspaces {
        /** {@code workspace} ("name" or "team/name") is split into the body's {@code name} and {@code team}. 202. */
        public String create(CreateWorkspace p) throws IOException, InterruptedException {
            String[] on = owned(need(p.workspace, "workspace"));
            return api("POST", "/v1/workspaces", new Json()
                .str("name", on[1])
                .num("quota_gb", need(p.quotaGb, "quotaGb"))
                .str("team", on[0])
                .str("image", p.image)
                .raw("packages", p.packages == null ? null : strings(p.packages))
                .num("ttl_secs", p.ttlSecs)
                .raw("env", p.env == null ? null : stringMap(p.env))
                .toString());
        }

        /** Replace the whole env (literals or {@code "${{ secrets.NAME }}"}); takes effect on the next start. */
        public String setEnv(String workspace, Map<String, String> env) throws IOException, InterruptedException {
            return setEnv(workspace, env, null);
        }

        /** As {@link #setEnv(String, Map)}; {@code restartOnEnvChange} true also restarts a running workspace. */
        public String setEnv(String workspace, Map<String, String> env, Boolean restartOnEnvChange)
                throws IOException, InterruptedException {
            return api("PUT", path(need(workspace, "workspace"), "/env"), new Json()
                .raw("env", stringMap(need(env, "env")))
                .bool("restartOnEnvChange", restartOnEnvChange).toString());
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

        /** A stop cuts a sync point and tears the workspace down — seconds, never waiting on a replica. */
        public String stop(String workspace) throws IOException, InterruptedException {
            return api("POST", path(workspace, "/stop"), null);
        }

        /** Restart a running workspace so it gets the current env (202); 409 when not running. */
        public String restart(String workspace) throws IOException, InterruptedException {
            return api("POST", path(workspace, "/restart"), null);
        }

        /**
         * Push a SNAPSHOT of the working copy — the one durable, restorable state (202).
         * {@code volume} names the volume and is required on its first push.
         */
        public String takeSnapshot(TakeWorkspaceSnapshot p) throws IOException, InterruptedException {
            return api("POST", path(need(p.workspace, "workspace"), "/snapshots"), new Json()
                .str("message", p.message).str("volume", p.volume)
                .num("ttl_secs", p.ttlSecs).bool("never", p.never).toString());
        }

        /** The old name of {@link #takeSnapshot}. */
        public String push(TakeWorkspaceSnapshot p) throws IOException, InterruptedException {
            return takeSnapshot(p);
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
         * A new workspace {@code to} grafted onto a PUSHED snapshot (a volume name, {@code name@N}
         * or a snapshot id) — also one whose workspace is long gone. {@code team} (or
         * {@code to} as "team/name") is required for a team's volume. Unset overrides mean
         * "whatever the snapshot froze".
         */
        public String restore(RestoreWorkspace p) throws IOException, InterruptedException {
            String snapshot = need(p.snapshot, "snapshot");
            String[] on = owned(need(p.to, "to"));
            return api("POST", "/v1/workspaces/restore", new Json()
                .str("name", on[1])
                .str("snapshot_id", snapshot)
                .str("team", p.team != null ? p.team : on[0])
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
         * An {@link Ide} bound to this workspace's tool server. The address is a private IP:
         * reachable from inside Kloudlite (a bench); from a laptop, tunnel with
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
        private String workspace, image;
        private Long quotaGb, ttlSecs;
        private List<String> packages;
        private Map<String, String> env;

        public CreateWorkspace workspace(String v) { workspace = v; return this; }
        public CreateWorkspace quotaGb(long v) { quotaGb = v; return this; }
        public CreateWorkspace image(String v) { image = v; return this; }
        /** nixpkgs attributes, optionally pinned ({@code attr@version}). */
        public CreateWorkspace packages(String... v) { packages = Arrays.asList(v); return this; }
        /** Seconds unused (600..=2592000) before the workspace is deleted; unset, never. */
        public CreateWorkspace ttlSecs(long v) { ttlSecs = v; return this; }
        /** Variable to a literal value or a {@code "${{ secrets.NAME }}"} reference to the owner's secrets. */
        public CreateWorkspace env(Map<String, String> v) { env = v; return this; }
    }

    /** {@link Workspaces#takeSnapshot}: {@code ttlSecs} (3600..=31536000) or {@code never(true)} set the expiry. */
    public static final class TakeWorkspaceSnapshot {
        private String workspace, message, volume;
        private Long ttlSecs;
        private Boolean never;

        public TakeWorkspaceSnapshot workspace(String v) { workspace = v; return this; }
        public TakeWorkspaceSnapshot message(String v) { message = v; return this; }
        public TakeWorkspaceSnapshot volume(String v) { volume = v; return this; }
        public TakeWorkspaceSnapshot ttlSecs(long v) { ttlSecs = v; return this; }
        public TakeWorkspaceSnapshot never(boolean v) { never = v; return this; }
    }

    public static final class CloneWorkspace {
        private String workspace, to, task;

        public CloneWorkspace workspace(String v) { workspace = v; return this; }
        public CloneWorkspace to(String v) { to = v; return this; }
        public CloneWorkspace task(String v) { task = v; return this; }
    }

    /** {@link Workspaces#restore}: {@code snapshot} is sent as {@code snapshot_id}, {@code to} as {@code name} (and {@code team}). */
    public static final class RestoreWorkspace {
        private String snapshot, to, team, image;
        private List<String> packages;
        private Long quotaGb;

        public RestoreWorkspace snapshot(String v) { snapshot = v; return this; }
        public RestoreWorkspace to(String v) { to = v; return this; }
        public RestoreWorkspace team(String v) { team = v; return this; }
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
                body(need(p.environment, "environment"), p.quotaGb, p.snapshot, p.services, p.ttlSecs));
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
            return api("PATCH", path(env, ""), new Json()
                .raw("services", array(need(p.services, "services")))
                .bool("restartOnEnvChange", p.restartOnEnvChange).toString());
        }

        public String start(String environment) throws IOException, InterruptedException {
            return api("POST", path(environment, "/start"), null);
        }

        public String stop(String environment) throws IOException, InterruptedException {
            return api("POST", path(environment, "/stop"), null);
        }

        /** Restart one service so it gets the current env (202); 409 when the environment is not running. */
        public String restart(String environment, String service) throws IOException, InterruptedException {
            return api("POST", path(need(environment, "environment"),
                "/services/" + seg(need(service, "service")) + "/restart"), null);
        }

        /** Seconds unused before the environment is deleted (600..=2592000); {@code null} clears it. */
        public String setTtl(String environment, Long ttlSecs) throws IOException, InterruptedException {
            return api("PUT", path(need(environment, "environment"), "/ttl"), "{\"ttl_secs\":" + ttlSecs + "}");
        }

        /** The volume survives while a pushed snapshot references it. */
        public String delete(String environment) throws IOException, InterruptedException {
            return api("DELETE", path(environment, ""), null);
        }

        /**
         * Push a SNAPSHOT of every service's data and the service list (202). {@code volume} names
         * the volume and is required on its first push.
         */
        public String takeSnapshot(TakeEnvironmentSnapshot p) throws IOException, InterruptedException {
            return api("POST", path(need(p.environment, "environment"), "/snapshots"), new Json()
                .str("message", p.message).str("volume", p.volume)
                .num("ttl_secs", p.ttlSecs).bool("never", p.never).toString());
        }

        /** The old name of {@link #takeSnapshot}. */
        public String push(TakeEnvironmentSnapshot p) throws IOException, InterruptedException {
            return takeSnapshot(p);
        }

        /**
         * A new environment {@code to} ("name" or "team/name"): a copy of the live disk, or of one
         * of this environment's snapshots when {@code snapshot} is set ({@code services} and
         * {@code quotaGb} apply only then).
         */
        public String clone(CloneEnvironment p) throws IOException, InterruptedException {
            String env = need(p.environment, "environment");
            return api("POST", path(env, "/clone"), body(need(p.to, "to"), p.quotaGb, p.snapshot, p.services, p.ttlSecs));
        }

        /**
         * Put THIS environment back to one of its own snapshots, services and data (202). A
         * persistent service the snapshot lacks is a 409 naming it; list it in {@code discard}
         * to accept losing its data.
         */
        public String restore(RestoreEnvironment p) throws IOException, InterruptedException {
            String env = need(p.environment, "environment"), snapshot = need(p.snapshot, "snapshot");
            return api("POST", path(env, "/restore"),
                new Json().str("snapshot_id", snapshot)
                    .raw("discard", p.discard.isEmpty() ? null : strings(p.discard)).toString());
        }

        private String path(String environment, String tail) {
            return at("/v1/environments/", environment, tail);
        }

        private String body(String env, Long quotaGb, String snapshot, List<Service> services, Long ttlSecs) {
            String[] on = owned(env);
            String b = obj("name", on[1], "owner", on[0], "snapshot_id", snapshot);
            StringBuilder out = new StringBuilder(b.substring(0, b.length() - 1));
            if (quotaGb != null) out.append(",\"quota_gb\":").append(quotaGb);
            if (!services.isEmpty()) out.append(",\"services\":").append(array(services));
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
        private Boolean restartOnEnvChange;

        public SetServices environment(String v) { environment = v; return this; }
        public SetServices services(Service... v) { services = Arrays.asList(v); return this; }
        /** True also restarts a service whose env changed; unset, it is only marked {@code restartRequired}. */
        public SetServices restartOnEnvChange(boolean v) { restartOnEnvChange = v; return this; }
    }

    /** {@link Environments#takeSnapshot}: {@code ttlSecs} (3600..=31536000) or {@code never(true)} set the expiry. */
    public static final class TakeEnvironmentSnapshot {
        private String environment, message, volume;
        private Long ttlSecs;
        private Boolean never;

        public TakeEnvironmentSnapshot environment(String v) { environment = v; return this; }
        public TakeEnvironmentSnapshot message(String v) { message = v; return this; }
        public TakeEnvironmentSnapshot volume(String v) { volume = v; return this; }
        public TakeEnvironmentSnapshot ttlSecs(long v) { ttlSecs = v; return this; }
        public TakeEnvironmentSnapshot never(boolean v) { never = v; return this; }
    }

    public static final class CloneEnvironment {
        private String environment, to, snapshot;
        private List<Service> services = List.of();
        private Long quotaGb, ttlSecs;

        public CloneEnvironment environment(String v) { environment = v; return this; }
        public CloneEnvironment to(String v) { to = v; return this; }
        public CloneEnvironment snapshot(String v) { snapshot = v; return this; }
        public CloneEnvironment services(Service... v) { services = Arrays.asList(v); return this; }
        public CloneEnvironment quotaGb(long v) { quotaGb = v; return this; }
        public CloneEnvironment ttlSecs(long v) { ttlSecs = v; return this; }
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

        /** Stop the session and every workspace in it (202, {@code {"stopped": [ids]}}); sandboxes and the environment are untouched. */
        public String stop(String session) throws IOException, InterruptedException {
            return api("POST", path(session, "/stop"), null);
        }

        /**
         * Create a workspace {@code p.workspace} (a bare name) managed by the session, in its team
         * (201). The session controls it from then on; it takes the inputs of
         * {@link Workspaces#create}.
         */
        public String createWorkspace(String session, CreateWorkspace p) throws IOException, InterruptedException {
            return api("POST", path(need(session, "session"), "/workspaces"), new Json()
                .str("name", need(p.workspace, "workspace"))
                .num("quota_gb", need(p.quotaGb, "quotaGb"))
                .str("image", p.image)
                .raw("packages", p.packages == null ? null : strings(p.packages))
                .num("ttl_secs", p.ttlSecs)
                .raw("env", p.env == null ? null : stringMap(p.env))
                .toString());
        }

        /** Delete a workspace the session manages, releasing its intercepts first (204); snapshots stay. */
        public String deleteWorkspace(String session, String workspace) throws IOException, InterruptedException {
            return api("DELETE", path(need(session, "session"), "/workspaces/" + seg(need(workspace, "workspace"))), null);
        }

        /**
         * Create a sandbox managed by the session (202), in its team and reaching its
         * environment. Same inputs as {@link Sandboxes#create} ({@code team} must be the session's).
         */
        public SandboxHandle createSandbox(String session, CreateSandbox p) throws IOException, InterruptedException {
            String body = p.body();
            return new SandboxHandle(api("POST", path(need(session, "session"), "/sandboxes"), body));
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
     * {@code "${{ secrets.NAME }}"}. {@code scope} is "mine" (the default: your own secret in the
     * team {@code owner}, or your personal one for your own handle) or "team" (the team's shared
     * secret; needs an admin).
     */
    public class Secrets {
        public void set(String owner, String name, String value) throws IOException, InterruptedException {
            set(owner, name, value, "mine");
        }

        public void set(String owner, String name, String value, String scope) throws IOException, InterruptedException {
            api("PUT", secretPath(owner, scope, need(name, "name")), "{\"value\":" + quote(need(value, "value")) + "}");
        }

        public String list(String owner) throws IOException, InterruptedException {
            return list(owner, "mine");
        }

        public String list(String owner, String scope) throws IOException, InterruptedException {
            return api("GET", secretPath(owner, scope, null), null);
        }

        /** A 409 while a workspace or environment still references it; the body names them. */
        public void delete(String owner, String name) throws IOException, InterruptedException {
            delete(owner, name, "mine");
        }

        public void delete(String owner, String name, String scope) throws IOException, InterruptedException {
            api("DELETE", secretPath(owner, scope, need(name, "name")), null);
        }

        private String secretPath(String owner, String scope, String name) {
            if (!"mine".equals(scope) && !"team".equals(scope)) {
                throw new IllegalArgumentException("scope is \"mine\" or \"team\", got " + scope);
            }
            return "/v1/secrets/" + seg(need(owner, "owner")) + ("mine".equals(scope) ? "/mine" : "")
                + (name == null ? "" : "/" + seg(name));
        }
    }

    /** The provider accounts linked to an owner, over {@code /v1/connectors}. */
    public class Connectors {
        /** Connectors by name; a non-null {@code owner} (a handle or team slug) replaces your own. */
        public String list(String owner) throws IOException, InterruptedException {
            return api("GET", "/v1/connectors" + query("owner", owner), null);
        }

        /** Remove a connector (204); a team's needs an admin. */
        public void delete(String name) throws IOException, InterruptedException {
            api("DELETE", "/v1/connectors/" + seg(need(name, "name")), null);
        }
    }

    /**
     * The btrfs volumes behind workspaces and environments: snapshot history, restore targets,
     * and the deletes — over {@code /v1/volumes}.
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

        /** Takes a DETACHED volume (by id) with all its snapshots; refused while a working copy remains. */
        public String deleteById(String volume) throws IOException, InterruptedException {
            return api("DELETE", "/v1/volumes/" + seg(need(volume, "volume")), null);
        }

        /** One named volume; {@code owner} is your handle or a team slug. */
        public String get(String owner, String name) throws IOException, InterruptedException {
            return api("GET", named(owner, name, ""), null);
        }

        /** The versions of a named volume, newest first. */
        public String versions(String owner, String name) throws IOException, InterruptedException {
            return api("GET", named(owner, name, "/versions"), null);
        }

        /** Delete a named volume and every snapshot on it (204); the name is free again. */
        public String delete(String owner, String name) throws IOException, InterruptedException {
            return api("DELETE", named(owner, name, ""), null);
        }

        /** Delete version {@code n} (204); deleting the last one deletes the volume. */
        public String deleteVersion(String owner, String name, int n) throws IOException, InterruptedException {
            return api("DELETE", named(owner, name, "/versions/" + n), null);
        }

        private String named(String owner, String name, String tail) {
            return "/v1/volumes/" + seg(need(owner, "owner")) + "/" + seg(need(name, "name")) + tail;
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
     * Throwaway sandboxes, over {@code /v1/sandboxes}: one isolated machine that runs ONE command
     * ({@code cmd}) and ends when it exits or after its own timeout. A sandbox is named by its id.
     * Unlike most modules these calls hand back a {@link SandboxHandle}, because a sandbox is
     * waited on, not read once; it still exposes its raw JSON.
     *
     * <pre>
     *   Kloudlite.SandboxHandle sb = kl.sandboxes().create(new Kloudlite.CreateSandbox()
     *       .cmd(List.of("python3", "-c", "print(6 * 7)")).packages("python3"));
     *   Integer code = sb.waitFor(null);
     *   System.out.print(sb.logs());
     * </pre>
     */
    public class Sandboxes {
        /** Returns at once (202), the sandbox usually still {@code pending}. */
        public SandboxHandle create(CreateSandbox p) throws IOException, InterruptedException {
            return new SandboxHandle(api("POST", "/v1/sandboxes", p.body()));
        }

        /** One sandbox by id (a name does not resolve). */
        public SandboxHandle get(String sandbox) throws IOException, InterruptedException {
            return new SandboxHandle(api("GET", "/v1/sandboxes/" + seg(need(sandbox, "sandbox")), null));
        }

        /** ONE page of the caller's sandboxes, newest first; a non-null team scopes to that team. {@link #listPage} picks the page. */
        public List<SandboxHandle> list(String team) throws IOException, InterruptedException {
            return listPage(new ListSandboxes().team(team)).sandboxes;
        }

        /** A page ({@code limit} 1..=200, default 50) and the cursor of the next one. */
        public SandboxPage listPage(ListSandboxes p) throws IOException, InterruptedException {
            HttpResponse<String> resp = exchange("GET", "/v1/sandboxes"
                + query("team", p.team, "limit", p.limit == null ? null : p.limit.toString(), "cursor", p.cursor),
                null, "application/json", Duration.ofSeconds(30));
            List<SandboxHandle> out = new ArrayList<>();
            List<String> docs;
            try {
                docs = JsonReader.elements(resp.body());
            } catch (IllegalArgumentException e) {
                throw new ApiException(502, "the sandbox listing was not a JSON array");
            }
            for (String doc : docs) out.add(new SandboxHandle(doc));
            return new SandboxPage(out, resp.headers().firstValue("X-Next-Cursor").orElse(null));
        }

        /**
         * One command in a fresh sandbox: create, wait for it to exit, read the output, and
         * terminate — ALWAYS, also when anything before it failed (a failed terminate surfaces
         * beside, never instead of, the failure that came first). The sandbox keeps one stream,
         * so {@code stdout} holds the output and error output together and {@code stderr} is "".
         */
        public RunResult run(RunSandbox p) throws IOException, InterruptedException {
            // A command allowed longer than the sandbox's default lifetime must not be cut by it:
            // the sandbox outlives the wait by a minute, within the server's 10 s..24 h.
            if (p.timeout != null && !p.timeout.isZero()) {
                long secs = Math.min(86_400, Math.max(10, (p.timeout.toMillis() + 999) / 1000 + 60));
                p.create.timeout(Duration.ofSeconds(secs));
            }
            SandboxHandle sb = create(p.create);
            Throwable failed = null;
            try {
                Integer code = sb.waitFor(p.timeout);
                return new RunResult(code, sb.logs(), "", sb.id());
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

    /** What {@link Sandboxes#listPage} answers: the page and the {@code X-Next-Cursor} for the next (null on the last). */
    public static final class SandboxPage {
        public final List<SandboxHandle> sandboxes;
        public final String nextCursor;

        SandboxPage(List<SandboxHandle> sandboxes, String nextCursor) {
            this.sandboxes = sandboxes;
            this.nextCursor = nextCursor;
        }
    }

    /**
     * One sandbox: the doc as last read ({@link #json}, with {@link #id}, {@link #phase} —
     * {@code pending|starting|running|terminated} — and, once terminated, {@link #reason} and
     * {@link #exitCode}). Nothing re-reads it behind the caller's back except the waits;
     * {@link #refresh} does.
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
        /** The command's exit code; null until it has exited (and when the sandbox ended some other way). */
        public Integer exitCode() {
            Long c = longOf(doc.get("exit_code"));
            return c == null ? null : (Integer) c.intValue();
        }
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

        /** The main process's output (stdout and stderr together): live while it runs, the last 256 KiB after. 409 while there is none yet. */
        public String logs() throws IOException, InterruptedException {
            return logs(false);
        }

        /** {@code follow} true holds until the process ends. */
        public String logs(boolean follow) throws IOException, InterruptedException {
            return exchange("GET", path("/logs") + (follow ? "?follow=true" : ""), null, "text/plain",
                follow ? null : Duration.ofSeconds(30)).body();
        }

        /** Not in this SDK: {@code GET /v1/sandboxes/{id}/attach} is a WebSocket and this client uses only the JDK. */
        public void attach() {
            throw new UnsupportedOperationException("sandbox attach is a WebSocket and is not in the Java SDK");
        }

        /** Stop it for good (202, or 200 if already ended); the record stays until {@link #delete}. Returns this handle. */
        public SandboxHandle terminate() throws IOException, InterruptedException {
            String raw = api("POST", path("/terminate"), null);
            if (raw != null && !raw.isBlank()) set(raw);
            return this;
        }

        /** {@link #waitFor(Duration)} with no limit. */
        public Integer waitFor() throws IOException, InterruptedException {
            return waitFor(null);
        }

        /**
         * Block until it has ended and answer its exit code (null when it ended some other way
         * than the command exiting; {@link #reason} says how). Waits in steps of at most 20 s, the
         * most the api holds one open; a null or zero limit waits as long as it takes and the
         * limit passing is an {@link HttpTimeoutException}.
         */
        public Integer waitFor(Duration timeout) throws IOException, InterruptedException {
            long deadline = deadline(timeout);
            while (true) {
                long left = Math.min(20_000, remainingMs(deadline));
                if (left <= 0) {
                    throw new HttpTimeoutException("sandbox " + id() + " still " + phase() + " after " + timeout);
                }
                Map<String, Object> out = JsonReader.parsed(api("POST", path("/wait"),
                    new Json().num("timeout_ms", left).toString()));
                Map<String, Object> merged = new LinkedHashMap<>(doc);
                for (String k : new String[] {"phase", "reason", "exit_code", "message"}) {
                    if (out.containsKey(k)) merged.put(k, out.get(k));
                }
                doc = merged;
                json = JsonReader.write(merged);
                if ("terminated".equals(phase())) return exitCode();
            }
        }

        /** Delete the record (204); a sandbox still running dies with it. */
        public void delete() throws IOException, InterruptedException {
            api("DELETE", path(""), null);
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

    // ── sandbox params ─────────────────────────────────────────────────────

    /**
     * {@link Sandboxes#create}: {@code cmd} is required, every other field optional, an unset one
     * being the server's default (a 300 s timeout, 1 core, 2 GB of memory, 10 GB of disk, the
     * internet reachable). {@code fromWorkspace} is an id, a name or "team/name";
     * {@code fromSnapshot} a volume name, {@code name@N} or snapshot id (exclusive with it).
     */
    public static final class CreateSandbox {
        private String image, fromWorkspace, fromSnapshot, team, network;
        private List<String> cmd, packages;
        private Boolean tty, stdin;
        private Duration timeout;
        private Map<String, String> env;
        private Long cpu, memoryGb, diskGb;

        /** The program, then its arguments: a non-empty list. */
        public CreateSandbox cmd(List<String> argv) { cmd = argv; return this; }
        public CreateSandbox image(String v) { image = v; return this; }
        /** nixpkgs attributes, optionally pinned ({@code attr@version}). */
        public CreateSandbox packages(String... v) { packages = Arrays.asList(v); return this; }
        public CreateSandbox fromWorkspace(String v) { fromWorkspace = v; return this; }
        public CreateSandbox fromSnapshot(String v) { fromSnapshot = v; return this; }
        public CreateSandbox team(String v) { team = v; return this; }
        /** Run the command on a pty. */
        public CreateSandbox tty(boolean v) { tty = v; return this; }
        /** Keep input open for {@code attach}. */
        public CreateSandbox stdin(boolean v) { stdin = v; return this; }
        /** The sandbox's whole lifetime (10 s..24 h), sent in whole seconds (rounded up). */
        public CreateSandbox timeout(Duration v) { timeout = v; return this; }
        /** "internet" or "none". */
        public CreateSandbox network(String v) { network = v; return this; }
        public CreateSandbox env(Map<String, String> v) { env = v; return this; }
        public CreateSandbox cpu(int cores) { cpu = (long) cores; return this; }
        public CreateSandbox memoryGb(int v) { memoryGb = (long) v; return this; }
        public CreateSandbox diskGb(int v) { diskGb = (long) v; return this; }

        String body() {
            if (cmd == null || cmd.isEmpty()) {
                throw new IllegalArgumentException(
                    "cmd is required: the program and its arguments as a non-empty list of strings");
            }
            return new Json()
                .raw("cmd", strings(cmd))
                .str("image", image)
                .raw("packages", packages == null ? null : strings(packages))
                .str("from_workspace", fromWorkspace)
                .str("from_snapshot", fromSnapshot)
                .str("team", team)
                .bool("tty", tty)
                .bool("stdin", stdin)
                .num("timeout_secs", seconds(timeout))
                .str("network", network)
                .raw("env", env == null ? null : stringMap(env))
                .num("cpu", cpu)
                .num("memory_gb", memoryGb)
                .num("disk_gb", diskGb)
                .toString();
        }
    }

    /** {@link Sandboxes#listPage}: every field optional; {@code limit} is 1..=200. */
    public static final class ListSandboxes {
        private String team, cursor;
        private Integer limit;

        public ListSandboxes team(String v) { team = v; return this; }
        public ListSandboxes limit(int v) { limit = v; return this; }
        public ListSandboxes cursor(String v) { cursor = v; return this; }
    }

    /**
     * {@link Sandboxes#run}: the command ({@code cmd} required), how long to wait for it
     * ({@code timeout}; unset waits until it exits, and a set one also sizes the sandbox's own
     * lifetime to cover it plus a minute), and every other {@link CreateSandbox} field.
     */
    public static final class RunSandbox {
        private final CreateSandbox create = new CreateSandbox();
        private Duration timeout;

        public RunSandbox cmd(List<String> argv) { create.cmd(argv); return this; }
        public RunSandbox timeout(Duration v) { timeout = v; return this; }
        public RunSandbox image(String v) { create.image(v); return this; }
        public RunSandbox packages(String... v) { create.packages(v); return this; }
        public RunSandbox fromWorkspace(String v) { create.fromWorkspace(v); return this; }
        public RunSandbox fromSnapshot(String v) { create.fromSnapshot(v); return this; }
        public RunSandbox team(String v) { create.team(v); return this; }
        public RunSandbox tty(boolean v) { create.tty(v); return this; }
        public RunSandbox stdin(boolean v) { create.stdin(v); return this; }
        public RunSandbox network(String v) { create.network(v); return this; }
        public RunSandbox env(Map<String, String> v) { create.env(v); return this; }
        public RunSandbox cpu(int cores) { create.cpu(cores); return this; }
        public RunSandbox memoryGb(int v) { create.memoryGb(v); return this; }
        public RunSandbox diskGb(int v) { create.diskGb(v); return this; }
    }

    /** What {@link Sandboxes#run} answers; {@link #toString} is its JSON. */
    public static final class RunResult {
        /** Null when the sandbox ended some other way than the command exiting. */
        public final Integer exitCode;
        /** The output and the error output together. */
        public final String stdout;
        /** Always "": the sandbox keeps one stream. */
        public final String stderr;
        /** The sandbox's id — already terminated. */
        public final String sandbox;

        RunResult(Integer exitCode, String stdout, String stderr, String sandbox) {
            this.exitCode = exitCode;
            this.stdout = stdout;
            this.stderr = stderr;
            this.sandbox = sandbox;
        }

        @Override
        public String toString() {
            return new Json().raw("exit_code", exitCode == null ? "null" : exitCode.toString())
                .str("stdout", stdout).str("stderr", stderr).str("sandbox", sandbox).toString();
        }
    }

    /**
     * Pipelines, over {@code /v1/pipelines}: a JSON definition stored as numbered versions; a run
     * executes one version, each job in its own sandbox. Definitions and documents travel as raw
     * JSON strings. {@link #run}, {@link #runs} and {@link #getRun} hand back {@link Run} handles.
     *
     * <pre>
     *   Kloudlite.Run run = kl.pipelines().run(new Kloudlite.RunPipeline()
     *       .owner("acme").pipeline("test").input("target", "staging"));
     *   try (Stream&lt;String&gt; log = run.follow("test")) { log.forEach(System.out::print); }
     *   String state = run.waitFor(); // "success"
     * </pre>
     */
    public class Pipelines {
        private String base(String owner, String name) {
            return "/v1/pipelines/" + seg(need(owner, "owner")) + (name == null ? "" : "/" + seg(name));
        }

        /**
         * Save {@code definition} (JSON; its {@code name} is the pipeline's) as version 1. A webhook
         * trigger's URL is in the answer's {@code webhookUrl}, this once. 409 if the name exists.
         */
        public String create(String owner, String definition) throws IOException, InterruptedException {
            return api("POST", base(owner, null), need(definition, "definition"));
        }

        /**
         * Replace the definition: a changed one is the next version, an equal one saves nothing.
         * To roll back, send an old definition. A concurrent save is an {@link ApiException} 409
         * ("changed; retry").
         */
        public String update(String owner, String name, String definition) throws IOException, InterruptedException {
            return api("PUT", base(owner, need(name, "name")), need(definition, "definition"));
        }

        /** {@code owner}'s pipelines by name: {@code [{name, latestVersion, updatedAtMs, triggers}]}. */
        public String list(String owner) throws IOException, InterruptedException {
            return api("GET", base(owner, null), null);
        }

        /** {@link #get(String, String, int)} of the latest version. */
        public String get(String owner, String name) throws IOException, InterruptedException {
            return api("GET", base(owner, need(name, "name")), null);
        }

        /** The pipeline document of {@code version}, {@code definition} included. */
        public String get(String owner, String name, int version) throws IOException, InterruptedException {
            return api("GET", base(owner, need(name, "name")) + query("version", Integer.toString(version)), null);
        }

        /** The versions, oldest first: {@code [{version, createdBy, createdAtMs, definitionSha256}]}. */
        public String versions(String owner, String name) throws IOException, InterruptedException {
            return api("GET", base(owner, need(name, "name")) + "/versions", null);
        }

        /** Delete the pipeline and its versions; finished runs stay. 409 while a run is queued or running. */
        public void delete(String owner, String name) throws IOException, InterruptedException {
            api("DELETE", base(owner, need(name, "name")), null);
        }

        /** Delete one version; 409 for the latest or one a queued or running run uses. */
        public void deleteVersion(String owner, String name, int version) throws IOException, InterruptedException {
            api("DELETE", base(owner, need(name, "name")) + "/versions/" + version, null);
        }

        /** A new webhook secret; the old URL stops at once. {@code {webhookUrl}}, shown only here. */
        public String rotateWebhook(String owner, String name) throws IOException, InterruptedException {
            return api("POST", base(owner, need(name, "name")) + "/webhook/rotate", null);
        }

        /**
         * Start a run by hand and answer it at once (202), {@code queued}. A version that cannot
         * run is an {@link ApiException} 422 whose message lists every reason, and nothing is
         * recorded.
         */
        public Run run(RunPipeline p) throws IOException, InterruptedException {
            String body = p.body();
            return new Run(api("POST", base(p.owner, need(p.pipeline, "pipeline")) + "/runs", body));
        }

        /** The pipeline's runs, newest first. */
        public List<Run> runs(ListRuns p) throws IOException, InterruptedException {
            String limit = p.limit == null ? null : p.limit.toString();
            String raw = api("GET", base(p.owner, need(p.pipeline, "pipeline")) + "/runs" + query("limit", limit), null);
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

        /** One run by its id ("1760400000000-0a1b2c"). */
        public Run getRun(GetRun p) throws IOException, InterruptedException {
            String run = need(p.run, "run");
            return new Run(api("GET", base(p.owner, need(p.pipeline, "pipeline")) + "/runs/" + seg(run), null));
        }
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
            if (text(d.get("id")) == null || text(d.get("owner")) == null || text(d.get("pipeline")) == null) {
                throw new ApiException(502, "the answer named no run id, owner and pipeline");
            }
            doc = d;
            json = raw;
        }

        public String id() { return text(doc.get("id")); }
        /** Per owner and pipeline, from 1: the name people say out loud. */
        public Long number() { return longOf(doc.get("number")); }
        public String pipeline() { return text(doc.get("pipeline")); }
        public String owner() { return text(doc.get("owner")); }
        /** The version the run was planned from; it cannot change. */
        public Long pipelineVersion() { return longOf(doc.get("pipelineVersion")); }
        public String definitionSha256() { return text(doc.get("definitionSha256")); }
        /** manual, schedule, webhook or pipeline. */
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
            return "/v1/pipelines/" + seg(owner()) + "/" + seg(pipeline()) + "/runs/" + seg(id()) + tail;
        }

        @Override
        public String toString() {
            return "Run " + owner() + "/" + pipeline() + "#" + number() + " " + state();
        }
    }

    // ── pipeline params ────────────────────────────────────────────────────

    /**
     * {@link Pipelines#run}: {@code owner} and {@code pipeline} required; {@code version} defaults to
     * the newest. {@code input} adds one input the pipeline declares. {@code environment} runs
     * every job that names none in that environment; {@code clone} instead in a per-run copy of
     * it, deleted when the run ends — one or the other.
     */
    public static final class RunPipeline {
        private String owner, pipeline, environment, clone;
        private Integer version;
        private Map<String, String> inputs;

        public RunPipeline owner(String v) { owner = v; return this; }
        public RunPipeline pipeline(String v) { pipeline = v; return this; }
        public RunPipeline version(int v) { version = v; return this; }
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
                .raw("version", version == null ? null : version.toString())
                .raw("inputs", inputs == null ? null : stringMap(inputs))
                .raw("environment", clone != null ? obj("clone", clone) : environment == null ? null : quote(environment))
                .toString();
        }
    }

    /** {@link Pipelines#runs}: {@code owner} and {@code pipeline} required; {@code limit} is optional. */
    public static final class ListRuns {
        private String owner, pipeline;
        private Integer limit;

        public ListRuns owner(String v) { owner = v; return this; }
        public ListRuns pipeline(String v) { pipeline = v; return this; }
        public ListRuns limit(int v) { limit = v; return this; }
    }

    /** {@link Pipelines#getRun}: all three required. */
    public static final class GetRun {
        private String owner, pipeline, run;

        public GetRun owner(String v) { owner = v; return this; }
        public GetRun pipeline(String v) { pipeline = v; return this; }
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

        /** The JSON text of a value parsed by this reader (maps, lists, strings, numbers, booleans, null). */
        static String write(Object v) {
            if (v == null) return "null";
            if (v instanceof String) return quote((String) v);
            if (v instanceof Map) {
                List<String> parts = new ArrayList<>();
                for (Map.Entry<?, ?> e : ((Map<?, ?>) v).entrySet()) parts.add(quote((String) e.getKey()) + ":" + write(e.getValue()));
                return "{" + String.join(",", parts) + "}";
            }
            if (v instanceof List) {
                List<String> parts = new ArrayList<>();
                for (Object x : (List<?>) v) parts.add(write(x));
                return "[" + String.join(",", parts) + "]";
            }
            return v.toString();
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
                if (e == 'n') b.append('\n');
                else if (e == 't') b.append('\t');
                else if (e == 'r') b.append('\r');
                else if (e == 'b') b.append('\b');
                else if (e == 'f') b.append('\f');
                else if (e == 'u') {
                    if (i + 4 > s.length()) throw bad();
                    b.append((char) Integer.parseInt(s.substring(i, i + 4), 16));
                    i += 4;
                } else b.append(e); // \" \\ \/
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
            if (c == '"') b.append("\\\"");
            else if (c == '\\') b.append("\\\\");
            else if (c == '\n') b.append("\\n");
            else if (c == '\r') b.append("\\r");
            else if (c == '\t') b.append("\\t");
            else if (c < 0x20) b.append(String.format("\\u%04x", (int) c));
            else b.append(c);
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
