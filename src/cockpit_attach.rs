//! Host-mediated cockpit attach (real interactive TTY).
//!
//! Cockpit opens an authenticated WebSocket; we open
//! `ExecSandboxInteractive` into the Board-named cockpit sandbox and relay bytes.
//! Board `cockpit_session` stays authoritative — this module never parks, resumes,
//! or stops the session. Host `openshell sandbox connect` remains a manual CLI
//! path; honr does not shell out to launch editors.
//!
//! Attach drops into interactive Cursor `agent` (not a bare shell). When the
//! Board session has a `conversation_id`, we pass `--resume` so the TTY
//! continues that thread after the supervisor's headless scrape.

use crate::model::CockpitSessionStatus;
use crate::openshell::InteractiveEvent;
use crate::store::SharedBoard;
use crate::supervisor::{cockpit_briefing, shell_quote, stop_agent};
use crate::ws::{read_frame, write_frame, WsFrame};

use axum::extract::{Request, State as AxState};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use axum_extra::extract::cookie::CookieJar;
use hyper::upgrade;
use hyper_util::rt::TokioIo;
use serde::Deserialize;
use serde_json::json;
use std::time::Duration;

/// Where the cockpit agent works inside the sandbox (same as supervisor).
const WORKDIR: &str = "/sandbox/repo";

pub fn routes() -> Router<SharedBoard> {
    Router::new().route("/cockpit-attach", get(cockpit_attach_ws))
}

#[derive(Debug)]
struct AttachError {
    status: StatusCode,
    message: String,
}

impl AttachError {
    fn conflict(msg: impl Into<String>) -> Self {
        Self {
            status: StatusCode::CONFLICT,
            message: msg.into(),
        }
    }
}

impl IntoResponse for AttachError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(json!({ "error": self.message })),
        )
            .into_response()
    }
}

/// Read-only Board gate — Running + named environment required.
fn ready_environment(board: &SharedBoard) -> Result<String, AttachError> {
    let session = board
        .cockpit_session()
        .ok_or_else(|| AttachError::conflict("no cockpit session"))?;
    match session.status {
        CockpitSessionStatus::Parked => {
            return Err(AttachError::conflict(
                "cockpit session is parked — Stop then Start again",
            ));
        }
        CockpitSessionStatus::Running => {}
    }
    session
        .environment
        .filter(|e| !e.trim().is_empty())
        .ok_or_else(|| AttachError::conflict("cockpit session has no environment yet"))
}

/// Interactive Cursor Agent CLI argv for Cockpit attach.
///
/// Login shell so PATH finds `agent`; `exec` replaces the shell. No `--force`:
/// Cockpit is a human-in-the-loop seat, so tool calls should prompt (unlike
/// headless card workers). `--trust` / `--sandbox disabled` still apply —
/// OpenShell already contains the process. `--approve-mcps` skips the MCP
/// server approval prompt (injected `honr` is intentional for this seat).
/// `--resume` continues a Board conversation. `initial_prompt` seeds a freshly
/// minted chat (omit on reconnect).
pub(crate) fn attach_agent_command(
    conversation_id: Option<&str>,
    initial_prompt: Option<&str>,
) -> Vec<String> {
    let mut agent = String::from("agent --trust --approve-mcps --sandbox disabled");
    if let Some(cid) = conversation_id.map(str::trim).filter(|s| !s.is_empty()) {
        agent.push_str(" --resume ");
        agent.push_str(&shell_quote(cid));
    }
    if let Some(prompt) = initial_prompt.map(str::trim).filter(|s| !s.is_empty()) {
        agent.push(' ');
        agent.push_str(&shell_quote(prompt));
    }
    let script = format!("cd {WORKDIR} 2>/dev/null || cd /sandbox; exec {agent}");
    vec!["bash".into(), "-lc".into(), script]
}

fn session_conversation_id(board: &SharedBoard) -> Option<String> {
    board
        .cockpit_session()
        .and_then(|s| s.conversation_id)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Mint a chat id in the sandbox when the Board has none, and persist it.
pub(crate) fn parse_create_chat_id(stdout: &str) -> Option<String> {
    stdout
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.contains(' '))
        .map(|s| s.to_string())
}

/// Shell that mints a Cursor chat id without deadlocking.
///
/// `agent create-chat` prints an id then keeps running. Piped to `head` it often
/// fully-buffers stdout (no TTY) so `head -n1` waits forever. Write to a file,
/// poll for the first token line, then kill the process.
pub(crate) fn create_chat_script() -> String {
    format!(
        r#"cd {WORKDIR} 2>/dev/null || cd /sandbox
out=$(mktemp)
agent create-chat >"$out" 2>/dev/null &
pid=$!
id=""
for _ in $(seq 1 80); do
  if [ -s "$out" ]; then
    id=$(tr -d '\r' <"$out" | awk 'NF && $0 !~ / / {{print; exit}}')
    if [ -n "$id" ]; then
      kill "$pid" 2>/dev/null || true
      wait "$pid" 2>/dev/null || true
      printf '%s\n' "$id"
      rm -f "$out"
      exit 0
    fi
  fi
  kill -0 "$pid" 2>/dev/null || break
  sleep 0.25
done
kill "$pid" 2>/dev/null || true
wait "$pid" 2>/dev/null || true
if [ -s "$out" ]; then
  tr -d '\r' <"$out" | awk 'NF && $0 !~ / / {{print; exit}}'
fi
rm -f "$out"
exit 1
"#
    )
}

/// `(conversation_id, freshly_minted)`.
async fn ensure_conversation_id(
    board: &SharedBoard,
    os: &crate::openshell::OpenShell,
    environment: &str,
) -> Option<(String, bool)> {
    if let Some(id) = session_conversation_id(board) {
        return Some((id, false));
    }
    let out = match os
        .exec(environment, &create_chat_script(), Duration::from_secs(30))
        .await
    {
        Ok(o) => o,
        Err(e) => {
            tracing::warn!("cockpit-attach create-chat failed: {e}");
            return None;
        }
    };
    let Some(id) = parse_create_chat_id(&out.stdout) else {
        tracing::warn!(
            "cockpit-attach create-chat: no id in stdout (exit {}): {:?}",
            out.code,
            out.stdout.trim()
        );
        return None;
    };
    if let Err(e) = board.update_cockpit_session(None, Some(id.clone())) {
        tracing::warn!("cockpit-attach persist conversation_id: {e}");
    }
    Some((id, true))
}

#[derive(Debug, Deserialize)]
struct ClientCtrl {
    #[serde(rename = "type")]
    kind: String,
    cols: Option<u32>,
    rows: Option<u32>,
}

/// Authenticated WebSocket → interactive `agent` in the Board cockpit sandbox.
async fn cockpit_attach_ws(
    AxState(board): AxState<SharedBoard>,
    headers: HeaderMap,
    mut req: Request,
) -> Response {
    let environment = match ready_environment(&board) {
        Ok(e) => e,
        Err(e) => return e.into_response(),
    };

    // Capture login before upgrade — MCP inject runs after the socket is up so
    // the browser is not stuck on "connecting…" during sandbox uploads.
    let jar = CookieJar::from_headers(req.headers());
    let login = crate::auth::session_user_from_jar(&board, &jar).map(|u| u.login);

    let key = match headers.get("sec-websocket-key").and_then(|v| v.to_str().ok()) {
        Some(k) => k,
        None => {
            return (StatusCode::BAD_REQUEST, "Missing Sec-WebSocket-Key").into_response();
        }
    };
    let accept_key = crate::ws::compute_ws_accept(key);
    let on_upgrade = upgrade::on(&mut req);

    tokio::spawn(async move {
        match on_upgrade.await {
            Ok(upgraded) => {
                let io = TokioIo::new(upgraded);
                if let Err(e) = handle_attach(io, board, environment, login).await {
                    tracing::warn!("cockpit-attach session ended: {e}");
                }
            }
            Err(e) => tracing::debug!("cockpit-attach upgrade error: {e}"),
        }
    });

    Response::builder()
        .status(StatusCode::SWITCHING_PROTOCOLS)
        .header(header::UPGRADE, "websocket")
        .header(header::CONNECTION, "Upgrade")
        .header("sec-websocket-accept", accept_key)
        .body(axum::body::Body::empty())
        .unwrap_or_else(|_| {
            (StatusCode::INTERNAL_SERVER_ERROR, "Response build error").into_response()
        })
}

async fn handle_attach<S>(
    stream: S,
    board: SharedBoard,
    environment: String,
    login: Option<String>,
) -> Result<(), String>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let (mut reader, mut writer) = tokio::io::split(stream);
    let os = board.openshell_client();

    // Free leftover detached / hung create-chat / prior interactive attach so
    // the new TTY is uncontested. `stop_agent` only knows the supervisor pidfile.
    stop_agent(&os, &environment).await;
    let _ = os
        .exec(
            &environment,
            "pkill -f '/usr/local/bin/agent' 2>/dev/null || pkill -f 'cursor-agent' 2>/dev/null || true",
            Duration::from_secs(10),
        )
        .await;
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Refresh MCP inject from the browser session when present (ties tools to
    // the human). Supervisor already injected `cockpit` fallback on sandbox ready.
    if let Some(sub) = login.as_deref() {
        if let Err(e) =
            crate::cockpit_mcp::provision_cockpit_mcp(&board, &os, &environment, sub).await
        {
            tracing::warn!("cockpit-attach MCP provision: {e}");
        }
    }

    let ensured = ensure_conversation_id(&board, &os, &environment).await;
    let (conversation_id, fresh) = match &ensured {
        Some((id, fresh)) => (Some(id.as_str()), *fresh),
        None => (None, false),
    };
    let briefing = if fresh {
        Some(cockpit_briefing())
    } else {
        None
    };
    let command = attach_agent_command(conversation_id, briefing.as_deref());

    // Initial size; client sends resize ASAP after ready.
    let mut session = os
        .exec_interactive(&environment, command, 80, 24)
        .await
        .map_err(|e| e.to_string())?;

    write_frame(
        &mut writer,
        WsFrame::Text(
            json!({
                "type": "ready",
                "environment": environment,
                "resumed": conversation_id.is_some() && !fresh,
                "conversation_id": conversation_id,
            })
            .to_string(),
        ),
    )
    .await
    .map_err(|e| e.to_string())?;

    loop {
        tokio::select! {
            frame = read_frame(&mut reader) => {
                match frame {
                    Ok(Some(WsFrame::Binary(data))) => {
                        if !data.is_empty() {
                            session.write_stdin(data).await.map_err(|e| e.to_string())?;
                        }
                    }
                    Ok(Some(WsFrame::Text(text))) => {
                        if let Ok(ctrl) = serde_json::from_str::<ClientCtrl>(&text) {
                            match ctrl.kind.as_str() {
                                "resize" => {
                                    let cols = ctrl.cols.unwrap_or(80).max(1);
                                    let rows = ctrl.rows.unwrap_or(24).max(1);
                                    session.resize(cols, rows).await.map_err(|e| e.to_string())?;
                                }
                                "ping" => {
                                    write_frame(
                                        &mut writer,
                                        WsFrame::Text(r#"{"type":"pong"}"#.into()),
                                    )
                                    .await
                                    .map_err(|e| e.to_string())?;
                                }
                                _ => {}
                            }
                        }
                    }
                    Ok(Some(WsFrame::Ping(data))) => {
                        write_frame(&mut writer, WsFrame::Pong(data))
                            .await
                            .map_err(|e| e.to_string())?;
                    }
                    Ok(Some(WsFrame::Pong(_))) => {}
                    Ok(Some(WsFrame::Close)) | Ok(None) | Err(_) => break,
                }
            }
            ev = session.next_event() => {
                match ev {
                    Some(InteractiveEvent::Stdout(data)) | Some(InteractiveEvent::Stderr(data)) => {
                        if write_frame(&mut writer, WsFrame::Binary(data)).await.is_err() {
                            break;
                        }
                    }
                    Some(InteractiveEvent::Exit(code)) => {
                        let _ = write_frame(
                            &mut writer,
                            WsFrame::Text(json!({ "type": "exit", "code": code }).to_string()),
                        )
                        .await;
                        let _ = write_frame(&mut writer, WsFrame::Close).await;
                        break;
                    }
                    None => break,
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::Schema;
    use crate::store::Board;
    use axum::body::Body;
    use axum::http::Request;
    use std::sync::Arc;
    use tower_service::Service;

    fn board() -> SharedBoard {
        Arc::new(
            Board::new(
                Schema::default(),
                std::env::temp_dir().join(format!(
                    "honr-cockpit-attach-test-{}-{}.json",
                    std::process::id(),
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_nanos()
                )),
            )
            .with_buffer_capacity(8),
        )
    }

    #[test]
    fn ready_environment_requires_running_named_session() {
        let b = board();
        assert!(ready_environment(&b).is_err());

        let _ = b
            .create_cockpit_session(Some("honr-cockpit".into()), None)
            .expect("create");
        assert_eq!(ready_environment(&b).expect("env"), "honr-cockpit");

        let _ = b.park_cockpit_session().expect("park");
        let err = ready_environment(&b).unwrap_err();
        assert_eq!(err.status, StatusCode::CONFLICT);
        assert!(err.message.contains("parked"));
    }

    #[test]
    fn attach_agent_command_cold_start() {
        let cmd = attach_agent_command(None, Some("be the cockpit"));
        assert_eq!(cmd[0], "bash");
        assert_eq!(cmd[1], "-lc");
        let script = &cmd[2];
        assert!(
            script.contains("exec agent --trust --approve-mcps --sandbox disabled"),
            "{script}"
        );
        assert!(
            !script.contains("--force"),
            "Cockpit must not enable run-everything: {script}"
        );
        assert!(!script.contains("--resume"), "{script}");
        assert!(script.contains("'be the cockpit'"), "{script}");
        assert!(script.contains(WORKDIR), "{script}");
    }

    #[test]
    fn attach_agent_command_resumes_board_conversation() {
        let cmd = attach_agent_command(Some("22096329-228f-47a1-a16f-cbde6da8fe5b"), None);
        let script = &cmd[2];
        assert!(
            script.contains("--resume '22096329-228f-47a1-a16f-cbde6da8fe5b'"),
            "{script}"
        );
        assert!(
            script.contains("exec agent --trust --approve-mcps --sandbox disabled"),
            "{script}"
        );
        assert!(
            !script.contains("--force"),
            "Cockpit must not enable run-everything: {script}"
        );
        assert!(!script.contains("be the cockpit"), "{script}");
    }

    #[test]
    fn attach_agent_command_fresh_chat_seeds_briefing() {
        let cmd = attach_agent_command(Some("new-chat-id"), Some("hello seat"));
        let script = &cmd[2];
        assert!(script.contains("--resume 'new-chat-id'"), "{script}");
        assert!(script.contains("'hello seat'"), "{script}");
    }

    #[test]
    fn attach_agent_command_ignores_blank_conversation() {
        let cmd = attach_agent_command(Some("  "), None);
        assert!(!cmd[2].contains("--resume"), "{}", cmd[2]);
    }

    #[test]
    fn parse_create_chat_id_takes_first_token_line() {
        assert_eq!(
            parse_create_chat_id("22096329-228f-47a1-a16f-cbde6da8fe5b\n").as_deref(),
            Some("22096329-228f-47a1-a16f-cbde6da8fe5b")
        );
        assert_eq!(
            parse_create_chat_id("Created chat\nabc-123\n").as_deref(),
            Some("abc-123")
        );
        assert_eq!(parse_create_chat_id("hello world\n"), None);
    }

    #[test]
    fn create_chat_script_polls_file_and_kills_hanging_agent() {
        let s = create_chat_script();
        assert!(s.contains("agent create-chat"), "{s}");
        assert!(s.contains("mktemp"), "{s}");
        assert!(s.contains("kill \"$pid\""), "{s}");
        assert!(
            !s.contains("| head"),
            "piped head deadlocks when create-chat fully-buffers: {s}"
        );
    }

    #[tokio::test]
    async fn cockpit_attach_route_refuses_without_session() {
        let b = board();
        let mut app = Router::new().nest("/api", routes()).with_state(b);
        let req = Request::builder()
            .method("GET")
            .uri("/api/cockpit-attach")
            .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==")
            .header("upgrade", "websocket")
            .header("connection", "Upgrade")
            .body(Body::empty())
            .unwrap();
        let res = app.call(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::CONFLICT);
    }
}
