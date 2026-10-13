// The fork workflow, end to end: fork a repo, commit onto a new branch of the
// fork, propose it back upstream as a cross-repo pull request, and merge it.
//
//   javac Kloudlite.java Example.java
//   KLOUDLITE_URL=https://api.example.com KLOUDLITE_TOKEN=… java Example
//
// The client returns raw JSON strings; parse with the mapper of your choice —
// here a crude scan keeps the example dependency-free.

import java.util.Base64;

public class Example {
    public static void main(String[] args) throws Exception {
        Kloudlite kl = new Kloudlite(System.getenv("KLOUDLITE_URL"), System.getenv("KLOUDLITE_TOKEN"));

        String upOwner = "acme", upName = "payments"; // the repo being contributed to
        String me = "dev";                            // the handle the fork lands under

        try {
            // 1. Fork: objects and refs, never the upstream's pull requests or settings.
            String fork = kl.repos().fork(upOwner, upName, me, "payments", null, null);
            System.out.println("forked: " + fork);

            // 2. Commit a change onto a NEW branch of the fork (the fork's main stays put).
            String content = Base64.getEncoder()
                .encodeToString("# Payments\n\nFixed rounding.\n".getBytes());
            String committed = kl.repos().commit(me, "payments", """
                {"branch":"main","newBranch":"fix-rounding",
                 "message":"Fix rounding in invoice totals",
                 "changes":[{"path":"README.md","contentBase64":"%s"}]}""".formatted(content));
            System.out.println("committed: " + committed);

            // 3. Propose it back: head names the FORK's branch, headRepo says where it lives.
            String pr = kl.repos().openPull(upOwner, upName,
                "Fix rounding in invoice totals", "fix-rounding", "main", "", me + "/payments");
            long number = numberOf(pr);
            System.out.println("opened #" + number);

            // 4. The diff upstream reviews is the mirror: pull/{n}/head.
            kl.repos().compare(upOwner, upName, "main", "pull/" + number + "/head");
            kl.repos().comment(upOwner, upName, number, "Ready for review");

            // 5. Merge. 202 — a job; the pull reports where it got to.
            kl.repos().merge(upOwner, upName, number, "squash");
            for (int i = 0; i < 30; i++) {
                String got = kl.repos().pull(upOwner, upName, number);
                if (!got.contains("\"state\":\"open\"")) {
                    System.out.println("pull is no longer open");
                    break;
                }
                Thread.sleep(2000);
            }
        } catch (Kloudlite.ApiException e) {
            System.err.println("failed with " + e.status + ": " + e.message);
        }
    }

    /** The `"number":N` field of a pull's JSON — enough parsing for an example. */
    static long numberOf(String json) {
        int k = json.indexOf("\"number\":");
        int start = k + "\"number\":".length();
        int end = start;
        while (end < json.length() && Character.isDigit(json.charAt(end))) end++;
        return Long.parseLong(json.substring(start, end));
    }
}
