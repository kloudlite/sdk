//! Model hub round trip through the front door `huggingface_hub` uses: `hub.{url}` with the
//! personal token as a Bearer PAT. A 1 KiB regular file (no Xet), so this proves the hub listener,
//! its ingress and the commit/resolve path, not the CAS.
//!
//! The hub has no listing route, so the stage owns teardown of its one repo: `download` deletes
//! it, and `cleanup` is the idempotent backstop `registry::run` never needs but a failed upload
//! does. A run killed mid-journey leaves one tiny `run-…-hub` repo that nothing sweeps (ponytail:
//! add a hub listing and a `sweep_hub` beside `sweep_images` when litter shows up).

use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use base64::Engine;
use futures::FutureExt;
use serde_json::{json, Value};

use crate::ctx::Ctx;

const UPLOAD_CEILING: Duration = Duration::from_secs(30);
const DOWNLOAD_CEILING: Duration = Duration::from_secs(30);

fn base(c: &Ctx) -> String {
    c.cfg.hub_url.trim_end_matches('/').to_string()
}

fn payload() -> Vec<u8> {
    (0..1024u32).map(|i| b'a' + (i % 26) as u8).collect()
}

/// One hub call with the PAT; non-2xx is an `Err` carrying a clipped body, like `stages::call`.
async fn send(c: &Ctx, req: reqwest::RequestBuilder, pat: &str) -> Result<Vec<u8>> {
    let tp = super::traceparent();
    let r = super::probe_headers(req, &tp)
        .header(super::REQUEST_ID, super::request_id(c))
        .header("authorization", c.bearer(pat))
        .send()
        .await
        .map_err(|e| anyhow!("{}", e.without_url()))?;
    let status = r.status();
    let body = r.bytes().await.map(|b| b.to_vec()).map_err(|e| anyhow!("{} while reading the body of {status}", e.without_url()))?;
    if !status.is_success() {
        return Err(anyhow!("{status}: {}", super::clip(&String::from_utf8_lossy(&body))));
    }
    Ok(body)
}

pub async fn run(c: &mut Ctx) {
    let Some(pat) = c.state.token_value.clone() else {
        c.skip("hub.upload", "no personal token");
        c.skip("hub.download", "no personal token");
        return;
    };
    let name = format!("{}-hub", c.prefix());
    let owner = c.probe_user.clone();
    let up = upload(c, &pat, &owner, &name).await;
    if up {
        download(c, &pat, &owner, &name).await;
    } else {
        c.skip("hub.download", "nothing was uploaded");
    }
    // Idempotent: the download step already deleted it on the happy path.
    let _ = delete(c, &pat, &owner, &name).await;
}

async fn delete(c: &Ctx, pat: &str, owner: &str, name: &str) -> Result<()> {
    let req = c
        .http
        .delete(format!("{}/api/repos/delete", base(c)))
        .json(&json!({ "name": name, "organization": owner, "type": "model" }));
    send(c, req, pat).await.map(|_| ())
}

/// `hub.upload`: create the repo and commit one file; the answer names a `commitOid`.
async fn upload(c: &mut Ctx, pat: &str, owner: &str, name: &str) -> bool {
    c.step("hub.upload", UPLOAD_CEILING, |c| {
        let (pat, owner, name) = (pat.to_string(), owner.to_string(), name.to_string());
        async move {
            let create = c
                .http
                .post(format!("{}/api/repos/create", base(c)))
                .json(&json!({ "name": name, "organization": owner, "type": "model", "private": true }));
            send(c, create, &pat).await.context("could not create the repo")?;
            let content = base64::engine::general_purpose::STANDARD.encode(payload());
            let body = format!(
                "{}\n{}\n",
                json!({ "key": "header", "value": { "summary": "probe" } }),
                json!({ "key": "file", "value": { "path": "probe.txt", "content": content, "encoding": "base64" } }),
            );
            let commit = c
                .http
                .post(format!("{}/api/models/{owner}/{name}/commit/main", base(c)))
                .header("content-type", "application/x-ndjson")
                .body(body);
            let out = send(c, commit, &pat).await.context("could not commit")?;
            let v: Value = serde_json::from_slice(&out).unwrap_or(Value::Null);
            v.get("commitOid").and_then(Value::as_str).ok_or_else(|| anyhow!("the commit answered no commitOid"))?;
            Ok(())
        }
        .boxed()
    })
    .await
}

/// `hub.download`: the file comes back byte for byte, then the repo goes.
async fn download(c: &mut Ctx, pat: &str, owner: &str, name: &str) {
    c.step("hub.download", DOWNLOAD_CEILING, |c| {
        let (pat, owner, name) = (pat.to_string(), owner.to_string(), name.to_string());
        async move {
            let get = c.http.get(format!("{}/{owner}/{name}/resolve/main/probe.txt", base(c)));
            let got = send(c, get, &pat).await.context("could not resolve the file")?;
            if got != payload() {
                return Err(anyhow!("the downloaded bytes differ from the uploaded ones ({} bytes back)", got.len()));
            }
            delete(c, &pat, &owner, &name).await.context("could not delete the repo")
        }
        .boxed()
    })
    .await;
}
