//! `kl-connect ws follow <ws> <id>` — a detached workspace process (`exec detach: true`) as a
//! local command: its output on stdout/stderr, its exit code as ours. Claude Code runs it as a
//! background shell, so a server in the workspace reads like one started on the laptop.
//!
//! Same door as `kl-connect mcp` (`bench::serve`): no ssh tunnel and no workspace token. It polls
//! `process_output` about once a second, carrying the stdout/stderr byte cursors (`since`,
//! `since_err`). When the link dies (laptop sleep: the 15 s keepalive ends `serve`), it reconnects
//! with the same cursors, so nothing is lost or printed twice while the process ring still holds it;
//! bytes the ring dropped in between are reported on stderr.

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream, Lines};

const RECONNECT: std::time::Duration = std::time::Duration::from_secs(5);
const POLL: std::time::Duration = std::time::Duration::from_secs(1);

pub async fn follow(team: Option<&str>, ws: &str, id: &str) -> Result<(), String> {
    let cfg = crate::config::load()?;
    let mut cur = (0u64, 0u64);
    loop {
        match once(&cfg, team, ws, id, &mut cur).await {
            Ok(Ok(code)) => std::process::exit(code),
            Ok(Err(e)) => return Err(e),
            Err(e) => {
                eprintln!("kl-connect: follow: {e}; reconnecting");
                tokio::time::sleep(RECONNECT).await;
            }
        }
    }
}

/// One connection. Outer Err: the link (retry); inner Err: the tool refused (fatal); Ok: exit code.
async fn once(cfg: &crate::config::Config, team: Option<&str>, ws: &str, id: &str, cur: &mut (u64, u64)) -> Result<Result<i32, String>, String> {
    let (to_bench, bench_r) = tokio::io::duplex(1 << 20);
    let (bench_w, from_bench) = tokio::io::duplex(1 << 20);
    let mut door = Door { w: to_bench, lines: BufReader::new(from_bench).lines(), n: 0 };
    let poll = async {
        door.call("initialize", json!({ "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": { "name": "kl-connect-follow", "version": "1" } })).await?;
        door.send(json!({ "jsonrpc": "2.0", "method": "notifications/initialized" })).await?;
        loop {
            let args = json!({ "workspace": ws, "id": id, "since": cur.0, "since_err": cur.1 });
            let r = door.call("tools/call", json!({ "name": "process_output", "arguments": args })).await?;
            let text = r.pointer("/result/content/0/text").and_then(Value::as_str).unwrap_or("");
            if r.pointer("/result/isError") == Some(&Value::Bool(true)) {
                return Ok(Err(format!("process_output: {text}")));
            }
            let out: Value = serde_json::from_str(text).map_err(|e| format!("process_output reply: {e}"))?;
            let s = step(&out, cur);
            tokio::io::stdout().write_all(s.stdout.as_bytes()).await.map_err(|e| e.to_string())?;
            tokio::io::stdout().flush().await.map_err(|e| e.to_string())?;
            eprint!("{}", s.stderr);
            if let Some(code) = s.exit {
                return Ok(Ok(code));
            }
            if s.quiet {
                tokio::time::sleep(POLL).await;
            }
        }
    };
    tokio::select! {
        r = poll => r,
        r = crate::bench::serve(cfg, team, bench_r, bench_w) => Err(r.err().unwrap_or_else(|| "bench closed".into())),
    }
}

/// What one `process_output` reply prints, and whether the process is done.
#[derive(Debug, PartialEq)]
struct Step {
    stdout: String,
    stderr: String,
    exit: Option<i32>,
    quiet: bool,
}

fn step(out: &Value, cur: &mut (u64, u64)) -> Step {
    let s = |k: &str| out.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let n = |k: &str| out.get(k).and_then(Value::as_u64).unwrap_or(0);
    let (stdout, mut stderr) = (s("stdout"), s("stderr"));
    for (k, which) in [("dropped", "stdout"), ("dropped_err", "stderr")] {
        if n(k) > 0 {
            stderr.push_str(&format!("kl-connect: follow: {} bytes of {which} dropped by the workspace\n", n(k)));
        }
    }
    let next = (out.get("next").and_then(Value::as_u64).unwrap_or(cur.0), out.get("next_err").and_then(Value::as_u64).unwrap_or(cur.1));
    let quiet = next == *cur;
    *cur = next;
    let running = out.get("state").and_then(Value::as_str) == Some("running");
    // a process that ended with output still unread prints it first: exit only on a drained reply
    let exit = (!running && quiet).then(|| match out.get("exit_code").and_then(Value::as_i64) {
        Some(c) if c >= 0 => c as i32,
        // no code, or -1 (which a shell shows as 255): a signal ended it. The state cannot say so,
        // it is only `running` or `exited`, so the line is fixed.
        _ => {
            stderr.push_str("kl-connect: follow: process was killed (no exit code)\n");
            1
        }
    });
    Step { stdout, stderr, exit, quiet }
}

/// The wire to the bench: one `mcp` op per JSON-RPC message, replies matched by `re`.
struct Door {
    w: DuplexStream,
    lines: Lines<BufReader<DuplexStream>>,
    n: u64,
}

impl Door {
    async fn send(&mut self, msg: Value) -> Result<u64, String> {
        self.n += 1;
        let line = format!("{}\n", json!({ "id": self.n, "op": "mcp", "args": msg }));
        self.w.write_all(line.as_bytes()).await.map_err(|e| e.to_string())?;
        Ok(self.n)
    }

    async fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let n = self.send(json!({ "jsonrpc": "2.0", "id": self.n + 1, "method": method, "params": params })).await?;
        loop {
            let line = self.lines.next_line().await.map_err(|e| e.to_string())?.ok_or("bench closed")?;
            let Ok(f) = serde_json::from_str::<Value>(&line) else { continue };
            if f.get("re").and_then(Value::as_u64) != Some(n) {
                continue;
            }
            if f.get("ok") == Some(&Value::Bool(true)) {
                return Ok(f.get("value").cloned().unwrap_or(Value::Null));
            }
            return Err(f.pointer("/error/message").and_then(Value::as_str).unwrap_or("bench refused").to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursors_advance_and_exit_waits_for_drained_output() {
        let mut cur = (0, 0);
        let s = step(&json!({ "stdout": "hi\n", "stderr": "", "next": 3, "next_err": 0, "state": "running", "exit_code": null }), &mut cur);
        assert_eq!((s.stdout.as_str(), s.exit, s.quiet, cur), ("hi\n", None, false, (3, 0)));
        let s = step(&json!({ "stdout": "", "stderr": "", "next": 3, "next_err": 0, "state": "running" }), &mut cur);
        assert!(s.quiet && s.exit.is_none());
        // ended with output still arriving: print it, exit on the next (drained) reply
        let s = step(&json!({ "stdout": "bye\n", "next": 7, "next_err": 0, "state": "exited", "exit_code": 2 }), &mut cur);
        assert_eq!((s.exit, cur), (None, (7, 0)));
        let s = step(&json!({ "stdout": "", "next": 7, "next_err": 0, "state": "exited", "exit_code": 2 }), &mut cur);
        assert_eq!(s.exit, Some(2));
        // killed: no exit code is a failure
        let s = step(&json!({ "next": 7, "next_err": 0, "state": "exited", "exit_code": null }), &mut cur);
        assert_eq!((s.exit, s.stderr.as_str()), (Some(1), "kl-connect: follow: process was killed (no exit code)\n"));
        let s = step(&json!({ "next": 7, "next_err": 0, "state": "exited", "exit_code": -1 }), &mut cur);
        assert_eq!((s.exit, s.stderr.as_str()), (Some(1), "kl-connect: follow: process was killed (no exit code)\n"));
    }

    #[test]
    fn dropped_bytes_are_reported() {
        let mut cur = (0, 0);
        let s = step(&json!({ "stdout": "x", "dropped": 10, "dropped_err": 0, "next": 11, "state": "running" }), &mut cur);
        assert!(s.stderr.contains("10 bytes of stdout dropped"), "{}", s.stderr);
    }
}
