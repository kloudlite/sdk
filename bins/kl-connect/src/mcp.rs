//! `kl-connect mcp [team]` — the bench's every tool as a stdio MCP server for any client on the
//! laptop (`claude mcp add kl -- kl-connect mcp`): every workspace, environment and pod tool, each
//! aimed by its own `workspace`/`env` argument. Same door as `bench-proxy` (`bench::serve`): each
//! JSON-RPC line on stdin goes to the bench as one `mcp` wire op, and each reply's value comes back
//! as one line on stdout. The bench's events (`{ev, ...}`) are not for this client and are dropped.
//! stdout carries JSON-RPC only; the wait spinner and errors go to stderr.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

pub async fn mcp(team: Option<&str>) -> Result<(), String> {
    let cfg = crate::config::load()?;
    let (mut to_bench, bench_r) = tokio::io::duplex(1 << 20);
    let (bench_w, from_bench) = tokio::io::duplex(1 << 20);
    // wire id → the client's JSON-RPC id, for a failed op (`ok: false` carries no JSON-RPC id)
    let ids: Arc<Mutex<HashMap<u64, Value>>> = Arc::default();

    let up_ids = ids.clone();
    tokio::spawn(async move {
        let mut lines = BufReader::new(tokio::io::stdin()).lines();
        let mut n = 0u64;
        while let Ok(Some(line)) = lines.next_line().await {
            let Ok(msg) = serde_json::from_str::<Value>(&line) else { continue };
            n += 1;
            if let Some(id) = msg.get("id").filter(|id| !id.is_null()) {
                up_ids.lock().unwrap().insert(n, id.clone());
            }
            if to_bench.write_all(request(n, msg).as_bytes()).await.is_err() {
                break;
            }
        }
        // dropping `to_bench` ends the pump: the client closed stdin
    });
    tokio::spawn(async move {
        let mut lines = BufReader::new(from_bench).lines();
        let mut out = tokio::io::stdout();
        while let Ok(Some(line)) = lines.next_line().await {
            let Some(reply) = reply(&line, &mut ids.lock().unwrap()) else { continue };
            if out.write_all(reply.as_bytes()).await.is_err() || out.flush().await.is_err() {
                break;
            }
        }
    });
    crate::bench::serve(&cfg, team, bench_r, bench_w).await
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
}
