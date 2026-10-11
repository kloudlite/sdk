//! The `/etc/hosts` block: one marked block, everything outside it left byte for byte. A bare name
//! the person defined outside the block is never shadowed -- skipped and reported instead.
//! Written by temp file in the same directory and rename, so a killed daemon never leaves half a file.
use std::{net::Ipv4Addr, path::Path};

pub const BEGIN: &str = "# kl-connect begin";
pub const END: &str = "# kl-connect end";

pub struct Block {
    pub env: String,
    pub name: String,
    pub entries: Vec<(String, Ipv4Addr)>,
}

/// New file text and the names skipped because the person's own lines already define them.
pub fn render(current: &str, block: Option<&Block>) -> (String, Vec<String>) {
    let mut out = String::new();
    let mut inside = false;
    for line in current.split_inclusive('\n') {
        let l = line.trim_end_matches(['\n', '\r']);
        if l.starts_with(BEGIN) {
            inside = true;
            continue;
        }
        if inside {
            if l == END {
                inside = false;
            }
            continue;
        }
        out.push_str(line);
    }
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    let Some(b) = block else { return (out, vec![]) };
    let taken: std::collections::HashSet<String> = out
        .lines()
        .map(|l| l.split('#').next().unwrap_or(""))
        .flat_map(|l| l.split_whitespace().skip(1))
        .map(str::to_string)
        .collect();
    let mut skipped = vec![];
    let mut body = format!("{BEGIN} env={} name={}\n", b.env, b.name);
    for (n, ip) in &b.entries {
        if taken.contains(n) {
            skipped.push(n.clone());
            continue;
        }
        body.push_str(&format!("{ip}\t{n}\n"));
    }
    body.push_str(END);
    body.push('\n');
    out.push_str(&body);
    (out, skipped)
}

/// Mode is carried over from the file being replaced (0644 for `/etc/hosts`): the temp file is
/// created by us, so without this the rename could hand `/etc/hosts` over with a different mode.
pub fn install(path: &Path, text: &str) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let tmp = path.with_file_name(format!("{}.kl-connect.tmp", path.file_name().unwrap().to_string_lossy()));
    let _ = std::fs::remove_file(&tmp);
    std::fs::write(&tmp, text).map_err(|e| format!("write {}: {e}", tmp.display()))?;
    let mode = std::fs::metadata(path).map(|m| m.permissions().mode() & 0o7777).unwrap_or(0o644);
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(mode))
        .map_err(|e| format!("chmod {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("rename over {}: {e}", path.display()))
}

/// macOS caches /etc/hosts in the directory service and mDNSResponder.
pub fn flush() {
    let _ = std::process::Command::new("dscacheutil").arg("-flushcache").status();
    let _ = std::process::Command::new("killall").args(["-HUP", "mDNSResponder"]).status();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block() -> Block {
        Block {
            env: "ab12".into(),
            name: "dev".into(),
            entries: vec![
                ("mongodb".into(), "10.43.12.7".parse().unwrap()),
                ("api".into(), "10.43.40.2".parse().unwrap()),
            ],
        }
    }
    const BASE: &str = "127.0.0.1\tlocalhost\n::1\tlocalhost\n";

    #[test]
    fn writes_one_block_and_leaves_the_rest() {
        let (t, skipped) = render(BASE, Some(&block()));
        assert!(t.starts_with(BASE));
        assert!(t.ends_with("# kl-connect begin env=ab12 name=dev\n10.43.12.7\tmongodb\n10.43.40.2\tapi\n# kl-connect end\n"));
        assert!(skipped.is_empty());
    }

    #[test]
    fn rewriting_is_idempotent_and_removal_restores_the_file() {
        let (once, _) = render(BASE, Some(&block()));
        assert_eq!(render(&once, Some(&block())).0, once);
        assert_eq!(render(&once, None).0, BASE);
    }

    #[test]
    fn a_name_defined_outside_is_skipped() {
        let base = format!("{BASE}10.0.0.9 api # mine\n");
        let (t, skipped) = render(&base, Some(&block()));
        assert_eq!(skipped, vec!["api".to_string()]);
        assert!(t.contains("10.43.12.7\tmongodb\n") && !t.contains("10.43.40.2"));
        assert!(t.starts_with(&base));
    }

    #[test]
    fn a_file_without_a_trailing_newline_keeps_its_last_line() {
        let (t, _) = render("127.0.0.1 localhost", Some(&block()));
        assert!(t.starts_with("127.0.0.1 localhost\n# kl-connect begin"));
        assert_eq!(render(&t, None).0, "127.0.0.1 localhost\n");
    }

    #[test]
    fn a_stale_temp_file_is_replaced() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("hosts");
        std::fs::write(&p, BASE).unwrap();
        std::fs::write(d.path().join("hosts.kl-connect.tmp"), "garbage").unwrap();
        install(&p, "new\n").unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "new\n");
        assert!(!d.path().join("hosts.kl-connect.tmp").exists());
    }
}
