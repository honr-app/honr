//! The human face. Thin: every handler here delegates straight to `Board`, so
//! the pixels and the agent API can't drift apart.

use crate::model::{ItemId, State, WorkItem};
use crate::store::{AncestryLine, SharedBoard};

use axum::extract::{Path, State as AxState};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

#[derive(Debug)]
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
        .route("/webhooks/github", post(github_webhook))
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
        .route(
            "/items/{id}/materialize-proposal",
            post(materialize_proposal_heal),
        )
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

/// Approve Initial plan proposal (id = Project or Initial plan Task).
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

/// Write / revise the proposal on the Initial plan card (id = Project or Initial plan).
/// Does not materialize Tasks — Approve does.
async fn save_plan(
    AxState(b): AxState<SharedBoard>,
    Path(id): Path<ItemId>,
    Json(req): Json<SavePlanReq>,
) -> ApiResult<crate::model::TaskProposal> {
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

/// Heal: create sibling Tasks from a Done card's proposal (e.g. merged before
/// materialize-on-Done was wired).
async fn materialize_proposal_heal(
    AxState(b): AxState<SharedBoard>,
    Path(id): Path<ItemId>,
) -> ApiResult<Vec<ItemId>> {
    let before: std::collections::HashSet<_> = b
        .get(id)
        .and_then(|i| i.parent)
        .map(|p| b.children_of(p))
        .unwrap_or_default()
        .into_iter()
        .collect();
    let made = b.materialize_pending_proposal(id).map_err(ApiError)?;
    if let Some(parent) = b.get(id).and_then(|i| i.parent) {
        for cid in b.children_of(parent) {
            if !before.contains(&cid) {
                b.schedule_beads_mirror(cid);
            }
        }
    }
    Ok(Json(made.into_iter().map(|i| i.id).collect()))
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

#[derive(Debug, Deserialize)]
pub struct GithubWebhookPayload {
    pub r#ref: Option<String>,
    pub after: Option<String>,
    #[serde(default)]
    pub head_commit: Option<GithubCommit>,

    pub action: Option<String>,
    #[serde(default)]
    pub pull_request: Option<GithubPullRequest>,

    #[serde(default)]
    pub repository: Option<GithubRepository>,
}

#[derive(Debug, Deserialize)]
pub struct GithubCommit {
    pub id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct GithubPullRequest {
    pub merged: Option<bool>,
    pub merge_commit_sha: Option<String>,
    pub html_url: Option<String>,
    pub number: Option<u64>,
    pub base: Option<GithubBranchRef>,
}

#[derive(Debug, Deserialize)]
pub struct GithubBranchRef {
    pub r#ref: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct GithubRepository {
    pub default_branch: Option<String>,
    pub full_name: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct WebhookResponse {
    pub status: String,
    pub main_advanced: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ref_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit_sha: Option<String>,
    /// Board cards moved to Done because their `pr_url` matched a merged PR.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub completed_item_ids: Vec<u64>,
}

fn resolve_merged_pr_url(payload: &GithubWebhookPayload) -> Option<String> {
    let pr = payload.pull_request.as_ref()?;
    if let Some(url) = pr.html_url.as_ref().map(|u| u.trim()).filter(|u| !u.is_empty()) {
        return Some(url.to_string());
    }
    let number = pr.number?;
    let full_name = payload
        .repository
        .as_ref()
        .and_then(|r| r.full_name.as_deref())
        .filter(|s| !s.is_empty())?;
    Some(format!("https://github.com/{full_name}/pull/{number}"))
}

async fn github_webhook(
    AxState(b): AxState<SharedBoard>,
    headers: HeaderMap,
    Json(payload): Json<GithubWebhookPayload>,
) -> ApiResult<WebhookResponse> {
    let event_type = headers
        .get("x-github-event")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");

    if event_type == "ping" {
        return Ok(Json(WebhookResponse {
            status: "pong".into(),
            main_advanced: false,
            ref_name: None,
            commit_sha: None,
            completed_item_ids: Vec::new(),
        }));
    }

    let default_branch = payload
        .repository
        .as_ref()
        .and_then(|r| r.default_branch.as_deref())
        .unwrap_or("main");

    let is_main_ref = |r: &str| -> bool {
        r == default_branch
            || r == "main"
            || r == format!("refs/heads/{default_branch}")
            || r == "refs/heads/main"
            || r.ends_with(&format!("/{default_branch}"))
            || r.ends_with("/main")
    };

    let is_push_main = if let Some(ref_str) = &payload.r#ref {
        is_main_ref(ref_str)
    } else {
        false
    };

    let is_pr_main_merge = if let Some(pr) = &payload.pull_request {
        let is_closed_or_merged =
            payload.action.as_deref() == Some("closed") || pr.merged == Some(true);
        let merged = pr.merged == Some(true);
        let base_is_main = pr
            .base
            .as_ref()
            .and_then(|b| b.r#ref.as_deref())
            .is_some_and(is_main_ref);
        is_closed_or_merged && merged && base_is_main
    } else {
        false
    };

    let mut completed_item_ids = Vec::new();
    if is_pr_main_merge {
        if let Some(pr_url) = resolve_merged_pr_url(&payload) {
            let number = payload
                .pull_request
                .as_ref()
                .and_then(|pr| pr.number);
            if let Some(id) = b.complete_for_merged_pr(&pr_url, number) {
                completed_item_ids.push(id);
                // Done materializes Initial plan / split proposals — push new cards.
                if let Some(parent) = b.get(id).and_then(|i| i.parent) {
                    for cid in b.children_of(parent) {
                        if b.get(cid).is_some_and(|c| {
                            !c.is_initial_plan_task() && c.github_issue_url.is_none()
                        }) {
                            b.schedule_beads_mirror(cid);
                        }
                    }
                }
            }
        }
    }

    if is_push_main || is_pr_main_merge {
        let ref_name = payload
            .r#ref
            .clone()
            .or_else(|| {
                payload
                    .pull_request
                    .as_ref()
                    .and_then(|pr| pr.base.as_ref())
                    .and_then(|b| b.r#ref.clone())
            })
            .unwrap_or_else(|| format!("refs/heads/{default_branch}"));

        let commit_sha = if is_push_main {
            payload
                .after
                .clone()
                .filter(|s| s != "0000000000000000000000000000000000000000")
                .or_else(|| payload.head_commit.as_ref().and_then(|c| c.id.clone()))
        } else {
            payload
                .pull_request
                .as_ref()
                .and_then(|pr| pr.merge_commit_sha.clone())
        };

        b.notify_main_advanced(&ref_name, commit_sha.clone());

        Ok(Json(WebhookResponse {
            status: "ok".into(),
            main_advanced: true,
            ref_name: Some(ref_name),
            commit_sha,
            completed_item_ids,
        }))
    } else {
        Ok(Json(WebhookResponse {
            status: "ignored".into(),
            main_advanced: false,
            ref_name: None,
            commit_sha: None,
            completed_item_ids,
        }))
    }
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

    #[tokio::test]
    async fn github_webhook_accepts_valid_payload_and_emits_main_advanced() {
        use crate::events::BoardEvent;

        let b: SharedBoard = std::sync::Arc::new(crate::store::Board::new(
            crate::schema::Schema::default(),
            std::env::temp_dir().join("honr-test-webhook.json"),
        ));

        let mut rx = b.subscribe();

        // 1. Push to main branch
        let push_payload = serde_json::json!({
            "ref": "refs/heads/main",
            "after": "1234567890abcdef1234567890abcdef12345678",
            "repository": {
                "default_branch": "main"
            }
        });

        let mut headers = HeaderMap::new();
        headers.insert("x-github-event", "push".parse().unwrap());

        let Json(resp) = github_webhook(
            AxState(b.clone()),
            headers.clone(),
            Json(serde_json::from_value(push_payload).unwrap()),
        )
        .await
        .expect("webhook response");

        assert_eq!(resp.status, "ok");
        assert!(resp.main_advanced);
        assert_eq!(resp.commit_sha.as_deref(), Some("1234567890abcdef1234567890abcdef12345678"));

        let event = rx.try_recv().expect("event emitted");
        match event {
            BoardEvent::MainAdvanced { seq: _, ref_name, commit_sha } => {
                assert_eq!(ref_name, "refs/heads/main");
                assert_eq!(commit_sha.as_deref(), Some("1234567890abcdef1234567890abcdef12345678"));
            }
            other => panic!("expected MainAdvanced, got {other:?}"),
        }

        // 2. PR merged into main
        let pr_payload = serde_json::json!({
            "action": "closed",
            "pull_request": {
                "merged": true,
                "merge_commit_sha": "fedcba0987654321fedcba0987654321fedcba09",
                "base": {
                    "ref": "main"
                }
            },
            "repository": {
                "default_branch": "main"
            }
        });

        let mut headers = HeaderMap::new();
        headers.insert("x-github-event", "pull_request".parse().unwrap());

        let Json(resp) = github_webhook(
            AxState(b.clone()),
            headers,
            Json(serde_json::from_value(pr_payload).unwrap()),
        )
        .await
        .expect("webhook response");

        assert_eq!(resp.status, "ok");
        assert!(resp.main_advanced);

        let event = rx.try_recv().expect("event emitted");
        match event {
            BoardEvent::MainAdvanced { seq: _, ref_name, commit_sha } => {
                assert_eq!(ref_name, "main");
                assert_eq!(commit_sha.as_deref(), Some("fedcba0987654321fedcba0987654321fedcba09"));
            }
            other => panic!("expected MainAdvanced, got {other:?}"),
        }

        // 3. Push to feature branch (filtered out, no event emitted)
        let feature_push = serde_json::json!({
            "ref": "refs/heads/feature/my-branch",
            "after": "9999999999999999999999999999999999999999",
            "repository": {
                "default_branch": "main"
            }
        });

        let mut headers = HeaderMap::new();
        headers.insert("x-github-event", "push".parse().unwrap());

        let Json(resp) = github_webhook(
            AxState(b.clone()),
            headers,
            Json(serde_json::from_value(feature_push).unwrap()),
        )
        .await
        .expect("webhook response");

        assert_eq!(resp.status, "ignored");
        assert!(!resp.main_advanced);
        assert!(rx.try_recv().is_err(), "no event should be emitted for feature branch push");

        // 4. Ping event (no event emitted)
        let ping_payload = serde_json::json!({
            "zen": "Non-blocking is better than blocking."
        });

        let mut headers = HeaderMap::new();
        headers.insert("x-github-event", "ping".parse().unwrap());

        let Json(resp) = github_webhook(
            AxState(b.clone()),
            headers,
            Json(serde_json::from_value(ping_payload).unwrap()),
        )
        .await
        .expect("webhook response");

        assert_eq!(resp.status, "pong");
        assert!(!resp.main_advanced);
        assert!(rx.try_recv().is_err(), "no event should be emitted for ping");
    }

    #[tokio::test]
    async fn github_webhook_endpoint_route_integration() {
        use tower_service::Service;

        let b: SharedBoard = std::sync::Arc::new(crate::store::Board::new(
            crate::schema::Schema::default(),
            std::env::temp_dir().join("honr-test-route.json"),
        ));

        let mut app = Router::new().nest("/api", routes()).with_state(b.clone());
        let mut rx = b.subscribe();

        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/api/webhooks/github")
            .header("content-type", "application/json")
            .header("x-github-event", "push")
            .body(axum::body::Body::from(
                serde_json::json!({
                    "ref": "refs/heads/main",
                    "after": "11223344556677889900aabbccddeeff11223344"
                })
                .to_string(),
            ))
            .unwrap();

        let response = app.call(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let event = rx.try_recv().expect("event emitted over route");
        match event {
            crate::events::BoardEvent::MainAdvanced { commit_sha, .. } => {
                assert_eq!(commit_sha.as_deref(), Some("11223344556677889900aabbccddeeff11223344"));
            }
            other => panic!("expected MainAdvanced event, got {other:?}"),
        }
    }

    fn review_card_with_pr(b: &SharedBoard, pr_url: &str) -> u64 {
        use crate::model::{Origin, State};
        let p = b
            .create(None, "Webhook Proj", "intent", None, Origin::Human, true, None)
            .unwrap();
        let t = b
            .create(
                Some(p.id),
                "Impl",
                "intent",
                Some("dod".into()),
                Origin::Human,
                false,
                None,
            )
            .unwrap();
        let _ = b.transition(t.id, State::Shaping, "human", None);
        let _ = b.transition(t.id, State::Backlog, "human", None);
        let _ = b.transition(t.id, State::Claimed, "agent", None);
        let _ = b.transition(t.id, State::Running, "agent", None);
        let _ = b.transition(t.id, State::Review, "agent", None);
        b.set_pr_url(t.id, Some(pr_url.to_string()));
        t.id
    }

    #[tokio::test]
    async fn github_webhook_merged_pr_completes_matching_review_card() {
        use crate::model::State;

        let b: SharedBoard = std::sync::Arc::new(crate::store::Board::new(
            crate::schema::Schema::default(),
            std::env::temp_dir().join(format!(
                "honr-test-webhook-complete-{}.json",
                std::process::id()
            )),
        ));
        let mut rx = b.subscribe();

        let pr_url = "https://github.com/shanemcd/honr/pull/4242";
        let id = review_card_with_pr(&b, pr_url);
        // Drain create/transition noise.
        while rx.try_recv().is_ok() {}

        let pr_payload = serde_json::json!({
            "action": "closed",
            "pull_request": {
                "merged": true,
                "html_url": pr_url,
                "number": 4242,
                "merge_commit_sha": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                "base": { "ref": "main" }
            },
            "repository": {
                "default_branch": "main",
                "full_name": "shanemcd/honr"
            }
        });

        let mut headers = HeaderMap::new();
        headers.insert("x-github-event", "pull_request".parse().unwrap());

        let Json(resp) = github_webhook(
            AxState(b.clone()),
            headers,
            Json(serde_json::from_value(pr_payload).unwrap()),
        )
        .await
        .expect("webhook response");

        assert_eq!(resp.status, "ok");
        assert!(resp.main_advanced);
        assert_eq!(resp.completed_item_ids, vec![id]);
        assert_eq!(b.get(id).unwrap().state, State::Done);

        let mut saw_main = false;
        while let Ok(ev) = rx.try_recv() {
            if matches!(ev, crate::events::BoardEvent::MainAdvanced { .. }) {
                saw_main = true;
            }
        }
        assert!(saw_main, "MainAdvanced should still fire on merge");
    }

    #[tokio::test]
    async fn github_webhook_closed_unmerged_pr_does_not_complete_card() {
        use crate::model::State;

        let b: SharedBoard = std::sync::Arc::new(crate::store::Board::new(
            crate::schema::Schema::default(),
            std::env::temp_dir().join(format!(
                "honr-test-webhook-unmerged-{}.json",
                std::process::id()
            )),
        ));
        let pr_url = "https://github.com/shanemcd/honr/pull/4243";
        let id = review_card_with_pr(&b, pr_url);

        let pr_payload = serde_json::json!({
            "action": "closed",
            "pull_request": {
                "merged": false,
                "html_url": pr_url,
                "number": 4243,
                "base": { "ref": "main" }
            },
            "repository": {
                "default_branch": "main",
                "full_name": "shanemcd/honr"
            }
        });

        let mut headers = HeaderMap::new();
        headers.insert("x-github-event", "pull_request".parse().unwrap());

        let Json(resp) = github_webhook(
            AxState(b.clone()),
            headers,
            Json(serde_json::from_value(pr_payload).unwrap()),
        )
        .await
        .expect("webhook response");

        assert_eq!(resp.status, "ignored");
        assert!(!resp.main_advanced);
        assert!(resp.completed_item_ids.is_empty());
        assert_eq!(b.get(id).unwrap().state, State::Review);
    }

    #[tokio::test]
    async fn github_webhook_merged_pr_no_matching_card_still_advances_main() {
        let b: SharedBoard = std::sync::Arc::new(crate::store::Board::new(
            crate::schema::Schema::default(),
            std::env::temp_dir().join(format!(
                "honr-test-webhook-nomatch-{}.json",
                std::process::id()
            )),
        ));
        let mut rx = b.subscribe();

        let pr_payload = serde_json::json!({
            "action": "closed",
            "pull_request": {
                "merged": true,
                "html_url": "https://github.com/shanemcd/honr/pull/99999",
                "number": 99999,
                "merge_commit_sha": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                "base": { "ref": "main" }
            },
            "repository": {
                "default_branch": "main",
                "full_name": "shanemcd/honr"
            }
        });

        let mut headers = HeaderMap::new();
        headers.insert("x-github-event", "pull_request".parse().unwrap());

        let Json(resp) = github_webhook(
            AxState(b.clone()),
            headers,
            Json(serde_json::from_value(pr_payload).unwrap()),
        )
        .await
        .expect("webhook response");

        assert_eq!(resp.status, "ok");
        assert!(resp.main_advanced);
        assert!(resp.completed_item_ids.is_empty());
        assert!(matches!(
            rx.try_recv().expect("MainAdvanced"),
            crate::events::BoardEvent::MainAdvanced { .. }
        ));
    }

    #[tokio::test]
    async fn github_webhook_merged_pr_complete_is_idempotent() {
        use crate::model::State;

        let b: SharedBoard = std::sync::Arc::new(crate::store::Board::new(
            crate::schema::Schema::default(),
            std::env::temp_dir().join(format!(
                "honr-test-webhook-idempotent-{}.json",
                std::process::id()
            )),
        ));
        let pr_url = "https://github.com/shanemcd/honr/pull/4244";
        let id = review_card_with_pr(&b, pr_url);

        let pr_payload = serde_json::json!({
            "action": "closed",
            "pull_request": {
                "merged": true,
                "number": 4244,
                "merge_commit_sha": "cccccccccccccccccccccccccccccccccccccccc",
                "base": { "ref": "main" }
            },
            "repository": {
                "default_branch": "main",
                "full_name": "shanemcd/honr"
            }
        });

        let mut headers = HeaderMap::new();
        headers.insert("x-github-event", "pull_request".parse().unwrap());

        let Json(resp1) = github_webhook(
            AxState(b.clone()),
            headers.clone(),
            Json(serde_json::from_value(pr_payload.clone()).unwrap()),
        )
        .await
        .expect("first");
        assert_eq!(resp1.completed_item_ids, vec![id]);
        assert_eq!(b.get(id).unwrap().state, State::Done);

        let Json(resp2) = github_webhook(
            AxState(b.clone()),
            headers,
            Json(serde_json::from_value(pr_payload).unwrap()),
        )
        .await
        .expect("second");
        assert!(resp2.main_advanced);
        assert!(
            resp2.completed_item_ids.is_empty(),
            "already-Done card must not re-complete"
        );
        assert_eq!(b.get(id).unwrap().state, State::Done);
    }

    #[tokio::test]
    async fn github_webhook_triggers_rebase_for_sibling_prs_in_review() {
        use crate::model::{Origin, State};

        let b: SharedBoard = std::sync::Arc::new(crate::store::Board::new(
            crate::schema::Schema::default(),
            std::env::temp_dir().join(format!(
                "honr-test-webhook-rebase-{}.json",
                std::process::id()
            )),
        ));

        let p = b
            .create(None, "Webhook Rebase Proj", "intent", None, Origin::Human, true, None)
            .unwrap();

        let t1 = b
            .create(Some(p.id), "Impl 1", "intent 1", Some("dod 1".into()), Origin::Human, false, None)
            .unwrap();
        let t2 = b
            .create(Some(p.id), "Impl 2", "intent 2", Some("dod 2".into()), Origin::Human, false, None)
            .unwrap();

        let pr1_url = "https://github.com/shanemcd/honr/pull/5001";
        let pr2_url = "https://github.com/shanemcd/honr/pull/5002";

        for (id, url) in [(t1.id, pr1_url), (t2.id, pr2_url)] {
            let _ = b.transition(id, State::Shaping, "human", None);
            let _ = b.transition(id, State::Backlog, "human", None);
            let _ = b.transition(id, State::Claimed, "agent", None);
            let _ = b.transition(id, State::Review, "agent", None);
            b.set_pr_url(id, Some(url.to_string()));
        }

        let pr_payload = serde_json::json!({
            "action": "closed",
            "pull_request": {
                "merged": true,
                "html_url": pr1_url,
                "number": 5001,
                "merge_commit_sha": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                "base": { "ref": "main" }
            },
            "repository": {
                "default_branch": "main",
                "full_name": "shanemcd/honr"
            }
        });

        let mut headers = HeaderMap::new();
        headers.insert("x-github-event", "pull_request".parse().unwrap());

        let Json(resp) = github_webhook(
            AxState(b.clone()),
            headers,
            Json(serde_json::from_value(pr_payload).unwrap()),
        )
        .await
        .expect("webhook response");

        assert_eq!(resp.completed_item_ids, vec![t1.id]);
        assert_eq!(b.get(t1.id).unwrap().state, State::Done);

        let t2_card = b.get(t2.id).unwrap();
        assert_eq!(t2_card.state, State::Review);
        assert!(t2_card.rebase_requested);
        assert!(t2_card.awaiting_dispatch);
    }
}
