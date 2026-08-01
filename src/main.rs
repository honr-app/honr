//! honr — an agent orchestrator whose board is a control plane, not a report.

mod api;
mod beads;
mod events;
mod machine;
mod mcp;
mod model;
mod openshell;
mod schema;
mod sse;
mod store;
mod supervisor;

use crate::schema::Schema;
use crate::store::{Board, SharedBoard};

use axum::routing::get;
use axum::Router;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tower_http::cors::CorsLayer;
use tower_http::services::{ServeDir, ServeFile};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "honr=info,tower_http=warn".into()),
        )
        .init();

    let schema = Schema::load("honr.yaml").unwrap_or_else(|e| {
        tracing::warn!("could not read honr.yaml ({e}); falling back to defaults");
        Schema::default()
    });
    let exec_cfg = schema.execution.clone();

    let board: SharedBoard = Arc::new(Board::load_or_new(schema, PathBuf::from("honr.json")));

    // Ensure the beads graph DB exists beside the board (identity + deps) and heal placeholders.
    if let Some(beads) = board.beads.clone() {
        let b = board.clone();
        tokio::spawn(async move {
            if let Err(e) = beads.init_stealth().await {
                tracing::warn!("beads init: {e}");
            }
            b.heal_placeholder_beads_ids().await;
        });
    }

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
        .nest("/api", api::routes())
        .route("/api/events", get(sse::events))
        .route("/healthz", get(|| async { "ok" }));

    // The cockpit's door. Same process, same port, same state.
    app = app.nest_service("/mcp", mcp::service(board.clone()));

    if web_dist.exists() {
        app = app.fallback_service(
            ServeDir::new(&web_dist).fallback(ServeFile::new(web_dist.join("index.html"))),
        );
    } else {
        tracing::info!("no web/dist build — run `npm run dev` in web/ for the board UI");
    }

    let app = app
        // The Vite dev server lives on another origin.
        .layer(CorsLayer::permissive())
        .with_state(board);

    // Overridable so a scratch instance (the UI screenshot harness) can run
    // alongside the real one instead of fighting it for the port.
    let port = std::env::var("HONR_PORT").unwrap_or_else(|_| "8080".into());
    let addr = format!("127.0.0.1:{port}");
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    tracing::info!("honr listening on http://{addr}  (MCP at /mcp)");

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown(persist.clone()))
        .await?;

    // The interval flusher can be up to its own period behind. Without this,
    // whatever happened in the last half-second is simply lost on exit.
    persist.flush();
    tracing::info!("board flushed; bye");
    Ok(())
}

async fn shutdown(board: SharedBoard) {
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

    tracing::info!("shutting down");
    board.flush();
}
