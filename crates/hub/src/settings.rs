//! `PUT {api}/settings` `{"private": bool}`, huggingface_hub's `update_repo_settings`. Routed by
//! the `mdl/`/`ds/` key like every other `{api}` route, so the flag is written on the owning node.
use crate::auth::open;
use crate::{internal, HubState};
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use kloudlite_core::httpx::Trusted;
use kloudlite_storage::HubKind;
use serde::Deserialize;

#[derive(Deserialize)]
pub struct Body {
    private: Option<bool>,
}

pub async fn put(
    State(st): State<HubState>,
    Extension(kind): Extension<HubKind>,
    trusted: Option<Extension<Trusted>>,
    headers: HeaderMap,
    Path((owner, name)): Path<(String, String)>,
    Json(b): Json<Body>,
) -> Response {
    let a = match open(&st, kind, trusted.as_deref(), &headers, &owner, &name, true).await {
        Ok(a) => a,
        Err(r) => return r,
    };
    if let Some(private) = b.private {
        if let Err(e) = st.app.store.hub_set_private(&a.repo, private).await {
            return internal(e);
        }
    }
    Json(serde_json::json!({})).into_response()
}
