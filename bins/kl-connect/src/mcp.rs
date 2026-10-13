//! `kl-connect mcp [team]` — the bench's every tool as a stdio MCP server for any client on the
//! laptop (`claude mcp add kl -- kl-connect mcp`): every workspace, environment and pod tool, each
//! aimed by its own `workspace`/`env` argument. Same door as `bench-proxy` (`bench::serve`): each
//! JSON-RPC line on stdin goes to the bench as one `mcp` wire op, and each reply's value comes back
//! as one line on stdout. The bench's events (`{ev, ...}`) are not for this client and are dropped.
//! stdout carries JSON-RPC only; the wait spinner and errors go to stderr.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

const RECONNECT: std::time::Duration = std::time::Duration::from_secs(5);

/// The process outlives the link (incident 2026-10-13: one dropped bench link exited the server,
/// and Claude Code never restarts a plugin's stdio server, so every kl tool stayed dead). A dropped
/// link reconnects after `RECONNECT`; requests it carried answer an error; the client's
/// `initialize` is replayed on the new link and its second reply swallowed.
pub async fn mcp(team: Option<&str>) -> Result<(), String> {
    let cfg = crate::config::load()?;
    let link = std::sync::Arc::new(Link::default());
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    let up = link.clone();
    tokio::spawn(async move {
        let mut lines = BufReader::new(tokio::io::stdin()).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let Ok(msg) = serde_json::from_str::<Value>(&line) else { continue };
            if tx.send(up.client(msg)).is_err() {
                break;
            }
        }
        // dropping `tx` ends `mcp`: the client closed stdin
    });
    let mut out = tokio::io::stdout();
    let mut first = true;
    loop {
        let (mut to_bench, bench_r) = tokio::io::duplex(1 << 20);
        let (bench_w, from_bench) = tokio::io::duplex(1 << 20);
        if !first {
            for line in link.replay() {
                to_bench.write_all(line.as_bytes()).await.map_err(|e| e.to_string())?;
            }
        }
        first = false;
        let pump_up = async {
            while let Some(line) = rx.recv().await {
                if to_bench.write_all(line.as_bytes()).await.is_err() {
                    return false;
                }
            }
            true
        };
        let pump_down = async {
            let mut lines = BufReader::new(from_bench).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let Some(reply) = link.reply(&line) else { continue };
                if out.write_all(reply.as_bytes()).await.is_err() || out.flush().await.is_err() {
                    return true;
                }
            }
            false
        };
        let done = tokio::select! {
            closed = pump_up => closed,
            gone = pump_down => gone,
            r = crate::bench::serve(&cfg, team, bench_r, bench_w) => {
                eprintln!("kl-connect: mcp: {}; reconnecting", r.err().unwrap_or_else(|| "bench closed".into()));
                false
            }
        };
        if done {
            return Ok(());
        }
        for line in link.dropped() {
            out.write_all(line.as_bytes()).await.map_err(|e| e.to_string())?;
        }
        out.flush().await.map_err(|e| e.to_string())?;
        tokio::time::sleep(RECONNECT).await;
    }
}

/// Wire ids across links: which client id each open request answers, which replies are replays.
#[derive(Default)]
struct Link(Mutex<Wire>);

#[derive(Default)]
struct Wire {
    n: u64,
    ids: HashMap<u64, Value>, // wire id → the client's JSON-RPC id, for a failed op (`ok: false` carries no JSON-RPC id)
    replayed: HashSet<u64>,
    hello: Vec<Value>, // `initialize` and `notifications/initialized`, as the client sent them
}

impl Link {
    fn client(&self, msg: Value) -> String {
        let w = &mut *self.0.lock().unwrap();
        w.n += 1;
        if let Some(id) = msg.get("id").filter(|id| !id.is_null()) {
            w.ids.insert(w.n, id.clone());
        }
        if matches!(msg.get("method").and_then(Value::as_str), Some("initialize" | "notifications/initialized")) {
            w.hello.push(msg.clone());
        }
        request(w.n, msg)
    }

    fn replay(&self) -> Vec<String> {
        let w = &mut *self.0.lock().unwrap();
        let hello = w.hello.clone();
        hello
            .into_iter()
            .map(|msg| {
                w.n += 1;
                w.replayed.insert(w.n);
                request(w.n, msg)
            })
            .collect()
    }

    fn reply(&self, line: &str) -> Option<String> {
        let w = &mut *self.0.lock().unwrap();
        let re = serde_json::from_str::<Value>(line).ok()?.get("re")?.as_u64()?;
        if w.replayed.remove(&re) {
            return None;
        }
        reply(line, &mut w.ids)
    }

    /// The open requests of a dead link, each as a JSON-RPC error on its own id.
    fn dropped(&self) -> Vec<String> {
        let w = &mut *self.0.lock().unwrap();
        w.replayed.clear();
        w.ids
            .drain()
            .map(|(_, id)| format!("{}\n", json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32603, "message": "kl-connect: the link to the bench dropped; reconnecting, call again" } })))
            .collect()
    }
}

/// One client message as one wire request line.
fn request(n: u64, msg: Value) -> String {
    format!("{}\n", json!({ "id": n, "op": "mcp", "args": msg }))
}

/// A bench frame as the stdout line it becomes, if any: an op's value (null for a notification
/// writes nothing), a failed op as a JSON-RPC error on the client's id, anything else dropped.
fn reply(line: &str, ids: &mut HashMap<u64, Value>) -> Option<String> {
    let f: Value = serde_json::from_str(line).ok()?;
    let re = f.get("re")?.as_u64()?;
    let id = ids.remove(&re);
    if f.get("ok") == Some(&Value::Bool(true)) {
        let v = f.get("value").filter(|v| !v.is_null())?;
        return Some(format!("{v}\n"));
    }
    let e = f.get("error");
    let message = e.and_then(|e| e.get("message")).and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| e.map(Value::to_string).unwrap_or_default());
    Some(format!("{}\n", json!({ "jsonrpc": "2.0", "id": id?, "error": { "code": -32603, "message": message } })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_client_line_becomes_one_mcp_op() {
        let line = request(3, json!({ "jsonrpc": "2.0", "id": 7, "method": "tools/list" }));
        let v: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v, json!({ "id": 3, "op": "mcp", "args": { "jsonrpc": "2.0", "id": 7, "method": "tools/list" } }));
        assert!(line.ends_with('\n'));
    }

    #[test]
    fn replies_events_and_failures() {
        let mut ids = HashMap::from([(1, json!(7)), (2, json!("a"))]);
        let ok = reply(r#"{"re":1,"ok":true,"value":{"jsonrpc":"2.0","id":7,"result":{}}}"#, &mut ids).unwrap();
        assert_eq!(serde_json::from_str::<Value>(&ok).unwrap(), json!({ "jsonrpc": "2.0", "id": 7, "result": {} }));
        // a notification's op answers null: nothing on stdout
        assert_eq!(reply(r#"{"re":5,"ok":true,"value":null}"#, &mut ids), None);
        // bench events and junk are not this client's
        assert_eq!(reply(r#"{"ev":"bench","key":"*","event":{}}"#, &mut ids), None);
        assert_eq!(reply("not json", &mut ids), None);
        let err = reply(r#"{"re":2,"ok":false,"error":{"name":"Error","message":"unknown op: mcp"}}"#, &mut ids).unwrap();
        assert_eq!(serde_json::from_str::<Value>(&err).unwrap(), json!({ "jsonrpc": "2.0", "id": "a", "error": { "code": -32603, "message": "unknown op: mcp" } }));
        assert!(ids.is_empty());
    }

    #[test]
    fn a_dropped_link_fails_open_calls_and_replays_hello_silently() {
        let link = Link::default();
        link.client(json!({ "jsonrpc": "2.0", "id": 0, "method": "initialize", "params": {} }));
        link.client(json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }));
        assert!(link.reply(r#"{"re":1,"ok":true,"value":{"jsonrpc":"2.0","id":0,"result":{}}}"#).is_some());
        link.client(json!({ "jsonrpc": "2.0", "id": 9, "method": "tools/call" }));
        let failed = link.dropped();
        assert_eq!(failed.len(), 1);
        assert_eq!(serde_json::from_str::<Value>(&failed[0]).unwrap()["id"], json!(9));
        let again = link.replay();
        let wire: Vec<Value> = again.iter().map(|l| serde_json::from_str(l).unwrap()).collect();
        assert_eq!((wire[0]["id"].clone(), wire[0]["args"]["method"].clone()), (json!(4), json!("initialize")));
        assert_eq!(wire[1]["args"]["method"], json!("notifications/initialized"));
        // the replayed initialize answers again: not the client's
        assert_eq!(link.reply(r#"{"re":4,"ok":true,"value":{"jsonrpc":"2.0","id":0,"result":{}}}"#), None);
        assert_eq!(link.reply(r#"{"re":5,"ok":true,"value":null}"#), None);
        // a new call after the reconnect goes through
        link.client(json!({ "jsonrpc": "2.0", "id": 10, "method": "tools/list" }));
        assert!(link.reply(r#"{"re":6,"ok":true,"value":{"jsonrpc":"2.0","id":10,"result":{}}}"#).is_some());
    }
}
