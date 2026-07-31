//! The human face. Thin: every handler here delegates straight to `Board`, so
//! the pixels and the agent API can't drift apart.

use crate::model::{ItemId, State, WorkItem};
use crate::store::{AncestryLine, SharedBoard};

use axum::extract::{Path, State as AxState};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

pub struct ApiError(String);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (StatusCode::BAD_REQUEST, Json(serde_json::json!({ "error": self.0 }))).into_response()
    }
}

impl<E: std::fmt::Display> From<E> for ApiError {
    fn from(e: E) -> Self {
        ApiError(e.to_string())
    }
}

type ApiResult<T> = Result<Json<T>, ApiError>;

pub fn routes() -> Router<SharedBoard> {
    Router::new()
        .route("/version", get(version))
        .route("/board", get(board))
        .route("/digest", get(digest))
        .route("/items", post(create_item))
        .route("/items/{id}", get(item_detail))
        .route("/items/{id}/transition", post(transition))
        .route("/items/{id}/steer", post(steer))
        .route("/items/{id}/pin", post(pin))
        .route("/items/{id}/halt", post(halt))
        .route("/items/{id}/answer", post(answer))
        .route("/items/{id}/approve", post(approve))
        .route("/items/{id}/request-changes", post(request_changes))
        .route("/items/{id}/cut", post(cut_scope))
}

#[derive(Serialize)]
pub struct Version {
    version: &'static str,
}

async fn version() -> Json<Version> {
    Json(Version { version: env!("CARGO_PKG_VERSION") })
}

async fn board(AxState(b): AxState<SharedBoard>) -> Json<crate::store::Snapshot> {
    Json(b.snapshot())
}

async fn digest(AxState(b): AxState<SharedBoard>) -> Json<crate::store::Digest> {
    Json(b.digest())
}

/// Layer 3 of the cognitive model: is this right? Transcript, diff, cost — and
/// the intent chain that says why it exists at all.
#[derive(Serialize)]
pub struct ItemDetail {
    #[serde(flatten)]
    item: WorkItem,
    ancestry: Vec<AncestryLine>,
    constraints: Vec<String>,
    children: Vec<ItemId>,
}

async fn item_detail(
    AxState(b): AxState<SharedBoard>,
    Path(id): Path<ItemId>,
) -> ApiResult<ItemDetail> {
    let item = b.get(id).ok_or_else(|| ApiError(format!("no work item #{id}")))?;
    Ok(Json(ItemDetail {
        ancestry: b.ancestry(id),
        constraints: b.inherited_pins(id),
        children: b.children_of(id),
        item,
    }))
}

#[derive(Deserialize)]
pub struct CreateItem {
    parent: Option<ItemId>,
    title: String,
    intent: String,
    #[serde(default)]
    definition_of_done: Option<String>,
    #[serde(default)]
    capability: Option<String>,
    #[serde(default)]
    above_line: bool,
}

async fn create_item(
    AxState(b): AxState<SharedBoard>,
    Json(req): Json<CreateItem>,
) -> ApiResult<WorkItem> {
    let item = b.create(
        req.parent,
        req.title,
        req.intent,
        req.definition_of_done,
        crate::model::Origin::Human,
        req.above_line,
        req.capability,
    );
    // A goal dropped in plain language starts shaping immediately.
    let item = b.transition(item.id, State::Shaping, "human", None).unwrap_or(item);
    Ok(Json(item))
}

#[derive(Deserialize)]
pub struct TransitionReq {
    to: State,
    #[serde(default)]
    reason: Option<String>,
}

async fn transition(
    AxState(b): AxState<SharedBoard>,
    Path(id): Path<ItemId>,
    Json(req): Json<TransitionReq>,
) -> ApiResult<WorkItem> {
    Ok(Json(b.transition(id, req.to, "human", req.reason)?))
}

#[derive(Deserialize)]
pub struct TextReq {
    text: String,
}

#[derive(Deserialize)]
pub struct ReasonReq {
    #[serde(default)]
    reason: Option<String>,
}

async fn steer(
    AxState(b): AxState<SharedBoard>,
    Path(id): Path<ItemId>,
    Json(req): Json<TextReq>,
) -> ApiResult<WorkItem> {
    Ok(Json(b.steer(id, req.text).map_err(ApiError)?))
}

async fn pin(
    AxState(b): AxState<SharedBoard>,
    Path(id): Path<ItemId>,
    Json(req): Json<TextReq>,
) -> ApiResult<WorkItem> {
    Ok(Json(b.pin(id, req.text).map_err(ApiError)?))
}

async fn halt(
    AxState(b): AxState<SharedBoard>,
    Path(id): Path<ItemId>,
    Json(req): Json<ReasonReq>,
) -> ApiResult<WorkItem> {
    Ok(Json(b.halt(id, req.reason).map_err(ApiError)?))
}

#[derive(Deserialize)]
pub struct AnswerReq {
    choice: String,
}

async fn answer(
    AxState(b): AxState<SharedBoard>,
    Path(id): Path<ItemId>,
    Json(req): Json<AnswerReq>,
) -> ApiResult<WorkItem> {
    Ok(Json(b.answer_escalation(id, req.choice).map_err(ApiError)?))
}

async fn approve(
    AxState(b): AxState<SharedBoard>,
    Path(id): Path<ItemId>,
) -> ApiResult<WorkItem> {
    Ok(Json(b.approve_review(id).map_err(ApiError)?))
}

async fn request_changes(
    AxState(b): AxState<SharedBoard>,
    Path(id): Path<ItemId>,
    Json(req): Json<TextReq>,
) -> ApiResult<WorkItem> {
    Ok(Json(b.request_changes(id, req.text).map_err(ApiError)?))
}

async fn cut_scope(
    AxState(b): AxState<SharedBoard>,
    Path(id): Path<ItemId>,
    Json(req): Json<ReasonReq>,
) -> ApiResult<Vec<ItemId>> {
    Ok(Json(b.cut_scope(id, req.reason).map_err(ApiError)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn version_reports_the_crate_version() {
        let Json(v) = version().await;
        assert_eq!(v.version, env!("CARGO_PKG_VERSION"));
        assert_eq!(
            serde_json::to_value(&v).unwrap(),
            serde_json::json!({ "version": env!("CARGO_PKG_VERSION") }),
        );
    }
}
