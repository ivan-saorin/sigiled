//! Durable questions shared by external drivers and the human browser.
//! An answer is a record, never an authorization grant or an execution command.
use crate::{
    auth::Actor,
    overview::Page,
    work_items::{Error, Item},
    AppState,
};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Question {
    pub question: String,
    pub context: String,
    #[serde(default)]
    pub answer: Option<Reply>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reply {
    pub text: String,
    pub actor: String,
    pub at: u64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Create {
    pub id: String,
    pub title: String,
    pub question: String,
    #[serde(default)]
    pub context: String,
    #[serde(default)]
    pub source_link: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Answer {
    pub expected_revision: u64,
    pub text: String,
}
pub fn registered(s: &AppState, p: &str) -> Result<(), Error> {
    if s.registry.contains(p) {
        Ok(())
    } else {
        Err(Error(StatusCode::NOT_FOUND, "unknown_project"))
    }
}
pub async fn list(
    _actor: Actor,
    State(s): State<AppState>,
    Path(p): Path<String>,
    Query(q): Query<Page>,
) -> Response {
    if let Err(e) = registered(&s, &p) {
        return e.into_response();
    }
    let (offset, limit) = match q.bounds() {
        Ok(v) => v,
        Err(e) => return *e,
    };
    match s.work_items.list_requests(&p, offset, limit) {
        Ok(v) => Json(v).into_response(),
        Err(e) => e.into_response(),
    }
}
pub async fn get(
    _actor: Actor,
    State(s): State<AppState>,
    Path((p, id)): Path<(String, String)>,
) -> Response {
    if let Err(e) = registered(&s, &p) {
        return e.into_response();
    }
    match s.work_items.get_request(&p, &id) {
        Ok(v) if v.request.is_some() => Json(v).into_response(),
        Ok(_) => Error(StatusCode::NOT_FOUND, "request_not_found").into_response(),
        Err(e) => e.into_response(),
    }
}
pub async fn create(
    actor: Actor,
    State(s): State<AppState>,
    Path(p): Path<String>,
    Json(body): Json<Create>,
) -> Response {
    if let Err(e) = registered(&s, &p) {
        return e.into_response();
    }
    match s.work_items.create_request(&p, &actor.driver, body) {
        Ok(v) => (StatusCode::CREATED, Json(v)).into_response(),
        Err(e) => e.into_response(),
    }
}
pub fn answer(s: &AppState, p: &str, id: &str, actor: &str, body: Answer) -> Result<Item, Error> {
    registered(s, p)?;
    s.work_items.answer_request(p, id, actor, body)
}
