//! The root LaunchDaemon. Installed by `sudo kl-connect env daemon install`: root because the daemon
//! rewrites /etc/hosts and creates a utun device; `--home` because root's own home holds no login.
//! The plist runs a root-owned copy at `BIN`, never the binary the person ran: a user-writable file
//! run by a root KeepAlive daemon is a root escalation. Reinstalling after an upgrade refreshes it.
use std::path::{Path, PathBuf};

pub const LABEL: &str = "io.kloudlite.kl-connect";
pub const BIN: &str = "/usr/local/libexec/kl-connect";
pub const PLIST: &str = "/Library/LaunchDaemons/io.kloudlite.kl-connect.plist";

pub fn plist(bin: &Path, home: &Path) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>Label</key><string>{LABEL}</string>
  <key>ProgramArguments</key><array>
    <string>{}</string><string>env</string><string>daemon</string><string>--home</string><string>{}</string>
  </array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>StandardErrorPath</key><string>/var/log/kl-connect.log</string>
</dict></plist>
"#,
        xml(bin),
        xml(home)
    )
}

fn xml(p: &Path) -> String {
    p.display().to_string().replace('&', "&amp;").replace('<', "&lt;")
}

pub fn install() -> Result<(), String> {
    let user = std::env::var("SUDO_USER").map_err(|_| "run with sudo: sudo kl-connect env daemon install".to_string())?;
    let out = std::process::Command::new("dscl")
        .args([".", "-read", &format!("/Users/{user}"), "NFSHomeDirectory"])
        .output()
        .map_err(|e| e.to_string())?;
    let home = PathBuf::from(
        String::from_utf8_lossy(&out.stdout)
            .trim()
            .strip_prefix("NFSHomeDirectory: ")
            .ok_or(format!("no home for {user}"))?,
    );
    let bin = std::env::current_exe().and_then(|p| p.canonicalize()).map_err(|e| e.to_string())?;
    let _ = launchctl(&["bootout", &format!("system/{LABEL}")]); // reinstall replaces
    copy_bin(&bin, Path::new(BIN))?;
    std::fs::write(PLIST, plist(Path::new(BIN), &home)).map_err(|e| format!("write {PLIST}: {e}"))?;
    launchctl(&["bootstrap", "system", PLIST])
}

/// Temp file in the target's directory, then rename: a running daemon keeps its old inode.
fn copy_bin(from: &Path, to: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let dir = to.parent().ok_or("no parent")?;
    std::fs::create_dir_all(dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o755)).map_err(|e| e.to_string())?;
    let tmp = dir.join(".kl-connect.tmp");
    let _ = std::fs::remove_file(&tmp);
    std::fs::copy(from, &tmp).map_err(|e| format!("copy to {}: {e}", tmp.display()))?;
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755)).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, to).map_err(|e| format!("rename over {}: {e}", to.display()))
}

pub fn uninstall() -> Result<(), String> {
    let _ = launchctl(&["bootout", &format!("system/{LABEL}")]);
    let _ = std::fs::remove_file(PLIST);
    let _ = std::fs::remove_file(BIN);
    let cur = std::fs::read_to_string("/etc/hosts").map_err(|e| e.to_string())?;
    let (t, _) = super::hosts::render(&cur, None)?;
    if t != cur {
        super::hosts::install(Path::new("/etc/hosts"), &t)?;
        super::hosts::flush();
    }
    Ok(())
}

fn launchctl(args: &[&str]) -> Result<(), String> {
    let o = std::process::Command::new("/bin/launchctl").args(args).output().map_err(|e| e.to_string())?;
    if o.status.success() {
        Ok(())
    } else {
        Err(format!("launchctl {}: {}", args.join(" "), String::from_utf8_lossy(&o.stderr).trim()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_binary_copy_is_executable_and_replaces_the_old_one() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let (src, dst) = (d.path().join("src"), d.path().join("lib/kl"));
        std::fs::write(&src, "new").unwrap();
        copy_bin(&src, &dst).unwrap();
        std::fs::write(&src, "newer").unwrap();
        copy_bin(&src, &dst).unwrap();
        assert_eq!(std::fs::read_to_string(&dst).unwrap(), "newer");
        assert_eq!(std::fs::metadata(&dst).unwrap().permissions().mode() & 0o777, 0o755);
    }

    #[test]
    fn the_plist_runs_the_daemon_for_that_home() {
        let p = plist(Path::new("/usr/local/bin/kl-connect"), Path::new("/Users/a"));
        for s in [
            "<string>io.kloudlite.kl-connect</string>",
            "<string>/usr/local/bin/kl-connect</string>",
            "<string>env</string>",
            "<string>daemon</string>",
            "<string>--home</string>",
            "<string>/Users/a</string>",
            "<key>RunAtLoad</key>",
            "<key>KeepAlive</key>",
        ] {
            assert!(p.contains(s), "{s}");
        }
    }
}
