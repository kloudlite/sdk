// Kloudlite SDK for Java — one file, no dependencies (java.net.http, Java 11+).
//
// Auth is a Bearer token (a session or CLI token). Every module hangs off the
// one client, and every repository is named by ONE "owner/name" string — the
// owner half being a person's handle or a team slug:
//
//   Kloudlite kl = new Kloudlite("https://api.example.com", "…");
//   String repo = kl.repos().create("acme/payments", null, null);
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
    private final Repos repos = new Repos();
    private final HubRepos models = new HubRepos("model", "models");
    private final HubRepos datasets = new HubRepos("dataset", "datasets");

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
    public Repos repos() {
        return repos;
    }

    /** Model repositories, on the hub listener. */
    public HubRepos models() {
        return models;
    }

    /** Dataset repositories, on the hub listener. */
    public HubRepos datasets() {
        return datasets;
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
    public class Repos {
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
         * A new repo whose contents are fetched from a PUBLIC GitHub repository — {@code github}
         * is its "owner/name" pair, never a URL. Answers 202: the repo exists but the fetch runs
         * in the background; poll {@link #importStatus}.
         */
        public String importGithub(String repo, String github, String visibility, String description)
                throws IOException, InterruptedException {
            String[] on = split(repo);
            return request("POST", "/v1/repos/import", obj(
                "owner", on[0], "name", on[1], "github", github,
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
     * "owner/name" string, exactly as in {@link Repos}.
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
         * A new repo whose contents — large files included — are fetched from a PUBLIC Hugging
         * Face repo of the same kind; {@code hf} is its "owner/name" pair, never a URL. Answers
         * 202; poll {@link #importStatus}.
         */
        public String importHf(String repo, String hf, boolean privateRepo)
                throws IOException, InterruptedException {
            return hubRequest("POST", path(repo, "/import"),
                "{\"from\":" + quote(hf) + ",\"private\":" + privateRepo + "}");
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
