// Kloudlite SDK for Java — one file, no dependencies (java.net.http, Java 11+).
//
// The client speaks to the browse API with HTTP Basic auth: the username names
// the owner the token belongs to (git's "x" placeholder also works), the token
// is the secret. No credentials is still a valid client — public repositories
// admit anonymous reads.
//
//   Kloudlite kl = new Kloudlite("https://git.example.com", "acme", "…");
//
// request() returns the raw JSON body as a String; bring your own JSON mapper.

import java.io.IOException;
import java.net.URI;
import java.net.http.HttpClient;
import java.net.http.HttpRequest;
import java.net.http.HttpResponse;
import java.nio.charset.StandardCharsets;
import java.time.Duration;
import java.util.Base64;

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
    private final String auth; // null = anonymous
    private final HttpClient http;

    /** token may be null for anonymous access; username null defaults to git's "x" placeholder. */
    public Kloudlite(String baseUrl, String username, String token) {
        this.baseUrl = baseUrl.replaceAll("/+$", "");
        this.auth = token == null ? null
            : "Basic " + Base64.getEncoder().encodeToString(
                ((username == null ? "x" : username) + ":" + token).getBytes(StandardCharsets.UTF_8));
        this.http = HttpClient.newBuilder().connectTimeout(Duration.ofSeconds(30)).build();
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
        if (auth != null) b.header("Authorization", auth);
        HttpResponse<String> resp = http.send(b.build(), HttpResponse.BodyHandlers.ofString());
        if (resp.statusCode() < 200 || resp.statusCode() > 299) {
            throw new ApiException(resp.statusCode(), errorText(resp.body()));
        }
        return resp.body();
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
