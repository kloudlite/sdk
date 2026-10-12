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
const LATE_PROVIDER: &str = "put --provider before the hf arguments: kl hf --provider kloudlite <hf arguments>";
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
                // Hugging Face token to our hub. `token=True` calls (whoami) read every source
                // directly and bypass the implicit-token switch, so close them all.
                None => {
                    remove.extend(["HF_TOKEN", "HUGGING_FACE_HUB_TOKEN", "HF_OIDC_RESOURCE"]);
                    set.push(("HF_TOKEN_PATH", "/dev/null".into()));
                    set.push(("HF_HUB_DISABLE_IMPLICIT_TOKEN", "1".into()));
                }
            }
            Ok(ChildEnv { set, remove })
        }
        other => Err(format!("unknown provider `{other}`: use huggingface or kloudlite")),
    }
}

/// A `--provider` among the hf arguments was meant for us; passing it on would send the call to
/// the default hub without a word.
pub fn check_args(args: &[String]) -> Result<(), String> {
    match args.iter().any(|a| a == "--provider" || a.starts_with("--provider=")) {
        true => Err(LATE_PROVIDER.to_string()),
        false => Ok(()),
    }
}

/// Runs `hf` and exits with its code.
pub fn run(provider: &str, args: &[String]) -> Result<(), String> {
    check_args(args)?;
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
            assert_eq!(e.remove, vec!["HF_TOKEN", "HUGGING_FACE_HUB_TOKEN", "HF_OIDC_RESOURCE"]);
            assert_eq!(get(&e, "HF_TOKEN_PATH"), Some("/dev/null"));
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

    #[test]
    fn a_late_provider_is_refused() {
        let args = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        for late in [args(&["download", "x", "--provider", "kloudlite"]), args(&["download", "x", "--provider=kloudlite"])] {
            assert!(check_args(&late).unwrap_err().contains("before the hf arguments"));
        }
        assert!(check_args(&args(&["download", "x", "--repo-type", "dataset"])).is_ok());
    }
}
