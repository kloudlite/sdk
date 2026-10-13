//! One `ToolSet` per family; `Registry` is what `/tools/{name}` dispatches on. Every call is logged once
//! (`ide.call`: tool, ms, ok, bytes) — the pod log is the record, there is no metrics endpoint.
pub mod container;
pub mod exec;
pub mod files;
pub mod graft;
pub mod patch;
pub mod watch;

use futures::future::BoxFuture;
use serde_json::Value;
use std::time::Instant;

#[derive(Debug)]
pub enum ToolError {
    Unknown(String),
    Invalid(String),
    Denied(String),
    Failed(String),
}

impl std::fmt::Display for ToolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ToolError::Unknown(t) => write!(f, "unknown tool {t}"),
            ToolError::Invalid(m) | ToolError::Denied(m) | ToolError::Failed(m) => f.write_str(m),
        }
    }
}

/// The HTTP status a tool error is: 404 unknown, 400 bad arguments, 403 outside the home, 500 failed.
pub fn status_of(e: &ToolError) -> axum::http::StatusCode {
    use axum::http::StatusCode;
    match e {
        ToolError::Unknown(_) => StatusCode::NOT_FOUND,
        ToolError::Invalid(_) => StatusCode::BAD_REQUEST,
        ToolError::Denied(_) => StatusCode::FORBIDDEN,
        ToolError::Failed(_) => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

pub struct Tool {
    pub name: &'static str,
    pub description: &'static str,
    pub schema: Value,
}

pub trait ToolSet: Send + Sync {
    fn tools(&self) -> Vec<Tool>;
    fn call<'a>(&'a self, name: &'a str, args: Value) -> BoxFuture<'a, Result<Value, ToolError>>;
}

pub struct Registry {
    sets: Vec<Box<dyn ToolSet>>,
    /// Built once: `call` used to ask every set for its whole `tools()` vec on every request,
    /// which rebuilt every schema to find one name (2026-09-12).
    index: std::collections::HashMap<String, usize>,
}

impl Registry {
    pub fn new(sets: Vec<Box<dyn ToolSet>>) -> Self {
        let mut index = std::collections::HashMap::new();
        for (i, s) in sets.iter().enumerate() {
            for t in s.tools() {
                index.insert(t.name.to_string(), i);
            }
        }
        Registry { sets, index }
    }

    pub fn tools(&self) -> Vec<Tool> {
        self.sets.iter().flat_map(|s| s.tools()).collect()
    }

    pub async fn call(&self, name: &str, args: Value) -> Result<Value, ToolError> {
        let start = Instant::now();
        let set = self.index.get(name).map(|i| &self.sets[*i]);
        let r = match set {
            Some(s) => s.call(name, args).await,
            None => Err(ToolError::Unknown(name.to_string())),
        };
        let bytes = r.as_ref().map(|v| v.to_string().len()).unwrap_or(0);
        tracing::info!(tool = name, ms = start.elapsed().as_millis() as u64, ok = r.is_ok(), bytes, "ide.call");
        r
    }
}

/// Argument readers with the tool's name in every error.
pub(crate) fn str_arg<'a>(args: &'a Value, key: &str) -> Result<&'a str, ToolError> {
    args.get(key).and_then(Value::as_str).ok_or_else(|| ToolError::Invalid(format!("`{key}` (string) is required")))
}
pub(crate) fn opt_str<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Value::as_str)
}
/// A non-negative integer argument, taken from a JSON number OR a decimal string.
///
/// `as_u64` alone answers `None` for `"16000"` and for `16000.0`, and every caller of this defaults
/// to 0 on `None` — so a client that quoted its cursor got the log from byte 0 on every poll,
/// silently, with no error to read (workspace session, 2026-09-18). A value that is neither is
/// still `None`, because a cursor we cannot parse must not read as "from the start".
pub(crate) fn opt_u64(args: &Value, key: &str) -> Option<u64> {
    match args.get(key)? {
        Value::Number(n) => n.as_u64().or_else(|| n.as_f64().filter(|f| *f >= 0.0).map(|f| f as u64)),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}
/// A flag, taken from a JSON bool OR the string `"true"`/`"false"`.
///
/// `as_bool` alone read `"detach": "true"` as false, so a quoted flag ran the server as a job that
/// blocked for the whole timeout, three times over, with no error to read (workspace agent,
/// 2026-10-13). Anything else stays false, the default every caller already has.
pub(crate) fn opt_bool(args: &Value, key: &str) -> bool {
    match args.get(key) {
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => s.trim().eq_ignore_ascii_case("true"),
        _ => false,
    }
}

/// An argv array's entries. A non-string used to be dropped silently, which shifted every
/// argument after it and ran a different command than the caller wrote (2026-09-12).
pub(crate) fn argv(a: &[Value]) -> Result<Vec<&str>, ToolError> {
    a.iter().map(|v| v.as_str().ok_or_else(|| ToolError::Invalid(format!("cmd[]: `{v}` is not a string")))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_argv_entry_that_is_not_a_string_is_refused_by_value() {
        assert_eq!(argv(&[serde_json::json!("echo"), serde_json::json!("hi")]).unwrap(), vec!["echo", "hi"]);
        let e = argv(&[serde_json::json!("echo"), serde_json::json!(1)]).unwrap_err();
        assert!(matches!(e, ToolError::Invalid(ref m) if m.contains('1')), "{e:?}");
    }

    #[test]
    fn a_quoted_flag_reads_as_the_flag() {
        let f = |v: Value| opt_bool(&serde_json::json!({ "detach": v }), "detach");
        assert!(f(serde_json::json!(true)) && f(serde_json::json!("true")) && f(serde_json::json!(" TRUE ")));
        assert!(!f(serde_json::json!(false)) && !f(serde_json::json!("false")) && !f(serde_json::json!("yes")) && !f(serde_json::json!(1)));
    }
}
