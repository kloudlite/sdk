//! The thin api client. Errors are strings because every one of them is printed and exited on.

#[derive(serde::Deserialize)]
pub struct Workspace {
    pub id: String,
    pub name: String,
    pub state: String,
    #[serde(default)]
    pub packages: Vec<String>,
}

/// Serialized too: `kl-connect ws ssh` hands it to the ProxyCommand child through the environment.
#[derive(serde::Deserialize, serde::Serialize)]
pub struct Session {
    /// The workspace it was minted for — the api resolves a name, so this is how the CLI learns
    /// the id without listing.
    pub id: String,
    pub token: String,
    pub gateway: String,
    pub host_key: String,
}

/// One client for the process: it pools connections, and the timeouts stop `kl` hanging forever
/// behind a black-holed network — every call it makes is a small JSON request.
pub fn client() -> &'static reqwest::Client {
    static C: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    C.get_or_init(|| {
        reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(5))
            .timeout(std::time::Duration::from_secs(15))
            .build()
            .expect("building an http client")
    })
}

/// 401 is the one status callers branch on (an expired cli token), so it is its own variant.
pub enum Error {
    Unauthorized,
    /// 403 / 404: access lost or the thing is gone. The env daemon tears down on these but only
    /// retries on every other failure, so they cannot be folded into `Other`.
    Gone(u16, String),
    Other(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Unauthorized => write!(f, "your login has expired — run `kl-connect login`"),
            Error::Gone(c, m) => write!(f, "{c}: {m}"),
            Error::Other(m) => write!(f, "{m}"),
        }
    }
}

async fn json<T: serde::de::DeserializeOwned>(r: reqwest::Response) -> Result<T, Error> {
    let status = r.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        return Err(Error::Unauthorized);
    }
    let body = r.text().await.map_err(|e| Error::Other(e.to_string()))?;
    if !status.is_success() {
        // The api's error bodies are `{"error": "…"}`; anything else is shown as it came.
        let msg = serde_json::from_str::<serde_json::Value>(&body)
            .ok()
            .and_then(|v| v.get("error").and_then(|e| e.as_str()).map(str::to_string))
            .unwrap_or_else(|| body.trim().to_string());
        let code = status.as_u16();
        if code == 403 || code == 404 {
            return Err(Error::Gone(code, msg));
        }
        return Err(Error::Other(format!("{code}: {msg}")));
    }
    serde_json::from_str(&body).map_err(|e| Error::Other(e.to_string()))
}

pub async fn list(
    cfg: &crate::config::Config,
    team: Option<&str>,
) -> Result<Vec<Workspace>, Error> {
    let mut req = client()
        .get(format!("{}/v1/workspaces", cfg.api))
        .bearer_auth(&cfg.token);
    if let Some(t) = team {
        req = req.query(&[("team", t)]);
    }
    json(req.send().await.map_err(|e| Error::Other(e.to_string()))?).await
}

/// `target` is an id or a name; the api resolves either (see `api::ssh_session` on the server).
///
/// PERCENT-ENCODED into the path: the name comes from a person's command line, and a `/` or a `?`
/// in it changed which endpoint was called rather than being sent as a name (2026-09-12).
pub async fn ssh_session(cfg: &crate::config::Config, target: &str) -> Result<Session, Error> {
    let target = path_segment(target);
    let r = client()
        .post(format!("{}/v1/workspaces/{target}/ssh-session", cfg.api))
        .bearer_auth(&cfg.token)
        .send()
        .await
        .map_err(|e| Error::Other(e.to_string()))?;
    json(r).await
}

/// One path segment, percent-encoded. Everything outside the unreserved set is escaped, which is
/// stricter than a URL needs and exactly what a segment carrying somebody's typed name wants.
fn path_segment(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            b => format!("%{b:02X}"),
        })
        .collect()
}

/// One condition off the builder's `Environment` status -- the same shape `GET
/// /v1/internal/builders/{slug}` and `/v1/builders/me` both answer.
#[derive(serde::Deserialize)]
pub struct Condition {
    // Kept for the shape's sake, unused by `kl-connect builder status`'s rendering.
    #[allow(dead_code)]
    #[serde(rename = "type")]
    pub type_: String,
    pub status: String,
    pub message: String,
}

#[derive(serde::Deserialize)]
pub struct BuilderStatus {
    pub state: String,
    pub ready: bool,
    #[serde(default)]
    pub conditions: Vec<Condition>,
}

/// `POST /v1/bench/session`'s 201 body.
#[derive(serde::Deserialize)]
pub struct BenchSession {
    #[allow(dead_code)]
    pub id: String,
    pub token: String,
    pub gateway: String,
    #[allow(dead_code)]
    pub expires_at: String,
}

/// `bench_session`'s answer: `Ready` on 201, `Waking(state)` on 202 (the api's `{"state": phase}`
/// while it wakes or starts).
pub enum SessionAnswer {
    Ready(BenchSession),
    Waking(String),
}

#[derive(serde::Deserialize)]
struct BenchState {
    state: String,
}

async fn bench_request(
    req: reqwest::RequestBuilder,
) -> Result<reqwest::Response, Error> {
    req.send().await.map_err(|e| Error::Other(e.to_string()))
}

/// `region` is sent only for a personal bench (its first use binds the person's region); a
/// team's is the team's.
pub async fn create_bench(
    cfg: &crate::config::Config,
    team: Option<&str>,
    region: Option<&str>,
) -> Result<serde_json::Value, Error> {
    let body = serde_json::json!({"team": team, "region": region});
    let r = bench_request(
        client()
            .post(format!("{}/v1/bench", cfg.api))
            .bearer_auth(&cfg.token)
            .json(&body),
    )
    .await?;
    json(r).await
}

#[allow(dead_code)] // part of the api surface the brief specifies; no `kl-connect bench status` yet
pub async fn get_bench(
    cfg: &crate::config::Config,
    team: Option<&str>,
) -> Result<serde_json::Value, Error> {
    let mut req = client().get(format!("{}/v1/bench", cfg.api)).bearer_auth(&cfg.token);
    if let Some(t) = team {
        req = req.query(&[("team", t)]);
    }
    json(bench_request(req).await?).await
}

/// 201 -> `Ready(session)`; 202 -> `Waking(state)`; anything else an `Error` carrying the body's
/// `error`.
pub async fn bench_session(cfg: &crate::config::Config, team: Option<&str>) -> Result<SessionAnswer, Error> {
    let mut req = client()
        .post(format!("{}/v1/bench/session", cfg.api))
        .bearer_auth(&cfg.token);
    if let Some(t) = team {
        req = req.query(&[("team", t)]);
    }
    let r = bench_request(req).await?;
    if r.status() == reqwest::StatusCode::ACCEPTED {
        let body = r.text().await.map_err(|e| Error::Other(e.to_string()))?;
        let st: BenchState = serde_json::from_str(&body).map_err(|e| Error::Other(e.to_string()))?;
        return Ok(SessionAnswer::Waking(st.state));
    }
    Ok(SessionAnswer::Ready(json(r).await?))
}

/// `GET /v1/builders/me` -- the caller's own hidden builder, or their team's with `team`.
pub async fn builder_status(
    cfg: &crate::config::Config,
    team: Option<&str>,
) -> Result<BuilderStatus, Error> {
    let mut req = client()
        .get(format!("{}/v1/builders/me", cfg.api))
        .bearer_auth(&cfg.token);
    if let Some(t) = team {
        req = req.query(&[("team", t)]);
    }
    json(req.send().await.map_err(|e| Error::Other(e.to_string()))?).await
}

/// One of the caller's spaces (`GET /v1/me/environments`): the environment that team follows.
#[derive(serde::Deserialize, Clone, PartialEq)]
pub struct Space {
    pub team: String,
    pub environment: String,
    #[allow(dead_code)]
    pub region: Option<String>,
}

/// The slice of `model::Environment` the env daemon reads.
#[derive(serde::Deserialize)]
pub struct EnvDoc {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub services: Vec<EnvService>,
}

#[derive(serde::Deserialize)]
pub struct EnvService {
    pub name: String,
    #[serde(default)]
    #[allow(dead_code)]
    pub ports: Vec<u16>,
}

pub async fn spaces(cfg: &crate::config::Config) -> Result<Vec<Space>, Error> {
    let r = client()
        .get(format!("{}/v1/me/environments", cfg.api))
        .bearer_auth(&cfg.token)
        .send()
        .await
        .map_err(|e| Error::Other(e.to_string()))?;
    json(r).await
}

/// `GET /v1/me/environments/watch`: holds up to 50 s. `Some(tag)` = the selection differs from
/// `seen` (or `seen` was None); `None` = nothing changed (304).
pub async fn watch_spaces(cfg: &crate::config::Config, seen: Option<&str>) -> Result<Option<String>, Error> {
    let mut rb = client()
        .get(format!("{}/v1/me/environments/watch", cfg.api))
        .bearer_auth(&cfg.token)
        .timeout(std::time::Duration::from_secs(70));
    if let Some(t) = seen {
        rb = rb.header(reqwest::header::IF_NONE_MATCH, t);
    }
    let r = rb.send().await.map_err(|e| Error::Other(e.to_string()))?;
    if r.status() == reqwest::StatusCode::NOT_MODIFIED {
        return Ok(None);
    }
    if r.status().is_success() {
        let tag = r.headers().get(reqwest::header::ETAG).and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
        return Ok(Some(tag));
    }
    // Not a success: `json` turns it into the same Unauthorized / Gone / Other as every call.
    json::<serde_json::Value>(r).await.map(|_| None)
}

/// `GET /v1/environments/{id}/watch`: same contract as `watch_spaces`, for one environment's
/// object (services added, removed or readied). `Some(tag)` = it moved; `None` = 304.
pub async fn watch_environment(cfg: &crate::config::Config, id: &str, seen: Option<&str>) -> Result<Option<String>, Error> {
    let mut rb = client()
        .get(format!("{}/v1/environments/{}/watch", cfg.api, path_segment(id)))
        .bearer_auth(&cfg.token)
        .timeout(std::time::Duration::from_secs(70));
    if let Some(t) = seen {
        rb = rb.header(reqwest::header::IF_NONE_MATCH, t);
    }
    let r = rb.send().await.map_err(|e| Error::Other(e.to_string()))?;
    if r.status() == reqwest::StatusCode::NOT_MODIFIED {
        return Ok(None);
    }
    if r.status().is_success() {
        let tag = r.headers().get(reqwest::header::ETAG).and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
        return Ok(Some(tag));
    }
    json::<serde_json::Value>(r).await.map(|_| None)
}

pub async fn set_space(cfg: &crate::config::Config, team: &str, env: &str) -> Result<(), Error> {
    let r = client()
        .put(format!("{}/v1/me/environments/{}", cfg.api, path_segment(team)))
        .bearer_auth(&cfg.token)
        .json(&serde_json::json!({ "environment": env }))
        .send()
        .await
        .map_err(|e| Error::Other(e.to_string()))?;
    empty(r).await
}

/// 404 is success: the space is already clear, which is what the caller asked for.
pub async fn clear_space(cfg: &crate::config::Config, team: &str) -> Result<(), Error> {
    let r = client()
        .delete(format!("{}/v1/me/environments/{}", cfg.api, path_segment(team)))
        .bearer_auth(&cfg.token)
        .send()
        .await
        .map_err(|e| Error::Other(e.to_string()))?;
    match empty(r).await {
        Err(Error::Gone(404, _)) => Ok(()),
        r => r,
    }
}

/// `GET /v1/environments?owner={team}`: the team's environments, for resolving a name to an id.
pub async fn environments(cfg: &crate::config::Config, team: &str) -> Result<Vec<EnvDoc>, Error> {
    let r = client()
        .get(format!("{}/v1/environments", cfg.api))
        .query(&[("owner", team)])
        .bearer_auth(&cfg.token)
        .send()
        .await
        .map_err(|e| Error::Other(e.to_string()))?;
    json(r).await
}

pub async fn environment(cfg: &crate::config::Config, id: &str) -> Result<EnvDoc, Error> {
    let r = client()
        .get(format!("{}/v1/environments/{}", cfg.api, path_segment(id)))
        .bearer_auth(&cfg.token)
        .send()
        .await
        .map_err(|e| Error::Other(e.to_string()))?;
    json(r).await
}

#[derive(Clone, Copy)]
pub enum Kind {
    Workspace,
    Environment,
}

impl Kind {
    fn path(self) -> &'static str {
        match self {
            Kind::Workspace => "workspaces",
            Kind::Environment => "environments",
        }
    }
}

#[derive(Debug, PartialEq)]
pub enum Step {
    Ready,
    /// Idle-stopped: the agent stopped it, so the first caller to want it back wakes it.
    Wake,
    /// A person's stop (no `idle_since`): never auto-woken.
    Stopped,
    Wait(String),
}

// ponytail: fixed 2 s poll and 180 s wake budget (cold start includes `nix copy`), same as the
// harness's ensure.ts; a knob if anyone waits longer.
const POLL: std::time::Duration = std::time::Duration::from_secs(2);
const WAKE_BUDGET: std::time::Duration = std::time::Duration::from_secs(180);

/// The harness predicate (harness/packages/tools/src/ensure.ts): workspaces report `ready`,
/// environments `state: running`.
fn is_ready(d: &serde_json::Value) -> bool {
    d["ready"] == true || matches!(d["state"].as_str(), Some("ready" | "running"))
}

/// Why it is not ready: the first `ConditionDoc { ready, reason, message }` under these keys that
/// is not ready and names a reason, else the state.
fn reason(d: &serde_json::Value) -> String {
    ["packages_status", "degraded", "placed"]
        .iter()
        .map(|k| &d[*k])
        .find(|c| c["ready"] == false && c["reason"].as_str().is_some_and(|r| !r.is_empty()))
        .and_then(|c| c["reason"].as_str())
        .or(d["state"].as_str())
        .unwrap_or("unknown")
        .to_string()
}

pub fn decide(d: &serde_json::Value) -> Step {
    if is_ready(d) {
        Step::Ready
    } else if !d["idle_since"].is_null() {
        Step::Wake
    } else if d["state"] == "stopped" {
        Step::Stopped
    } else {
        Step::Wait(reason(d))
    }
}

async fn get_doc(cfg: &crate::config::Config, kind: Kind, id: &str) -> Result<serde_json::Value, Error> {
    let r = client()
        .get(format!("{}/v1/{}/{}", cfg.api, kind.path(), path_segment(id)))
        .bearer_auth(&cfg.token)
        .send()
        .await
        .map_err(|e| Error::Other(e.to_string()))?;
    json(r).await
}

/// One door before any tunnel: ready returns; idle-stopped is started and waited for; a person's
/// stop stays stopped and says so.
pub async fn ensure_running(cfg: &crate::config::Config, kind: Kind, id: &str) -> Result<(), Error> {
    let d = get_doc(cfg, kind, id).await?;
    match decide(&d) {
        Step::Ready => return Ok(()),
        Step::Stopped => {
            return Err(Error::Other(format!(
                "{id} is stopped; start it from the console (kl-connect has no start command)"
            )))
        }
        Step::Wake => {
            eprintln!("waking {id}…");
            let r = client()
                .post(format!("{}/v1/{}/{}/start", cfg.api, kind.path(), path_segment(id)))
                .bearer_auth(&cfg.token)
                .send()
                .await
                .map_err(|e| Error::Other(e.to_string()))?;
            empty(r).await?;
        }
        Step::Wait(_) => {} // already coming up: just poll
    }
    let until = std::time::Instant::now() + WAKE_BUDGET;
    let mut failed = false;
    loop {
        tokio::time::sleep(POLL).await;
        let d = match get_doc(cfg, kind, id).await {
            Ok(d) => {
                failed = false;
                d
            }
            // one failed GET mid-poll is a blip; the second in a row is real
            Err(e) if failed => return Err(e),
            Err(_) => {
                failed = true;
                continue;
            }
        };
        if is_ready(&d) {
            return Ok(());
        }
        if matches!(d["state"].as_str(), Some("error" | "deleted")) {
            return Err(Error::Other(format!("{id} is {}: {}", d["state"].as_str().unwrap_or(""), reason(&d))));
        }
        if std::time::Instant::now() >= until {
            return Err(Error::Other(format!("{id} did not become ready: {}", reason(&d))));
        }
    }
}

/// Resolves a typed id or name, wakes the workspace, and returns its id. This runs BEFORE the ssh
/// mint: the api refuses to mint for a stopped workspace (409), and the session token lives 60 s,
/// so it must not be minted ahead of a wake that can take 180 s. `GET /v1/workspaces/{id}` takes
/// ids only, hence the list.
pub async fn wake_workspace(cfg: &crate::config::Config, target: &str) -> Result<String, Error> {
    let all = list(cfg, None).await?;
    let id = all
        .iter()
        .find(|w| w.id == target)
        .or_else(|| all.iter().find(|w| w.name == target))
        .map(|w| w.id.clone())
        .ok_or_else(|| Error::Gone(404, format!("no workspace {target}")))?;
    ensure_running(cfg, Kind::Workspace, &id).await?;
    Ok(id)
}

/// Keeps a connected environment from idling out (204).
pub async fn hold_env(cfg: &crate::config::Config, id: &str) -> Result<(), Error> {
    let r = client()
        .post(format!("{}/v1/environments/{}/hold", cfg.api, path_segment(id)))
        .bearer_auth(&cfg.token)
        .send()
        .await
        .map_err(|e| Error::Other(e.to_string()))?;
    empty(r).await
}

/// A 2xx with no body we need (204 on PUT/DELETE of a space); failures go through `json`'s mapping.
async fn empty(r: reqwest::Response) -> Result<(), Error> {
    if r.status().is_success() {
        return Ok(());
    }
    json::<serde_json::Value>(r).await.map(|_| ())
}

#[cfg(test)]
mod wake_tests {
    use super::*;

    #[test]
    fn decide_table() {
        use serde_json::json;
        assert_eq!(decide(&json!({"state": "running", "ready": true})), Step::Ready);
        assert_eq!(decide(&json!({"state": "stopped", "idle_since": "2026-10-12T10:00:00Z"})), Step::Wake);
        assert_eq!(decide(&json!({"state": "stopped"})), Step::Stopped);
        assert_eq!(
            decide(&json!({"state": "starting", "packages_status": {"ready": false, "reason": "KeysNotReady", "message": ""}})),
            Step::Wait("KeysNotReady".into())
        );
    }
}
