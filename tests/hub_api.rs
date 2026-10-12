//! Hub API over the hub listener: repo lifecycle, auth, visibility, reads, commits.
mod common;

use common::*;
use kloudlite_storage::HubKind;
use serde_json::{json, Value};

struct Hub {
    base: String,
    e: TestEnv,
    alice: String,
    bob: String,
}

async fn hub() -> Hub {
    let e = env().await;
    let app = app(e.store.clone()).await;
    let alice = e.store.create_token("alice").await.unwrap();
    let bob = e.store.create_token("bob").await.unwrap();
    let port = serve_hub(app).await;
    Hub { base: format!("http://127.0.0.1:{port}"), e, alice, bob }
}

fn client() -> reqwest::Client {
    reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap()
}

async fn create(h: &Hub, token: Option<&str>, body: Value) -> reqwest::Response {
    let mut r = client().post(format!("{}/api/repos/create", h.base)).json(&body);
    if let Some(t) = token {
        r = r.bearer_auth(t);
    }
    r.send().await.unwrap()
}

#[tokio::test]
async fn create_defaults_to_private_model() {
    let h = hub().await;
    let r = create(&h, Some(&h.alice), json!({"name": "bert"})).await;
    assert_eq!(r.status(), 200);
    let v: Value = r.json().await.unwrap();
    assert_eq!(v["url"], format!("{}/alice/bert", h.base));
    assert!(h.e.store.hub_exists(HubKind::Model, "alice", "bert").await.unwrap());
    assert!(!h.e.store.hub_public(HubKind::Model, "alice", "bert").await.unwrap());
}

#[tokio::test]
async fn create_dataset_public_with_organization() {
    let h = hub().await;
    let r = create(&h, Some(&h.alice), json!({"name": "squad", "organization": "alice", "type": "dataset", "private": false})).await;
    assert_eq!(r.status(), 200);
    let v: Value = r.json().await.unwrap();
    assert_eq!(v["url"], format!("{}/datasets/alice/squad", h.base));
    assert!(h.e.store.hub_public(HubKind::Dataset, "alice", "squad").await.unwrap());
}

#[tokio::test]
async fn create_accepts_owner_slash_name() {
    let h = hub().await;
    assert_eq!(create(&h, Some(&h.alice), json!({"name": "alice/gpt"})).await.status(), 200);
    assert!(h.e.store.hub_exists(HubKind::Model, "alice", "gpt").await.unwrap());
}

#[tokio::test]
async fn create_twice_is_409_with_url() {
    let h = hub().await;
    create(&h, Some(&h.alice), json!({"name": "bert"})).await;
    let r = create(&h, Some(&h.alice), json!({"name": "bert"})).await;
    assert_eq!(r.status(), 409);
    let v: Value = r.json().await.unwrap();
    assert_eq!(v["url"], format!("{}/alice/bert", h.base));
}

#[tokio::test]
async fn create_refuses_space_anonymous_and_strangers() {
    let h = hub().await;
    assert_eq!(create(&h, Some(&h.alice), json!({"name": "s", "type": "space"})).await.status(), 400);
    assert_eq!(create(&h, None, json!({"name": "bert"})).await.status(), 401);
    assert_eq!(create(&h, Some("not-a-token"), json!({"name": "bert"})).await.status(), 401);
    let r = create(&h, Some(&h.bob), json!({"name": "bert", "organization": "alice"})).await;
    assert_eq!(r.status(), 403);
    assert!(!h.e.store.hub_exists(HubKind::Model, "alice", "bert").await.unwrap());
}

#[tokio::test]
async fn basic_user_must_name_the_token_owner() {
    let h = hub().await;
    let ok = client().get(format!("{}/api/whoami-v2", h.base)).basic_auth("x", Some(&h.alice)).send().await.unwrap();
    assert_eq!(ok.status(), 200);
    let bad = client().get(format!("{}/api/whoami-v2", h.base)).basic_auth("bob", Some(&h.alice)).send().await.unwrap();
    assert_eq!(bad.status(), 401);
}

#[tokio::test]
async fn whoami_names_the_caller() {
    let h = hub().await;
    let r = client().get(format!("{}/api/whoami-v2", h.base)).bearer_auth(&h.alice).send().await.unwrap();
    assert_eq!(r.status(), 200);
    let v: Value = r.json().await.unwrap();
    assert_eq!(v["name"], "alice");
    assert_eq!(v["type"], "user");
    let anon = client().get(format!("{}/api/whoami-v2", h.base)).send().await.unwrap();
    assert_eq!(anon.status(), 401);
}

#[tokio::test]
async fn delete_by_owner_only() {
    let h = hub().await;
    create(&h, Some(&h.alice), json!({"name": "bert"})).await;
    let del = |t: String| {
        let url = format!("{}/api/repos/delete", h.base);
        async move { client().delete(url).bearer_auth(t).json(&json!({"name": "bert", "organization": "alice"})).send().await.unwrap() }
    };
    let r = del(h.bob.clone()).await;
    assert_eq!(r.status(), 404);
    assert_eq!(r.headers()["x-error-code"], "RepoNotFound");
    assert_eq!(del(h.alice.clone()).await.status(), 200);
    assert!(!h.e.store.hub_exists(HubKind::Model, "alice", "bert").await.unwrap());
    assert_eq!(del(h.alice.clone()).await.status(), 404);
}
