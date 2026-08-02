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
        .route("/items/{id}", get(item_detail).delete(delete_item))
        .route("/items/{id}/delete", post(delete_item))
        .route("/items/{id}/logs", get(item_logs))
        .route("/items/{id}/transition", post(transition))
        .route("/items/{id}/update", post(update_item))
        .route("/items/{id}/steer", post(steer))
        .route("/items/{id}/plan", post(save_plan))
        .route("/items/{id}/halt", post(halt))
        .route("/items/{id}/park", post(park))
        .route("/items/{id}/unpark", post(unpark))
        .route("/items/{id}/answer", post(answer))
        .route("/items/{id}/approve", post(approve))
        .route("/items/{id}/approve-plan", post(approve_plan))
        .route("/items/{id}/request-changes", post(request_changes))
        .route("/items/{id}/cut", post(cut_scope))
        .route("/items/{id}/dispatch", post(dispatch_item))
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
    children: Vec<ItemId>,
    default_engine: String,
    default_model: String,
}

async fn item_detail(
    AxState(b): AxState<SharedBoard>,
    Path(id): Path<ItemId>,
) -> ApiResult<ItemDetail> {
    let item = b.get(id).ok_or_else(|| ApiError(format!("no work item #{id}")))?;
    let default_engine = b.schema.execution.agents.engine.clone();
    let default_model = b.schema.execution.agents.vertex.model.clone();
    Ok(Json(ItemDetail {
        ancestry: b.ancestry(id),
        children: b.children_of(id),
        item,
        default_engine,
        default_model,
    }))
}

#[derive(Serialize)]
pub struct LogResponse {
    pub claude: Vec<String>,
    pub openshell: Vec<String>,
}

async fn item_logs(
    AxState(b): AxState<SharedBoard>,
    Path(id): Path<ItemId>,
) -> ApiResult<LogResponse> {
    let item = b.get(id).ok_or_else(|| ApiError(format!("no work item #{id}")))?;
    let claude = b.get_agent_logs(id);

    let env_name = item
        .environment
        .clone()
        .unwrap_or_else(|| format!("honr-card-{id}-a{}", item.run_failures + 1));

    let os = crate::openshell::OpenShell::default();
    let openshell = if let Ok(logs) = os.logs(&env_name, 60).await {
        logs.lines().map(|s| s.to_string()).collect()
    } else {
        Vec::new()
    };

    Ok(Json(LogResponse { claude, openshell }))
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
    let item = b
        .create(
            req.parent,
            req.title,
            req.intent,
            req.definition_of_done,
            crate::model::Origin::Human,
            req.above_line,
            req.capability,
        )
        .map_err(ApiError)?;
    // A project dropped in plain language starts shaping immediately.
    let item = b.transition(item.id, State::Shaping, "human", None).unwrap_or(item);
    b.schedule_beads_mirror(item.id);
    for cid in b.children_of(item.id) {
        b.schedule_beads_mirror(cid);
    }

    Ok(Json(item))
}

/// Approve Plan: materialize the Project's Plan artifact into Backlog Tasks.
/// Never transitions the Project itself to Backlog.
async fn approve_plan(
    AxState(b): AxState<SharedBoard>,
    Path(id): Path<ItemId>,
) -> ApiResult<Vec<ItemId>> {
    let published = b.approve_plan(id).map_err(ApiError)?;
    for cid in &published {
        b.schedule_beads_mirror(*cid);
    }
    Ok(Json(published))
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

#[derive(Deserialize)]
pub struct PlanTaskBody {
    key: String,
    title: String,
    intent: String,
    definition_of_done: String,
    #[serde(default)]
    blocked_by_keys: Vec<String>,
    #[serde(default)]
    capability: Option<String>,
}

#[derive(Deserialize)]
pub struct SavePlanReq {
    #[serde(default)]
    summary: Option<String>,
    tasks: Vec<PlanTaskBody>,
    #[serde(default)]
    cancel_keys: Vec<String>,
}

/// Write / revise the Plan artifact (does not materialize Tasks — Approve Plan does).
async fn save_plan(
    AxState(b): AxState<SharedBoard>,
    Path(id): Path<ItemId>,
    Json(req): Json<SavePlanReq>,
) -> ApiResult<crate::model::PlanArtifact> {
    let summary = req.summary.unwrap_or_else(|| {
        b.get(id)
            .map(|i| i.intent.clone())
            .unwrap_or_default()
    });
    let tasks = req
        .tasks
        .into_iter()
        .map(|t| crate::model::PlanTaskSpec {
            key: t.key,
            title: t.title,
            intent: t.intent,
            definition_of_done: t.definition_of_done,
            blocked_by_keys: t.blocked_by_keys,
            capability: t.capability,
            item_id: None,
        })
        .collect();
    Ok(Json(
        b.propose_plan(id, summary, tasks, req.cancel_keys)
            .map_err(ApiError)?,
    ))
}

#[derive(Deserialize)]
pub struct UpdateItemReq {
    title: Option<String>,
    intent: Option<String>,
    definition_of_done: Option<String>,
    engine: Option<String>,
    #[serde(default)]
    project_prompt: Option<String>,
}

async fn update_item(
    AxState(b): AxState<SharedBoard>,
    Path(id): Path<ItemId>,
    Json(req): Json<UpdateItemReq>,
) -> ApiResult<WorkItem> {
    Ok(Json(
        b.update_item(
            id,
            req.title,
            req.intent,
            req.definition_of_done,
            req.engine,
            req.project_prompt,
        )
        .map_err(ApiError)?,
    ))
}

async fn halt(
    AxState(b): AxState<SharedBoard>,
    Path(id): Path<ItemId>,
    Json(req): Json<ReasonReq>,
) -> ApiResult<WorkItem> {
    Ok(Json(b.halt(id, req.reason).map_err(ApiError)?))
}

async fn park(
    AxState(b): AxState<SharedBoard>,
    Path(id): Path<ItemId>,
    Json(req): Json<ReasonReq>,
) -> ApiResult<WorkItem> {
    Ok(Json(b.park(id, req.reason).map_err(ApiError)?))
}

async fn unpark(
    AxState(b): AxState<SharedBoard>,
    Path(id): Path<ItemId>,
) -> ApiResult<WorkItem> {
    Ok(Json(b.unpark(id).map_err(ApiError)?))
}

/// Queue a Backlog card for the supervisor to claim. Explicit start — nothing auto-dispatches.
async fn dispatch_item(
    AxState(b): AxState<SharedBoard>,
    Path(id): Path<ItemId>,
) -> ApiResult<WorkItem> {
    Ok(Json(b.enqueue_dispatch(id).map_err(ApiError)?))
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
    let before: std::collections::HashSet<_> = b
        .get(id)
        .and_then(|i| i.parent)
        .map(|p| b.children_of(p))
        .unwrap_or_default()
        .into_iter()
        .collect();
    let item = b.approve_review(id).map_err(ApiError)?;
    if let Some(parent) = item.parent {
        for cid in b.children_of(parent) {
            if !before.contains(&cid) {
                b.schedule_beads_mirror(cid);
            }
        }
    }
    Ok(Json(item))
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

async fn delete_item(
    AxState(b): AxState<SharedBoard>,
    Path(id): Path<ItemId>,
) -> ApiResult<serde_json::Value> {
    b.delete_item(id).map_err(ApiError)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Where a card ran and what it produced have to survive the trip to the
    /// browser. The card face reads them off the board snapshot and the drawer
    /// off the detail payload, where the item is `#[serde(flatten)]`ed — so
    /// either can stop carrying them without a single type changing.
    #[tokio::test]
    async fn a_finished_card_carries_its_pr_and_sandbox_to_the_ui() {
        let b: SharedBoard = std::sync::Arc::new(crate::store::Board::new(
            crate::schema::Schema::default(),
            std::env::temp_dir().join("honr-test-nowrite.json"),
        ));
        let id = b
            .create(None, "t", "i", None, crate::model::Origin::Human, false, None)
            .expect("create")
            .id;
        b.set_environment(id, Some("honr-card-8-a1".into()));
        b.set_pr_url(id, Some("https://github.com/shanemcd/honr/pull/1".into()));

        let Json(snap) = board(AxState(b.clone())).await;
        let on_the_card = serde_json::to_value(&snap).unwrap();
        assert_eq!(on_the_card["items"][0]["pr_url"], "https://github.com/shanemcd/honr/pull/1");
        assert_eq!(on_the_card["items"][0]["environment"], "honr-card-8-a1");

        let Ok(Json(detail)) = item_detail(AxState(b), Path(id)).await else {
            panic!("no detail for the card we just created");
        };
        let in_the_drawer = serde_json::to_value(&detail).unwrap();
        assert_eq!(in_the_drawer["pr_url"], "https://github.com/shanemcd/honr/pull/1");
        assert_eq!(in_the_drawer["environment"], "honr-card-8-a1");
    }

    #[tokio::test]
    async fn version_reports_the_crate_version() {
        let Json(v) = version().await;
        assert_eq!(v.version, env!("CARGO_PKG_VERSION"));
        assert_eq!(
            serde_json::to_value(&v).unwrap(),
            serde_json::json!({ "version": env!("CARGO_PKG_VERSION") }),
        );
    }

    #[tokio::test]
    async fn item_detail_and_board_snapshot_include_resolved_blockers() {
        let b: SharedBoard = std::sync::Arc::new(crate::store::Board::new(
            crate::schema::Schema::default(),
            std::env::temp_dir().join("honr-test-blockers.json"),
        ));
        let project = b
            .create(None, "Proj", "why", None, crate::model::Origin::Human, true, None)
            .expect("project");
        let blocker = b
            .create(
                Some(project.id),
                "Blocker Task",
                "Must be done first",
                Some("done".into()),
                crate::model::Origin::Human,
                false,
                None,
            )
            .expect("blocker");
        let blocked = b
            .create(
                Some(project.id),
                "Blocked Task",
                "Waiting on blocker",
                Some("done".into()),
                crate::model::Origin::Human,
                false,
                None,
            )
            .expect("blocked");
        b.set_blocked_by(blocked.id, vec![blocker.id]);

        let Json(snap) = board(AxState(b.clone())).await;
        let snap_val = serde_json::to_value(&snap).unwrap();
        let blocked_item = snap_val["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["id"] == blocked.id)
            .expect("blocked item in snapshot");

        assert_eq!(blocked_item["blocked_by"], serde_json::json!([blocker.id]));
        assert_eq!(
            blocked_item["blockers"],
            serde_json::json!([
                {
                    "id": blocker.id,
                    "title": "Blocker Task",
                    "state": "draft"
                }
            ])
        );

        let Ok(Json(detail)) = item_detail(AxState(b), Path(blocked.id)).await else {
            panic!("no detail for blocked task");
        };
        let detail_val = serde_json::to_value(&detail).unwrap();
        assert_eq!(detail_val["blocked_by"], serde_json::json!([blocker.id]));
        assert_eq!(
            detail_val["blockers"],
            serde_json::json!([
                {
                    "id": blocker.id,
                    "title": "Blocker Task",
                    "state": "draft"
                }
            ])
        );
    }
}
