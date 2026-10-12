# Hub Provider Wrapper Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A workspace reaches Hugging Face by default and the kloudlite hub only when the caller names `provider="kloudlite"`, through `kl hf` and a small Python package, and agents learn this from the `kloudlite` skill.

**Architecture:** The hub accepts the workspace's existing registry JWT when a PAT lookup misses. The agent puts `KL_HUB_URL` in the workspace env. `kl hf` and `kloudlite.hub` turn a provider name into an endpoint, a token and a separate cache, and pass nothing for `huggingface`.

**Tech Stack:** Rust (axum, clap), Python (`huggingface_hub`), Markdown skills.

**Spec:** `docs/superpowers/specs/2026-10-12-hub-provider-wrapper-design.md`

## Global Constraints

- Providers are exactly `huggingface` (default) and `kloudlite`. An unknown provider is an error naming both.
- `kloudlite`: endpoint `$KL_HUB_URL`; token `$KL_HUB_TOKEN`, else `/etc/kloudlite/ssh/registry-token`; cache `~/.cache/kloudlite/hub`.
- `kloudlite` with no `KL_HUB_URL` fails with an error containing `hub address is missing`.
- `huggingface` sets and removes nothing.
- Workspaces never preset `HF_ENDPOINT`.
- The token goes in env or a keyword argument only, never in argv, logs or errors.
- `hf` missing: `hf not found. Add the package with `kl pkg add python3Packages.huggingface-hub`.`
- The hub never trusts the JWT's `"*"` scope; authorization stays owner, `may_act`, or public repo.
- Ponytail comment in `crates/hub/src/auth.rs`, verbatim: `// ponytail: one owner credential for registry and hub; split audiences if a hub-only token is ever needed.`
- The bench gets no `KL_HUB_URL`.
- Python package: `sdk/python/`, name `kloudlite`, version `0.1.0`, depends only on `huggingface_hub`. Nothing is published to PyPI.
- Cargo on the laptop with `CARGO_TARGET_DIR=/Volumes/kdisk/target-master`. Commits: imperative sentence case, no tool attribution, no trailers, specific paths only.
- CI gate: `cargo clippy --workspace --all-targets -- -D warnings`.

## Review Focus

1. **A person's own Hugging Face token must never reach our hub.** If `HF_TOKEN` is set, or `hf auth login` stored a token, and `kloudlite` has no token of its own, the child must send no token. `kl hf` removes `HF_TOKEN` and sets `HF_HUB_DISABLE_IMPLICIT_TOKEN=1`. Python passes `token=False`. Tests are in Tasks 3 and 4.
2. **A token file with a trailing newline**, which is how Secrets project, must be trimmed. An empty file is anonymous. Tests are in Tasks 3 and 4.
3. **`KL_HUB_URL` with a trailing slash** (`https://hub.khost.dev/`) is trimmed, so `huggingface_hub` never builds `//api/...`. Tests are in Tasks 3 and 4.
4. **`--provider` after the `hf` arguments** passes through to `hf` untouched. It does not select the provider. A test is in Task 3.
5. **An anonymous-scope registry JWT presented to the hub** (`sub == ""`) is a 401. It is never treated as anonymous, unlike `/v2`. A test is in Task 1.

---

### Task 1: The hub accepts the workspace registry JWT

**Files:**
- Modify: `crates/hub/src/auth.rs:36-41`
- Test: `tests/hub_api.rs` (the `Hub` struct and `hub()` helper at lines 12-26; new tests after `whoami_names_the_caller`)

**Interfaces:**
- Consumes: `app.jwt: Arc<kloudlite_core::jwt::Jwt>`. `Jwt::verify_registry(&self, &str) -> Option<String>` returns `Some("")` for an anonymous-scope token and `None` for a foreign, expired or malformed one. `Jwt::mint_registry(&self, owner, scope, ttl_secs) -> Result<String>`. `Jwt::new(&str) -> Result<Jwt>` needs a secret of at least 32 bytes.
- Produces: `caller()` returns `Ok(Some(owner))` for a valid registry JWT (Bearer, or Basic whose user is the owner or `x`).

- [ ] **Step 1: Extend the test helper and write the failing tests**

In `tests/hub_api.rs`, add two fields to `Hub` and mint the tokens before `serve_hub` takes `app`:

```rust
struct Hub {
    base: String,
    e: TestEnv,
    alice: String,
    bob: String,
    /// The workspace credential: a registry JWT for alice, as the keys beat mints it.
    alice_jwt: String,
    /// What `/v2/token` hands an anonymous caller: ours, but names nobody.
    anon_jwt: String,
}

async fn hub() -> Hub {
    let e = env().await;
    let app = app(e.store.clone()).await;
    let alice = e.store.create_token("alice").await.unwrap();
    let bob = e.store.create_token("bob").await.unwrap();
    let alice_jwt = app.jwt.mint_registry("alice", "*", 3600).unwrap();
    let anon_jwt = app.jwt.mint_registry("", "*", 3600).unwrap();
    let port = serve_hub(app).await;
    Hub { base: format!("http://127.0.0.1:{port}"), e, alice, bob, alice_jwt, anon_jwt }
}
```

Add these tests after `whoami_names_the_caller`:

```rust
async fn whoami(h: &Hub, auth: impl FnOnce(reqwest::RequestBuilder) -> reqwest::RequestBuilder) -> reqwest::Response {
    auth(client().get(format!("{}/api/whoami-v2", h.base))).send().await.unwrap()
}

#[tokio::test]
async fn registry_jwt_is_the_workspace_credential() {
    let h = hub().await;
    for r in [
        whoami(&h, |b| b.bearer_auth(&h.alice_jwt)).await,
        whoami(&h, |b| b.basic_auth("alice", Some(&h.alice_jwt))).await,
        whoami(&h, |b| b.basic_auth("x", Some(&h.alice_jwt))).await,
    ] {
        assert_eq!(r.status(), 200);
        let v: Value = r.json().await.unwrap();
        assert_eq!(v["name"], "alice");
    }
}

#[tokio::test]
async fn a_registry_jwt_that_does_not_verify_is_refused() {
    let h = hub().await;
    let foreign = kloudlite_core::jwt::Jwt::new("another-secret-that-is-at-least-32-bytes-long")
        .unwrap()
        .mint_registry("alice", "*", 3600)
        .unwrap();
    assert_eq!(whoami(&h, |b| b.bearer_auth(&foreign)).await.status(), 401);
    // Ours, but anonymous-scope: a presented credential that names nobody is a refusal here.
    assert_eq!(whoami(&h, |b| b.bearer_auth(&h.anon_jwt)).await.status(), 401);
    // The Basic user must be the owner the token names.
    assert_eq!(whoami(&h, |b| b.basic_auth("bob", Some(&h.alice_jwt))).await.status(), 401);
    // No header at all is still anonymous, and whoami answers anonymous with 401 as before.
    assert_eq!(whoami(&h, |b| b).await.status(), 401);
}

#[tokio::test]
async fn registry_jwt_reads_a_private_repo_of_its_owner_only() {
    let h = hub().await;
    seed_hub(&h, HubKind::Model, "alice", "secret", false, &[("config.json", b"{}")]).await;
    let mine = client().get(format!("{}/api/models/alice/secret", h.base)).bearer_auth(&h.alice_jwt).send().await.unwrap();
    assert_eq!(mine.status(), 200);
    let bob_pat = client().get(format!("{}/api/models/alice/secret", h.base)).bearer_auth(&h.bob).send().await.unwrap();
    // A signed-in stranger gets 404, as with a PAT.
    assert_eq!(bob_pat.status(), 404);
}
```

If `seed_hub` or `HubKind` are not yet in scope at that point in the file, move the test below `seed_hub` (line ~128). Do not duplicate the helper.

- [ ] **Step 2: Run the tests and confirm they fail**

Run: `CARGO_TARGET_DIR=/Volumes/kdisk/target-master cargo test --test hub_api registry_jwt 2>&1 | tail -20`
Expected: `registry_jwt_is_the_workspace_credential` and `registry_jwt_reads_a_private_repo_of_its_owner_only` FAIL with status 401. `a_registry_jwt_that_does_not_verify_is_refused` passes already, which is correct.

- [ ] **Step 3: Implement the fallthrough**

Replace the `match` at `crates/hub/src/auth.rs:36-41` with:

```rust
    match app.store.owner_for_token(&token).await {
        // git-lfs replays the git remote's credentials, so git's `x` placeholder is a valid user.
        Ok(Some(o)) if basic_user_names(headers, &o, true) => Ok(Some(o)),
        Ok(Some(_)) => Err(bad_credential()),
        // A workspace holds no PAT, only the registry JWT the keys beat mints for its owner. Under
        // the same Basic rule as a PAT; an anonymous-scope token names nobody, so it is refused
        // like any other credential that was presented and did not verify.
        // ponytail: one owner credential for registry and hub; split audiences if a hub-only token is ever needed.
        Ok(None) => match app.jwt.verify_registry(&token) {
            Some(o) if !o.is_empty() && basic_user_names(headers, &o, true) => Ok(Some(o)),
            _ => Err(bad_credential()),
        },
        Err(e) => Err(internal(e)),
    }
```

Update the `//!` header line 3 to read: `//! Credentials: Bearer PAT or registry JWT (huggingface_hub, transformers) or Basic \`user:PAT\` / \`user:JWT\` (git, git-lfs). A`. Keep the rest of the header.

- [ ] **Step 4: Run the tests and confirm they pass**

Run: `CARGO_TARGET_DIR=/Volumes/kdisk/target-master cargo test --test hub_api 2>&1 | tail -5`
Expected: every test in `hub_api` passes.

- [ ] **Step 5: Clippy**

Run: `CARGO_TARGET_DIR=/Volumes/kdisk/target-master cargo clippy -p kloudlite-hub --all-targets -- -D warnings 2>&1 | tail -5` and `CARGO_TARGET_DIR=/Volumes/kdisk/target-master cargo clippy --test hub_api -- -D warnings 2>&1 | tail -5`
Expected: no warnings.

- [ ] **Step 6: Commit**

```bash
git add crates/hub/src/auth.rs tests/hub_api.rs
git commit -m "Accept the workspace registry token at the hub"
```

---

### Task 2: `KL_HUB_URL` in the workspace env

**Files:**
- Modify: `crates/workspaces/src/k8s/workspace.rs:17` (`login_env` signature), near line 82 (env push), line ~636 (call)
- Modify: `crates/workspaces/src/k8s/secrets.rs:145` (`sshd_config`), `:155` (its `login_env` call), `:188` (`ws_ssh_secret`)
- Modify: `crates/workspaces/src/k8s/mod.rs:~136-152` (`PodContext`)
- Modify: `bins/agent/src/controller/mod.rs:238` (`Ctx` field), `:377` (init)
- Modify call sites: `bins/agent/src/controller/workspace/mod.rs:~327`, `bins/agent/src/controller/environment/run.rs:~48`, `bins/agent/src/controller/workspace/status.rs:70`, `crates/workspaces/src/k8s/tests/mod.rs:32`, `crates/workspaces/src/k8s/tests/pod.rs` (lines 198, 422, 442, 451, 457, 467, 476, 482, 591, 594, 642, 694, 746, 801), `bins/agent/tests/reconcile/the_workspace_reconciler_and_its_volume.rs:~315-329`
- Modify: `deploy/k3s/agent-daemonset.yaml:175`
- Test: `crates/workspaces/src/k8s/tests/pod.rs` (`the_login_env_tells_kl_where_it_is`, line ~473)

**Interfaces:**
- Consumes: nothing from Task 1.
- Produces: `login_env(ws_id, name, owner, team, registry_host, api_url, git_ssh_host, git_ssh_port, hub_url: &str)`. `hub_url` is the LAST parameter of `login_env`, `sshd_config` and `ws_ssh_secret`. Add `PodContext.hub_url: &'a str` and `Ctx.hub_url: String`. The pod env var is `KL_HUB_URL`, unset when empty.

- [ ] **Step 1: Write the failing test**

In `crates/workspaces/src/k8s/tests/pod.rs`, change `the_login_env_tells_kl_where_it_is` so that both calls pass a hub URL as the new last argument, and add the assertions:

```rust
    let env = login_env("w-abc123", "ws-1", "acme", "", "registry.kloudlite.io", "https://api.kloudlite.io", "git.khost.dev", "22", "https://hub.khost.dev");
    // ...existing assertions unchanged...
    assert_eq!(get("KL_HUB_URL"), "https://hub.khost.dev");
    let team = login_env("w-abc123", "ws-1", "alice", "acme", "registry.kloudlite.io", "", "git.khost.dev", "22", "");
    // ...existing assertions unchanged...
    // Unset, not empty, in a region without a hub: `kl hf --provider kloudlite` then says the
    // address is missing instead of dialling an empty host.
    assert!(!team.iter().any(|e| e.name == "KL_HUB_URL"), "an empty hub url must leave the var unset");
    // The default provider is Hugging Face, untouched: the platform never presets its endpoint.
    assert!(!env.iter().any(|e| e.name == "HF_ENDPOINT"));
```

- [ ] **Step 2: Confirm it fails**

Run: `CARGO_TARGET_DIR=/Volumes/kdisk/target-master cargo test -p kloudlite-workspaces the_login_env_tells_kl_where_it_is 2>&1 | tail -10`
Expected: compile error, because `login_env` takes 8 arguments and the test passes 9.

- [ ] **Step 3: Thread `hub_url` through**

In `workspace.rs`, add `hub_url: &str` as the last parameter of `login_env`. Then add this directly after the `KL_API_URL` block:

```rust
    // The kloudlite hub, reached only when a caller names `provider="kloudlite"` (`kl hf`,
    // `kloudlite.hub`). Never `HF_ENDPOINT`: Hugging Face stays the default, exactly as on a
    // laptop. Unset without `WS_HUB_URL`, so the wrapper says the address is missing.
    if !hub_url.is_empty() {
        env.push(var("KL_HUB_URL", hub_url.to_string()));
    }
```

At the call site (~line 636), pass `ctx.hub_url` last.

In `secrets.rs`, add `hub_url: &str` as the last parameter of `sshd_config` and `ws_ssh_secret`. Forward it in the `login_env(...)` call at line 155 and in `ws_ssh_secret`'s call to `sshd_config`.

In `k8s/mod.rs` `PodContext`, after `git_ssh_port`, add:

```rust
    /// The kloudlite hub base URL (`WS_HUB_URL`), surfaced as `KL_HUB_URL`; empty leaves it unset.
    pub hub_url: &'a str,
```

In `bins/agent/src/controller/mod.rs`, add `hub_url: String` beside `git_ssh_host` in `Ctx` with a one-line doc comment that matches the neighbours. At line ~377, initialise it as `hub_url: std::env::var("WS_HUB_URL").unwrap_or_default(),`. Use no default URL: an unset value means "no hub in this region".

Update every call site listed under **Files**:
- `PodContext { ... }` literals: add `hub_url: &ctx.hub_url,` in the agent and `hub_url: "",` in tests unless a test asserts on it.
- `ws_ssh_secret(...)` / `sshd_config(...)` / `login_env(...)` calls: add `&ctx.hub_url` in the agent and `""` in tests as the last argument.

Do NOT touch `crates/workspaces/src/k8s/bench.rs`; the bench gets no hub.

In `deploy/k3s/agent-daemonset.yaml`, beside `WS_REGISTRY_HOST` (line ~175), add the env entry in the same shape as its neighbours:

```yaml
            # The kloudlite hub, surfaced in workspaces as KL_HUB_URL for `kl hf --provider kloudlite`
            # and `kloudlite.hub`. Never HF_ENDPOINT: Hugging Face stays the default provider.
            - name: WS_HUB_URL
              value: https://hub.khost.dev
```

Match the indentation of the surrounding entries exactly.

- [ ] **Step 4: Run the tests**

Run: `CARGO_TARGET_DIR=/Volumes/kdisk/target-master cargo test -p kloudlite-workspaces 2>&1 | tail -5` and `CARGO_TARGET_DIR=/Volumes/kdisk/target-master cargo test -p kloudlite-agent 2>&1 | tail -5` (the package name is in `bins/agent/Cargo.toml`)
Expected: all pass.

- [ ] **Step 5: Clippy**

Run: `CARGO_TARGET_DIR=/Volumes/kdisk/target-master cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tail -5`
Expected: no warnings. If `login_env` now trips `too_many_arguments`, add `#[allow(clippy::too_many_arguments)]` as `secrets.rs` already does.

- [ ] **Step 6: Commit**

```bash
git add crates/workspaces/src/k8s/workspace.rs crates/workspaces/src/k8s/secrets.rs crates/workspaces/src/k8s/mod.rs crates/workspaces/src/k8s/tests/mod.rs crates/workspaces/src/k8s/tests/pod.rs bins/agent/src/controller/mod.rs bins/agent/src/controller/workspace/mod.rs bins/agent/src/controller/environment/run.rs bins/agent/src/controller/workspace/status.rs bins/agent/tests/reconcile/the_workspace_reconciler_and_its_volume.rs deploy/k3s/agent-daemonset.yaml
git commit -m "Give workspaces the hub address as KL_HUB_URL"
```

---

### Task 3: `kl hf`

**Files:**
- Create: `bins/kl/src/hf.rs`
- Modify: `bins/kl/src/main.rs` (`mod hf;` at line ~14, `Cmd::Hf` variant, dispatch in `real_main`, parse tests in `mod tests` at line ~263, `//!` header)

**Interfaces:**
- Consumes: `images::TOKEN_PATH` (`"/etc/kloudlite/ssh/registry-token"`, `bins/kl/src/images.rs:20`) and `KL_HUB_URL` from Task 2.
- Produces: `hf::child_env(provider: &str, hub_url: Option<String>, token: Option<String>, home: &str) -> Result<ChildEnv, String>` and `hf::run(provider: &str, args: &[String]) -> Result<(), String>`.

- [ ] **Step 1: Write `hf.rs` with its failing tests first**

Create `bins/kl/src/hf.rs` with the tests and with stubbed bodies `todo!()` in `child_env`, then run them (Step 2). Here is the final file:

```rust
//! `kl hf`: Hugging Face's `hf` CLI against either hub.
//!
//! Hugging Face is the default and gets nothing from us: a person's own token, mirror and cache
//! keep working, exactly as on a laptop. `--provider kloudlite` points the CHILD at our hub
//! (`KL_HUB_URL`) with the workspace's registry token and a cache of its own, so every `hf` verb
//! works against either hub with no code per verb.
//!
//! The separate cache is load-bearing: Hugging Face keys a snapshot by repo id alone, so `acme/x`
//! from our hub and `acme/x` from huggingface.co would otherwise share one entry. And a person's
//! Hugging Face token must never reach our hub: without a kloudlite token the child is told to
//! send none at all (`HF_HUB_DISABLE_IMPLICIT_TOKEN`), which also covers a token `hf auth login`
//! stored on disk. Spec: `docs/superpowers/specs/2026-10-12-hub-provider-wrapper-design.md`.

use std::io::ErrorKind;
use std::process::Command;

const NO_HF: &str = "hf not found. Add the package with `kl pkg add python3Packages.huggingface-hub`.";
const NO_HUB: &str = "hub address is missing: KL_HUB_URL is not set (a workspace sets it; elsewhere, set it to the hub's URL)";

/// What the `hf` child gets on top of this process's env. Never the token in argv or an error.
#[derive(Debug, PartialEq)]
pub struct ChildEnv {
    pub set: Vec<(&'static str, String)>,
    pub remove: Vec<&'static str>,
}

pub fn child_env(provider: &str, hub_url: Option<String>, token: Option<String>, home: &str) -> Result<ChildEnv, String> {
    match provider {
        "huggingface" => Ok(ChildEnv { set: vec![], remove: vec![] }),
        "kloudlite" => {
            let url = hub_url.map(|u| u.trim().trim_end_matches('/').to_string()).filter(|u| !u.is_empty()).ok_or(NO_HUB)?;
            let mut set = vec![("HF_ENDPOINT", url), ("HF_HUB_CACHE", format!("{home}/.cache/kloudlite/hub"))];
            let mut remove = vec![];
            match token.map(|t| t.trim().to_string()).filter(|t| !t.is_empty()) {
                Some(t) => set.push(("HF_TOKEN", t)),
                // Anonymous still reads public repos; what it must not do is send the person's
                // Hugging Face token to our hub.
                None => {
                    remove.push("HF_TOKEN");
                    set.push(("HF_HUB_DISABLE_IMPLICIT_TOKEN", "1".into()));
                }
            }
            Ok(ChildEnv { set, remove })
        }
        other => Err(format!("unknown provider `{other}`: use huggingface or kloudlite")),
    }
}

/// Runs `hf` and exits with its code.
pub fn run(provider: &str, args: &[String]) -> Result<(), String> {
    let home = std::env::var("HOME").map_err(|_| "HOME is not set".to_string())?;
    let token = std::env::var("KL_HUB_TOKEN")
        .ok()
        .filter(|t| !t.trim().is_empty())
        .or_else(|| std::fs::read_to_string(crate::images::TOKEN_PATH).ok());
    let env = child_env(provider, std::env::var("KL_HUB_URL").ok(), token, &home)?;
    let mut cmd = Command::new("hf");
    cmd.args(args);
    for k in &env.remove {
        cmd.env_remove(k);
    }
    cmd.envs(env.set.iter().map(|(k, v)| (*k, v.as_str())));
    let status = cmd.status().map_err(|e| if e.kind() == ErrorKind::NotFound { NO_HF.to_string() } else { format!("hf: {e}") })?;
    std::process::exit(status.code().unwrap_or(1));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn get<'a>(e: &'a ChildEnv, k: &str) -> Option<&'a str> {
        e.set.iter().find(|(n, _)| *n == k).map(|(_, v)| v.as_str())
    }

    #[test]
    fn huggingface_adds_nothing() {
        let e = child_env("huggingface", Some("https://hub.khost.dev".into()), Some("tok".into()), "/home/kl").unwrap();
        assert_eq!(e, ChildEnv { set: vec![], remove: vec![] });
    }

    #[test]
    fn kloudlite_sets_endpoint_token_and_its_own_cache() {
        let e = child_env("kloudlite", Some("https://hub.khost.dev/".into()), Some("tok\n".into()), "/home/kl").unwrap();
        assert_eq!(get(&e, "HF_ENDPOINT"), Some("https://hub.khost.dev"), "trailing slash trimmed");
        assert_eq!(get(&e, "HF_TOKEN"), Some("tok"), "token file newline trimmed");
        assert_eq!(get(&e, "HF_HUB_CACHE"), Some("/home/kl/.cache/kloudlite/hub"));
        assert!(e.remove.is_empty());
        assert_eq!(get(&e, "HF_HUB_DISABLE_IMPLICIT_TOKEN"), None);
    }

    #[test]
    fn kloudlite_without_a_token_sends_none_of_the_persons() {
        for token in [None, Some("".to_string()), Some("\n".to_string())] {
            let e = child_env("kloudlite", Some("https://hub.khost.dev".into()), token, "/home/kl").unwrap();
            assert_eq!(get(&e, "HF_TOKEN"), None);
            assert_eq!(e.remove, vec!["HF_TOKEN"]);
            assert_eq!(get(&e, "HF_HUB_DISABLE_IMPLICIT_TOKEN"), Some("1"));
        }
    }

    #[test]
    fn kloudlite_without_an_address_says_so() {
        for url in [None, Some("".to_string()), Some("  ".to_string())] {
            let err = child_env("kloudlite", url, Some("tok".into()), "/home/kl").unwrap_err();
            assert!(err.contains("hub address is missing"), "{err}");
            assert!(!err.contains("tok"));
        }
    }

    #[test]
    fn unknown_provider_names_both() {
        let err = child_env("hugginface", None, None, "/home/kl").unwrap_err();
        assert!(err.contains("huggingface") && err.contains("kloudlite"), "{err}");
    }
}
```

- [ ] **Step 2: Run the tests with `todo!()` bodies and confirm they fail**

Add `mod hf;` to `main.rs` first so the module compiles.
Run: `CARGO_TARGET_DIR=/Volumes/kdisk/target-master cargo test -p kl hf:: 2>&1 | tail -10`
Expected: 5 tests FAIL (panicked at `not yet implemented`).

- [ ] **Step 3: Fill in the bodies as above, and wire up the CLI**

In `main.rs`, add after `Env` in `enum Cmd`:

```rust
    /// Hugging Face's `hf` CLI, against Hugging Face (default) or the kloudlite hub:
    /// `kl hf --provider kloudlite download acme/model`
    Hf {
        /// huggingface or kloudlite. Put it before the hf arguments; everything after goes to hf.
        #[arg(long, default_value = "huggingface")]
        provider: String,
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
```

In `real_main`, add `Cmd::Hf { provider, args } => hf::run(&provider, &args),`. Add one sentence to the `//!` header: `` `kl hf` runs Hugging Face's `hf` against Hugging Face or, with `--provider kloudlite`, our hub (`hf.rs`). ``

Add to `mod tests` in `main.rs`:

```rust
    #[test]
    fn hf_takes_provider_first_and_passes_the_rest_through() {
        let Cmd::Hf { provider, args } = Cli::try_parse_from(["kl", "hf", "--provider", "kloudlite", "download", "acme/m", "--repo-type", "dataset"]).unwrap().cmd else { panic!() };
        assert_eq!(provider, "kloudlite");
        assert_eq!(args, ["download", "acme/m", "--repo-type", "dataset"]);
        // After the hf arguments, `--provider` is hf's business, not ours.
        let Cmd::Hf { provider, args } = Cli::try_parse_from(["kl", "hf", "download", "x", "--provider", "kloudlite"]).unwrap().cmd else { panic!() };
        assert_eq!(provider, "huggingface");
        assert_eq!(args, ["download", "x", "--provider", "kloudlite"]);
    }
```

- [ ] **Step 4: Run the tests**

Run: `CARGO_TARGET_DIR=/Volumes/kdisk/target-master cargo test -p kl 2>&1 | tail -5`
Expected: all pass. With `trailing_var_arg`, clap stops parsing options at the first positional, so a later `--provider` lands in `args`; this is spec section 4.

- [ ] **Step 5: Clippy, then commit**

Run: `CARGO_TARGET_DIR=/Volumes/kdisk/target-master cargo clippy -p kl --all-targets -- -D warnings 2>&1 | tail -5`
Expected: no warnings.

```bash
git add bins/kl/src/hf.rs bins/kl/src/main.rs
git commit -m "Add kl hf to reach Hugging Face or the kloudlite hub"
```

---

### Task 4: Python package `kloudlite.hub`

**Files:**
- Create: `sdk/python/pyproject.toml`, `sdk/python/kloudlite/__init__.py`, `sdk/python/kloudlite/hub.py`, `sdk/python/tests/test_hub.py`, `sdk/python/README.md`

**Interfaces:**
- Consumes: `KL_HUB_URL`, `KL_HUB_TOKEN`, `/etc/kloudlite/ssh/registry-token`.
- Produces: `kloudlite.hub.download(repo_id, provider="huggingface", **kw) -> str`, `download_file(repo_id, filename, provider="huggingface", **kw) -> str`, `api(provider="huggingface") -> HfApi`.

- [ ] **Step 1: Write the failing test**

`sdk/python/tests/test_hub.py`:

```python
import os
import tempfile
import unittest
from unittest import mock

from kloudlite import hub

HUB = {"KL_HUB_URL": "https://hub.khost.dev/", "KL_HUB_TOKEN": "tok"}


class Provider(unittest.TestCase):
    def call(self, fn, *a, env=None, **kw):
        with mock.patch.dict(os.environ, env or {}, clear=False), \
             mock.patch.object(hub, "snapshot_download", return_value="/p") as sd, \
             mock.patch.object(hub, "hf_hub_download", return_value="/f") as hd:
            fn(*a, **kw)
            return (sd if sd.called else hd).call_args

    def test_huggingface_passes_nothing(self):
        args = self.call(hub.download, "openai-community/gpt2", env=HUB)
        self.assertEqual(args.args, ("openai-community/gpt2",))
        self.assertEqual(args.kwargs, {})

    def test_kloudlite_sets_endpoint_token_and_cache(self):
        args = self.call(hub.download, "acme/m", provider="kloudlite", env=HUB)
        self.assertEqual(args.kwargs["endpoint"], "https://hub.khost.dev")
        self.assertEqual(args.kwargs["token"], "tok")
        self.assertEqual(args.kwargs["cache_dir"], os.path.expanduser("~/.cache/kloudlite/hub"))

    def test_download_file_passes_filename(self):
        args = self.call(hub.download_file, "acme/m", "config.json", provider="kloudlite", env=HUB)
        self.assertEqual(args.args, ("acme/m", "config.json"))

    def test_caller_overrides_win(self):
        args = self.call(hub.download, "acme/m", provider="kloudlite", env=HUB, cache_dir="/c", repo_type="dataset")
        self.assertEqual(args.kwargs["cache_dir"], "/c")
        self.assertEqual(args.kwargs["repo_type"], "dataset")

    def test_token_file_is_trimmed_and_empty_is_anonymous(self):
        with tempfile.NamedTemporaryFile("w", delete=False) as f:
            f.write("filetok\n")
        env = {"KL_HUB_URL": "https://hub.khost.dev", "KL_HUB_TOKEN": ""}
        with mock.patch.object(hub, "TOKEN_FILE", f.name):
            self.assertEqual(self.call(hub.download, "a/m", provider="kloudlite", env=env).kwargs["token"], "filetok")
            open(f.name, "w").close()
            # False, never None: None would let huggingface_hub send the person's own HF token.
            self.assertIs(self.call(hub.download, "a/m", provider="kloudlite", env=env).kwargs["token"], False)
        with mock.patch.object(hub, "TOKEN_FILE", "/nonexistent/registry-token"):
            self.assertIs(self.call(hub.download, "a/m", provider="kloudlite", env=env).kwargs["token"], False)
        os.unlink(f.name)

    def test_missing_address_and_unknown_provider(self):
        with mock.patch.dict(os.environ, {"KL_HUB_URL": ""}):
            with self.assertRaisesRegex(RuntimeError, "hub address is missing"):
                hub.download("a/m", provider="kloudlite")
        with self.assertRaisesRegex(ValueError, "huggingface.*kloudlite"):
            hub.download("a/m", provider="hugginface")

    def test_api_binds_endpoint_and_token(self):
        with mock.patch.dict(os.environ, HUB), mock.patch.object(hub, "HfApi") as api:
            hub.api("kloudlite")
            api.assert_called_once_with(endpoint="https://hub.khost.dev", token="tok")
            hub.api()
            api.assert_called_with()


if __name__ == "__main__":
    unittest.main()
```

- [ ] **Step 2: Confirm it fails**

Run: `cd sdk/python && python3 -m venv /Volumes/kdisk/venv-sdk && /Volumes/kdisk/venv-sdk/bin/pip -q install huggingface_hub && /Volumes/kdisk/venv-sdk/bin/python -m unittest discover -s tests 2>&1 | tail -5`
Expected: `ModuleNotFoundError: No module named 'kloudlite'`.

- [ ] **Step 3: Write the package**

`sdk/python/kloudlite/__init__.py`:

```python
"""Kloudlite client helpers. `kloudlite.hub` reaches Hugging Face or the kloudlite hub."""
```

`sdk/python/kloudlite/hub.py`:

```python
"""Models and datasets from Hugging Face (the default) or the kloudlite hub.

    from kloudlite import hub
    path = hub.download("openai-community/gpt2")                  # Hugging Face
    path = hub.download("acme/my-model", provider="kloudlite")    # the kloudlite hub
    AutoModel.from_pretrained(path)

`huggingface` passes nothing, so a person's own Hugging Face token, mirror and cache keep working.
`kloudlite` passes the hub's endpoint, the workspace's token and a cache of its own: Hugging Face
keys a cache entry by repo id alone, so the same name from both hubs must not share one. Without a
kloudlite token the call sends none (`token=False`), never the person's Hugging Face token.
"""

import os

from huggingface_hub import HfApi, hf_hub_download, snapshot_download

PROVIDERS = ("huggingface", "kloudlite")
TOKEN_FILE = "/etc/kloudlite/ssh/registry-token"


def _token():
    t = os.environ.get("KL_HUB_TOKEN", "").strip()
    if t:
        return t
    try:
        with open(TOKEN_FILE) as f:
            return f.read().strip() or False
    except OSError:
        return False


def _options(provider):
    if provider == "huggingface":
        return {}
    if provider != "kloudlite":
        raise ValueError(f"unknown provider {provider!r}: use 'huggingface' or 'kloudlite'")
    url = os.environ.get("KL_HUB_URL", "").strip().rstrip("/")
    if not url:
        raise RuntimeError("hub address is missing: KL_HUB_URL is not set (a workspace sets it)")
    return {"endpoint": url, "token": _token(), "cache_dir": os.path.expanduser("~/.cache/kloudlite/hub")}


def download(repo_id, provider="huggingface", **kw):
    """Download a whole repo; returns the local path for `from_pretrained` or `load_dataset`."""
    return snapshot_download(repo_id, **{**_options(provider), **kw})


def download_file(repo_id, filename, provider="huggingface", **kw):
    """Download one file; returns its local path."""
    return hf_hub_download(repo_id, filename, **{**_options(provider), **kw})


def api(provider="huggingface"):
    """An `HfApi` bound to the provider, for uploads and everything else."""
    o = _options(provider)
    o.pop("cache_dir", None)
    return HfApi(**o)
```

`sdk/python/pyproject.toml`:

```toml
[build-system]
requires = ["setuptools>=68"]
build-backend = "setuptools.build_meta"

[project]
name = "kloudlite"
version = "0.1.0"
description = "Models and datasets from Hugging Face or the kloudlite hub"
requires-python = ">=3.9"
dependencies = ["huggingface_hub"]

[tool.setuptools]
packages = ["kloudlite"]
```

`sdk/python/README.md`, short:

````markdown
# kloudlite (Python)

```python
from kloudlite import hub
path = hub.download("acme/my-model", provider="kloudlite")
```

Hugging Face is the default provider. `kloudlite` reads `KL_HUB_URL` and the workspace token.
Until the package is on PyPI: `pip install "kloudlite @ git+https://github.com/kloudlite/rustic-git#subdirectory=sdk/python"`.
Tests: `python -m unittest discover -s tests` (needs `huggingface_hub`).
````

- [ ] **Step 4: Run the tests and build the wheel**

Run: `cd sdk/python && /Volumes/kdisk/venv-sdk/bin/python -m unittest discover -s tests 2>&1 | tail -3`
Expected: `OK` (7 tests).
Run: `/Volumes/kdisk/venv-sdk/bin/pip -q install build && /Volumes/kdisk/venv-sdk/bin/python -m build --wheel --outdir /Volumes/kdisk/sdk-dist sdk/python 2>&1 | tail -2`
Expected: `Successfully built kloudlite-0.1.0-py3-none-any.whl`. Then delete `/Volumes/kdisk/sdk-dist`, `sdk/python/build` and `sdk/python/*.egg-info`, so that nothing but source is left in the tree.

- [ ] **Step 5: Commit**

```bash
git add sdk/python/pyproject.toml sdk/python/README.md sdk/python/kloudlite/__init__.py sdk/python/kloudlite/hub.py sdk/python/tests/test_hub.py
git commit -m "Add the kloudlite Python package with hub downloads by provider"
```

---

### Task 5: Skills learn the providers

**Files:**
- Modify: `harness/packages/agent/claude-plugin/skills/kloudlite/SKILL.md` (insert before `## Team rules`, line ~156)
- Modify: `harness/laptop-plugin/plugin/skills/kloudlite/SKILL.md` (insert before `## Team rules`, line ~170)

**Interfaces:**
- Consumes: the `kl hf` spelling from Task 3 and `kloudlite.hub` from Task 4.
- Produces: nothing in code.

- [ ] **Step 1: Insert the same section into both files, after "Repos and images"**

```markdown
## Models and datasets

There are two model hubs: Hugging Face and the Kloudlite hub. Hugging Face is the default.

1. A model or dataset name alone (for example `gpt2` or `meta-llama/Llama-3.1-8B`) means Hugging Face. Use the usual tools without a provider.
2. Use the Kloudlite hub only when the person says "our hub", "Kloudlite hub" or "our model", or names a repo that the person or the team uploaded here.
3. Do not set `HF_ENDPOINT`. In a shell, use `kl hf --provider kloudlite <hf arguments>`. In Python, use `from kloudlite import hub` and `hub.download(<repo>, provider="kloudlite")`.
4. To load a model from the Kloudlite hub, download it first. Then give the local path to `from_pretrained` or `load_dataset`. A repo name in `from_pretrained` always goes to Hugging Face.
5. Upload trained models to the Kloudlite hub, under the name of the person or the team. Use `hub.api("kloudlite").upload_folder(...)` or `kl hf --provider kloudlite upload ...`. Do not upload to Hugging Face unless the person tells you to.
6. Do not put a Hugging Face token in a file, a commit or a message. The Kloudlite hub needs no token in a workspace.
7. If `hf` is not found, add it with `kl pkg add python3Packages.huggingface-hub`. If `kloudlite` is not installed in Python, install it with `pip install "kloudlite @ git+https://github.com/kloudlite/rustic-git#subdirectory=sdk/python"`.
```

The spec's seven rules are 1-7 in the spec. Spec rule 1 is the lead sentence here. Rule 7 above adds the install line so that an agent which meets a missing tool does not stall. Keep the text identical in both files.

- [ ] **Step 2: Check that both files carry identical sections**

Run: `diff <(sed -n '/^## Models and datasets/,/^## Team rules/p' harness/packages/agent/claude-plugin/skills/kloudlite/SKILL.md) <(sed -n '/^## Models and datasets/,/^## Team rules/p' harness/laptop-plugin/plugin/skills/kloudlite/SKILL.md) && echo same`
Expected: `same`.

If a skill test exists (`grep -rln "SKILL.md" harness --include='*.test.ts'`), run it with `bun test <file>`. Expected: it passes.

- [ ] **Step 3: Commit**

```bash
git add harness/packages/agent/claude-plugin/skills/kloudlite/SKILL.md harness/laptop-plugin/plugin/skills/kloudlite/SKILL.md
git commit -m "Teach the kloudlite skill which model hub to use"
```

---

### Task 6: End-to-end run with `HF_ENDPOINT` unset

**Files:**
- Modify: `tests/hub_e2e.sh` (new section before the `run ...` lines, ~line 103; `kl` binary and package install near the venv setup, ~line 24)

**Interfaces:**
- Consumes: the `kl` binary (Task 3), `sdk/python` (Task 4), and the server accepting a PAT as Bearer (already true). The e2e uses alice's PAT through `KL_HUB_TOKEN`, because a local server has no keys beat to mint a JWT; Task 1's tests cover the JWT.
- Produces: the `provider` section in the summary.

- [ ] **Step 1: Add the section**

Near the venv setup, after `PY=...`, install the local package into the venv: `"$ROOT/venv/bin/pip" -q install -e sdk/python >/dev/null`. Locate `kl`: `KL=${KL_BIN:-$(dirname "$BIN")/kl}`. `$BIN` is the server binary from the same target dir. The section skips with 77 if `$KL` is not executable.

Add the function before `failed=0`:

```bash
sec_provider() {
  [ -x "$KL" ] || return 77
  command -v hf >/dev/null || "$ROOT/venv/bin/hf" --help >/dev/null 2>&1 || return 77
  # As in a workspace: nothing presets HF_ENDPOINT, and the provider is named per call.
  ( unset HF_ENDPOINT HF_TOKEN
    export HOME="$ROOT/home" KL_HUB_URL="$KLOUDLITE_HUB_URL" KL_HUB_TOKEN="$TOKEN" PATH="$ROOT/venv/bin:$PATH"
    mkdir -p "$HOME"
    "$KL" hf --provider kloudlite download alice/m config.json >/dev/null
    ls "$HOME/.cache/kloudlite/hub"/models--alice--m >/dev/null
    [ ! -e "$HOME/.cache/huggingface/hub/models--alice--m" ]
    "$PY" - <<'PYEOF'
import os
from kloudlite import hub
p = hub.download_file("alice/m", "config.json", provider="kloudlite")
assert p.startswith(os.path.expanduser("~/.cache/kloudlite/hub")), p
PYEOF
  )
}
```

Add `run provider-wrapper sec_provider kl` after `run upload_folder+snapshot_download ...`. It needs `alice/m`, which that section uploads.

The script `export`s `HF_ENDPOINT`/`HF_TOKEN` at line ~52. The subshell `unset` keeps that global intact for the other sections.

- [ ] **Step 2: Build and run**

Run: `CARGO_TARGET_DIR=/Volumes/kdisk/target-master cargo build -p kl -p kloudlite-server 2>&1 | tail -2` (check `bins/server/Cargo.toml` for the server package name and how the script finds `$BIN`), then `./tests/hub_e2e.sh 2>&1 | tail -12`
Expected: summary has `pass: provider-wrapper` and `PASS`. Exit 77 means skipped, which is not a pass; report it as a skip.

- [ ] **Step 3: Commit**

```bash
git add tests/hub_e2e.sh
git commit -m "Exercise the hub provider wrapper end to end"
```

---

### Task 7: Ship and drill

No new code. The images carry Tasks 1-3 (server for hub auth, agent for `KL_HUB_URL`, workspace image for `kl`).

- [ ] **Step 1: Full gate**

Run in one Sonnet subagent: `cargo test` and `cargo clippy --workspace --all-targets -- -D warnings`, both with `CARGO_TARGET_DIR=/Volumes/kdisk/target-master`. Report pass/fail plus failing lines only.

- [ ] **Step 2: Build images from a clean detached worktree**

`git worktree add --detach /Volumes/kdisk/rustic-git-wt/ship-hubwrap HEAD`, then `deploy/dev/dagger-ship.sh --source /Volumes/kdisk/rustic-git-wt/ship-hubwrap` with the AKS kubeconfig exported. Then `deploy/pin.sh <sha>`, commit the pin, and roll (`deploy/roll.sh`; if the classifier blocks it, hand the owner the `!` line). Apply `deploy/k3s/agent-daemonset.yaml` to k3s with `KUBECONFIG=/Users/karthik/rustic-git/.local/k3s.yaml`. Remove the ship worktree afterwards.

- [ ] **Step 3: Drill in a fresh workspace** (stopped and started after the agent roll, so the pod has `KL_HUB_URL`)

1. `env | grep -c HF_ENDPOINT` prints `0`. `kl hf download openai-community/gpt2 config.json` succeeds from huggingface.co.
2. `kl hf --provider kloudlite download karthik1729/gpt2-drill`, with no token typed, lands under `~/.cache/kloudlite/hub`.
3. `pip install "kloudlite @ git+...#subdirectory=sdk/python"`, then `hub.download("karthik1729/gpt2-drill", provider="kloudlite")`, then `AutoModelForCausalLM.from_pretrained(path)` generates text.
4. An agent session asked "load the gpt2-drill model from our hub" uses `provider="kloudlite"` unprompted.

- [ ] **Step 4: Ledger**

Add one line to `.local/HANDOFF-bench-ship.md`: shipped sha, pin, drill result per step.
