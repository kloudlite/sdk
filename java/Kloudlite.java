// Kloudlite SDK for Java — one file, no dependencies (java.net.http, Java 11+).
//
// Auth is a Bearer token (a session or CLI token). Every module hangs off the
// one client:
//
//   Kloudlite kl = new Kloudlite("https://api.example.com", "…");
//   String repo = kl.repos().create("acme", "payments", "private", "billing");
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
    private final String token; // null = anonymous
    private final HttpClient http;
    private final Repos repos = new Repos();

    /** token may be null for anonymous access to public data. */
    public Kloudlite(String baseUrl, String token) {
        this.baseUrl = baseUrl.replaceAll("/+$", "");
        this.token = token;
        this.http = HttpClient.newBuilder().connectTimeout(Duration.ofSeconds(30)).build();
    }

    /** Code repositories: {@code /v1/repos}. */
    public Repos repos() {
        return repos;
    }

    /**
     * One HTTP round trip. {@code jsonBody} may be null; returns the answer's raw
     * JSON body, and throws {@link ApiException} on any non-2xx status.
     */
    public String request(String method, String path, String jsonBody) throws IOException, InterruptedException {
        HttpRequest.Builder b = HttpRequest.newBuilder()
            .uri(URI.create(baseUrl + path))
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

    public class Repos {
        // repos
        public String create(String owner, String name, String visibility, String description)
                throws IOException, InterruptedException {
            return request("POST", "/v1/repos", obj(
                "owner", owner, "name", name,
                "visibility", visibility == null ? "private" : visibility,
                "description", description == null ? "" : description));
        }

        public String list(String owner) throws IOException, InterruptedException {
            return request("GET", "/v1/repos?owner=" + seg(owner), null);
        }

        public String get(String owner, String name) throws IOException, InterruptedException {
            return request("GET", repo(owner, name, ""), null);
        }

        /** Either argument may be null to leave it unchanged. */
        public String update(String owner, String name, String visibility, String description)
                throws IOException, InterruptedException {
            return request("PATCH", repo(owner, name, ""), obj("visibility", visibility, "description", description));
        }

        public String delete(String owner, String name) throws IOException, InterruptedException {
            return request("DELETE", repo(owner, name, ""), null);
        }

        /**
         * A new repo copied from another: objects and refs, never its pull requests or settings.
         * A null visibility means private, whatever the source was.
         */
        public String fork(String srcOwner, String srcName, String owner, String name,
                           String visibility, String description) throws IOException, InterruptedException {
            return request("POST", repo(srcOwner, srcName, "/fork"), obj(
                "owner", owner, "name", name,
                "visibility", visibility == null ? "private" : visibility,
                "description", description == null ? "" : description));
        }

        // branches
        public String branches(String owner, String name) throws IOException, InterruptedException {
            return request("GET", repo(owner, name, "/branches"), null);
        }

        /** {@code oid} is the head the caller saw; the delete is refused if the branch moved. */
        public String deleteBranch(String owner, String name, String branch, String oid)
                throws IOException, InterruptedException {
            return request("DELETE", repo(owner, name, "/branches/" + seg(branch) + "?oid=" + seg(oid)), null);
        }

        // protection
        public String protection(String owner, String name) throws IOException, InterruptedException {
            return request("GET", repo(owner, name, "/protection"), null);
        }

        public String protect(String owner, String name, String pattern, boolean remove)
                throws IOException, InterruptedException {
            return request("POST", repo(owner, name, "/protection"),
                "{\"pattern\":" + quote(pattern) + ",\"remove\":" + remove + "}");
        }

        // pull requests
        public String pulls(String owner, String name) throws IOException, InterruptedException {
            return request("GET", repo(owner, name, "/pulls"), null);
        }

        public String pull(String owner, String name, long number) throws IOException, InterruptedException {
            return request("GET", repo(owner, name, "/pulls/" + number), null);
        }

        public String openPull(String owner, String name, String title, String head, String base, String description)
                throws IOException, InterruptedException {
            return openPull(owner, name, title, head, base, description, null);
        }

        /** {@code headRepo} ("owner/name") makes it a cross-repo pull: {@code head} then names that fork's branch. */
        public String openPull(String owner, String name, String title, String head, String base,
                               String description, String headRepo) throws IOException, InterruptedException {
            return request("POST", repo(owner, name, "/pulls"), obj(
                "title", title, "head", head, "base", base,
                "description", description == null ? "" : description,
                "head_repo", headRepo));
        }

        public String comment(String owner, String name, long number, String body)
                throws IOException, InterruptedException {
            return request("POST", repo(owner, name, "/pulls/" + number + "/comments"), obj("body", body));
        }

        /** The merge is a job (202); strategy is fast-forward, squash, merge or rebase. */
        public String merge(String owner, String name, long number, String strategy)
                throws IOException, InterruptedException {
            return request("POST", repo(owner, name, "/pulls/" + number + "/merge?strategy=" + seg(strategy)), null);
        }

        public String close(String owner, String name, long number) throws IOException, InterruptedException {
            return request("POST", repo(owner, name, "/pulls/" + number + "/close"), null);
        }

        // commits
        public String compare(String owner, String name, String base, String head)
                throws IOException, InterruptedException {
            return request("GET", repo(owner, name, "/compare?base=" + seg(base) + "&head=" + seg(head)), null);
        }

        /** {@code commitJson} is the whole body: branch, message, changes, expect?, newBranch?. */
        public String commit(String owner, String name, String commitJson)
                throws IOException, InterruptedException {
            return request("POST", repo(owner, name, "/commits"), commitJson);
        }

        public String signature(String owner, String name, String sha) throws IOException, InterruptedException {
            return request("GET", repo(owner, name, "/commits/" + seg(sha) + "/signature"), null);
        }

        private String repo(String owner, String name, String tail) {
            return "/v1/repos/" + seg(owner) + "/" + seg(name) + tail;
        }
    }

    // ── small helpers, so a one-file SDK needs no JSON dependency ──────────

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
