//! The root LaunchDaemon. Installed by `sudo kl-connect env daemon install`: root because the daemon
//! rewrites /etc/hosts and creates a utun device; `--home` because root's own home holds no login.
use std::path::{Path, PathBuf};

pub const LABEL: &str = "io.kloudlite.kl-connect";
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
    std::fs::write(PLIST, plist(&bin, &home)).map_err(|e| format!("write {PLIST}: {e}"))?;
    launchctl(&["bootstrap", "system", PLIST])
}

pub fn uninstall() -> Result<(), String> {
    let _ = launchctl(&["bootout", &format!("system/{LABEL}")]);
    let _ = std::fs::remove_file(PLIST);
    let cur = std::fs::read_to_string("/etc/hosts").map_err(|e| e.to_string())?;
    let (t, _) = super::hosts::render(&cur, None);
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
