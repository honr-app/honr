//! honr — an agent orchestrator whose board is a control plane, not a report.

mod api;
mod auth;
mod db;
mod events;
mod github_app;
mod github_poll;
mod machine;
mod mcp;
mod mcp_oauth;
mod model;
mod openshell;
mod ops_chat;
mod secrets;
mod schema;
mod seed_policies;
mod sse;
mod store;
mod supervisor;
mod ws;

use crate::schema::Schema;
use crate::store::{Board, SharedBoard};

use axum::middleware;
use axum::routing::get;
use axum::Router;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tower_http::cors::CorsLayer;
use tower_http::services::{ServeDir, ServeFile};

/// How long graceful shutdown waits for open connections (SSE, MCP streams)
/// before we drop them. Without a ceiling, a single Chrome EventSource holds
/// the process forever — Ctrl-C logs "shutting down" and the shell never returns.
const SHUTDOWN_DRAIN: Duration = Duration::from_secs(3);

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "honr=info,tower_http=warn".into()),
        )
        .init();

    // parking_lot deadlock_detection: poll and log holders instead of hanging
    // forever like std RwLock (the NOT LIVE freeze mode).
    std::thread::spawn(|| {
        loop {
            std::thread::sleep(Duration::from_secs(5));
            let deadlocks = parking_lot::deadlock::check_deadlock();
            if deadlocks.is_empty() {
                continue;
            }
            tracing::error!("{} deadlock(s) detected", deadlocks.len());
            for (i, threads) in deadlocks.iter().enumerate() {
                tracing::error!("deadlock #{i} ({} threads)", threads.len());
                for t in threads {
                    tracing::error!("  thread {:?}\n{:?}", t.thread_id(), t.backtrace());
                }
            }
        }
    });

    let mut schema = Schema::load("honr.yaml").unwrap_or_else(|e| {
        tracing::warn!("could not read honr.yaml ({e}); falling back to defaults");
        Schema::default()
    });
    db::apply_database_url_override(&mut schema.board.database);
    let json_path = PathBuf::from("honr.json");
    let board: SharedBoard = match schema.board.database.parsed() {
        Ok(url) => {
            tracing::info!(%url, backend = %url.backend(), "board database configured");
            let store = Arc::new(
                db::DurableBoardStore::connect(url.as_str())
                    .await
                    .map_err(|e| anyhow::anyhow!("board database open/migrate: {e}"))?,
            );
            Arc::new(
                Board::load_with_store(schema.clone(), json_path, store)
                    .await
                    .map_err(|e| anyhow::anyhow!("board load from database: {e}"))?,
            )
        }
        Err(e) => {
            tracing::warn!("board.database.url invalid ({e}); using honr.json");
            Arc::new(Board::load_or_new(schema.clone(), json_path))
        }
    };
    let exec_cfg = schema.execution.clone();

    // Persist on an interval rather than per mutation, so heartbeating agents
    // don't turn into a write storm. Paired with a flush on shutdown, or the
    // last half-second of state is lost on every exit.
    let persist = board.clone();
    {
        let board = board.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_millis(500));
            loop {
                tick.tick().await;
                board.flush();
            }
        });
    }

    supervisor::spawn(board.clone(), exec_cfg);

    let web_dist = PathBuf::from("web/dist");
    let mut app = Router::new()
        .nest("/auth", auth::routes())
        .nest("/api", api::routes())
        .route("/api/events", get(sse::events))
        .route("/api/ws", get(ws::ws_handler))
        .route("/healthz", get(|| async { "ok" }))
        .nest("/.well-known", mcp_oauth::well_known_routes())
        .nest("/oauth", mcp_oauth::oauth_routes());

    // Operator MCP: Bearer via MCP OAuth once admin exists (bootstrap stays open).
    app = app.nest(
        "/mcp",
        mcp::router(board.clone()).layer(middleware::from_fn_with_state(
            board.clone(),
            mcp_oauth::require_mcp_bearer,
        )),
    );

    if web_dist.exists() {
        app = app.fallback_service(
            ServeDir::new(&web_dist).fallback(ServeFile::new(web_dist.join("index.html"))),
        );
    } else {
        tracing::info!("no web/dist build — run `npm run dev` in web/ for the board UI");
    }

    let app = app
        .layer(middleware::from_fn_with_state(
            board.clone(),
            auth::require_session,
        ))
        // The Vite dev server lives on another origin.
        .layer(CorsLayer::permissive())
        .with_state(board);

    // Overridable so a scratch instance (the UI screenshot harness) can run
    // alongside the real one instead of fighting it for the port.
    let port = std::env::var("HONR_PORT").unwrap_or_else(|_| "8080".into());
    let addr = format!("127.0.0.1:{port}");
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    tracing::info!("honr listening on http://{addr}  (MCP at /mcp)");

    // Graceful shutdown stops accepting, then waits for in-flight connections.
    // Board SSE and MCP streams never close on their own, so we race the drain
    // against a deadline (and a second interrupt) and drop whatever remains.
    let (drain_tx, drain_rx) = tokio::sync::oneshot::channel::<()>();
    let shutting_down = persist.clone();
    let server = axum::serve(listener, app).with_graceful_shutdown(async move {
        wait_interrupt().await;
        tracing::info!("shutting down");
        shutting_down.flush();
        // Returning starts the drain. Signal the watchdog so the deadline
        // starts now, not before the interrupt.
        let _ = drain_tx.send(());
    });

    tokio::select! {
        result = server => result?,
        _ = async {
            let _ = drain_rx.await;
            tokio::select! {
                _ = tokio::time::sleep(SHUTDOWN_DRAIN) => {
                    tracing::warn!(
                        "shutdown drain timed out after {}s; dropping remaining connections",
                        SHUTDOWN_DRAIN.as_secs()
                    );
                }
                _ = tokio::signal::ctrl_c() => {
                    tracing::warn!("second interrupt; dropping remaining connections");
                }
            }
        } => {}
    }

    // The interval flusher can be up to its own period behind. Without this,
    // whatever happened in the last half-second is simply lost on exit.
    // (Also covers the force-drop path, where serve never returned cleanly.)
    persist.flush();
    tracing::info!("board flushed; bye");
    Ok(())
}

async fn wait_interrupt() {
    let ctrl_c = tokio::signal::ctrl_c();
    #[cfg(unix)]
    {
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("install SIGTERM handler");
        tokio::select! {
            _ = ctrl_c => {}
            _ = term.recv() => {}
        }
    }
    #[cfg(not(unix))]
    let _ = ctrl_c.await;
}
