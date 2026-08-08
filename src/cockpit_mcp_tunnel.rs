//! Board-owned dial-in tunnel for cockpit MCP.
//!
//! OpenShell sandbox SSH (russh) supports LocalForward / `ForwardTcp`
//! (board → sandbox loopback) but **not** RemoteForward (`ssh -R`). So the
//! board cannot ask the sandbox to listen and dial out to `:8080`.
//!
//! Instead:
//! 1. In-sandbox `socat` listens on `127.0.0.1:18081` (uplink, fork) and
//!    pairs each board dial-in with an agent accept on `127.0.0.1:18080`.
//! 2. The board keeps a pool of in-process `ForwardTcp` sessions to `:18081`,
//!    each bridged to host `127.0.0.1:{HONR_PORT}` — no host socat/ssh -R.
//! 3. Agent MCP uses `http://127.0.0.1:18080/mcp` on local Docker/Podman and
//!    remote Kubernetes alike. Workers never get a tunnel.

use crate::openshell::OpenShell;
use parking_lot::Mutex;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::task::JoinHandle;

/// Fixed loopback port inside the cockpit sandbox for agent MCP clients.
pub const REMOTE_MCP_PORT: u16 = 18080;

/// Board dials this port via ForwardTcp; socat pairs it with an agent conn.
pub const UPLINK_PORT: u16 = 18081;

/// Warm ForwardTcp bridges kept ready for concurrent MCP HTTP requests.
const BRIDGE_POOL_SIZE: usize = 8;

/// JWT `aud` / agent URL when the dial-in tunnel is the path (default).
pub const DEFAULT_TUNNEL_MCP_RESOURCE: &str = "http://127.0.0.1:18080/mcp";

/// Dual-listen socat: uplink first so the board pool can pre-dial.
///
/// `reuseport` lets forked children share the agent listen port under load.
fn socat_relay_command() -> String {
    format!(
        "socat \
TCP-LISTEN:{UPLINK_PORT},fork,reuseaddr,reuseport,bind=127.0.0.1 \
TCP-LISTEN:{REMOTE_MCP_PORT},reuseaddr,reuseport,bind=127.0.0.1"
    )
}

/// True when `/proc/net/tcp{,6}` shows LISTEN on `port`.
///
/// Do not use `pgrep -f` against the socat argv: `openshell sandbox exec`
/// wraps the script in `sh -c '…socat TCP-LISTEN:…'`, so pgrep matches the
/// checker itself and we skip starting the relay.
fn port_listening_shell(port: u16) -> String {
    format!(
        "awk -v hp={hex} '$4==\"0A\" {{ n=split($2,a,\":\"); if (toupper(a[n])==hp) f=1 }} \
END {{ print f?\"LISTEN\":\"DOWN\" }}' /proc/net/tcp /proc/net/tcp6 2>/dev/null",
        hex = format!("{port:04X}")
    )
}

struct TunnelState {
    sandbox: String,
    stop: Arc<AtomicBool>,
    handle: JoinHandle<()>,
}

fn tunnel_slot() -> &'static Mutex<Option<TunnelState>> {
    static SLOT: std::sync::OnceLock<Mutex<Option<TunnelState>>> = std::sync::OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(None))
}

/// Board HTTP listen port (`HONR_PORT`, default 8080).
pub fn board_listen_port() -> u16 {
    std::env::var("HONR_PORT")
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .filter(|p| *p > 0)
        .unwrap_or(8080)
}

/// MCP resource URL minted into cockpit tokens (override with `HONR_MCP_URL`).
pub fn tunnel_mcp_resource() -> String {
    std::env::var("HONR_MCP_URL")
        .ok()
        .map(|s| s.trim().trim_end_matches('/').to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| DEFAULT_TUNNEL_MCP_RESOURCE.to_string())
}

async fn ensure_relay(os: &OpenShell, sandbox: &str) -> Result<(), String> {
    let have = os
        .exec(
            sandbox,
            "command -v socat >/dev/null && echo OK || echo MISSING_SOCAT",
            Duration::from_secs(30),
        )
        .await
        .map_err(|e| format!("check socat: {e}"))?;
    if !have.stdout.contains("OK") {
        return Err(
            "socat not found in sandbox (rebuild the cockpit image from sandbox/Containerfile)"
                .into(),
        );
    }

    let listen_uplink = port_listening_shell(UPLINK_PORT);
    let up = os
        .exec(sandbox, &listen_uplink, Duration::from_secs(30))
        .await
        .map_err(|e| format!("check MCP uplink listen: {e}"))?;
    if up.stdout.contains("LISTEN") {
        return Ok(());
    }

    // Start socat. Use `pkill -x socat` only — never `pkill -f <substring>`:
    // `bash -lc '…pkill -f relay.py…'` matches itself and kills the starter
    // (empty stdout, non-zero exit → "start MCP socat relay failed:").
    let start = format!(
        "pkill -x socat >/dev/null 2>&1 || true; \
         sleep 0.2; \
         nohup {cmd} >/tmp/honr-mcp-relay.log 2>&1 & \
         echo $! >/tmp/honr-mcp-relay.pid; \
         sleep 0.5; \
         if kill -0 \"$(cat /tmp/honr-mcp-relay.pid)\" 2>/dev/null; then echo ALIVE; else echo DEAD; cat /tmp/honr-mcp-relay.log; exit 1; fi",
        cmd = socat_relay_command(),
    );
    let out = os
        .exec(sandbox, &start, Duration::from_secs(45))
        .await
        .map_err(|e| format!("start MCP socat relay: {e}"))?;
    if !out.stdout.contains("ALIVE") {
        return Err(format!(
            "start MCP socat relay failed (code {}): {} {}",
            out.code,
            out.stdout.trim(),
            out.stderr.trim()
        ));
    }
    let up = os
        .exec(sandbox, &listen_uplink, Duration::from_secs(30))
        .await
        .map_err(|e| format!("verify MCP uplink listen: {e}"))?;
    if !up.stdout.contains("LISTEN") {
        let log = os
            .exec(
                sandbox,
                "cat /tmp/honr-mcp-relay.log 2>/dev/null || true",
                Duration::from_secs(15),
            )
            .await
            .map(|o| o.stdout)
            .unwrap_or_default();
        return Err(format!(
            "socat started but port {UPLINK_PORT} not listening: {} {}",
            up.stdout.trim(),
            log.trim()
        ));
    }
    Ok(())
}

fn shell_single_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\"'\"'"))
}

fn sandbox_gone(err: &str) -> bool {
    let e = err.to_ascii_lowercase();
    e.contains("sandbox not found") || e.contains("entity was not found")
}

async fn bridge_one(os: &OpenShell, sandbox: &str) -> Result<(), String> {
    let board_port = board_listen_port();
    let board = tokio::net::TcpStream::connect(("127.0.0.1", board_port))
        .await
        .map_err(|e| format!("connect board :{board_port}: {e}"))?;
    let session = os
        .create_ssh_session(sandbox)
        .await
        .map_err(|e| format!("create ForwardTcp session: {e}"))?;
    let result = os
        .forward_tcp_bridge(sandbox, "127.0.0.1", UPLINK_PORT, board, &session.token)
        .await;
    if let Err(e) = os.revoke_ssh_session(&session.token).await {
        tracing::debug!(error = %e, "cockpit: revoke MCP uplink session");
    }
    result.map_err(|e| e.to_string())
}

async fn pool_loop(os: OpenShell, sandbox: String, stop: Arc<AtomicBool>) {
    let mut set = tokio::task::JoinSet::new();
    let mut logged_gone = false;
    while !stop.load(Ordering::Relaxed) {
        while set.len() < BRIDGE_POOL_SIZE && !stop.load(Ordering::Relaxed) {
            let os2 = os.clone();
            let sb = sandbox.clone();
            let stop2 = stop.clone();
            set.spawn(async move {
                if stop2.load(Ordering::Relaxed) {
                    return BridgeOutcome::Stopped;
                }
                match bridge_one(&os2, &sb).await {
                    Ok(()) => BridgeOutcome::Ok,
                    Err(e) if sandbox_gone(&e) => BridgeOutcome::Gone(e),
                    Err(e) => {
                        if !stop2.load(Ordering::Relaxed) {
                            tracing::debug!(
                                sandbox = %sb,
                                error = %e,
                                "cockpit: MCP uplink bridge ended"
                            );
                            tokio::time::sleep(Duration::from_millis(400)).await;
                        }
                        BridgeOutcome::Retry
                    }
                }
            });
        }
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_millis(200)) => {
                if stop.load(Ordering::Relaxed) {
                    break;
                }
            }
            Some(joined) = set.join_next() => {
                match joined {
                    Ok(BridgeOutcome::Gone(e)) => {
                        if !logged_gone {
                            tracing::info!(
                                sandbox = %sandbox,
                                error = %e,
                                "cockpit: MCP uplink pool stopping; sandbox gone"
                            );
                            logged_gone = true;
                        }
                        stop.store(true, Ordering::Relaxed);
                        break;
                    }
                    Ok(BridgeOutcome::Stopped | BridgeOutcome::Ok | BridgeOutcome::Retry) => {}
                    Err(e) if e.is_cancelled() => {}
                    Err(e) => {
                        tracing::debug!(error = %e, "cockpit: MCP bridge task join");
                    }
                }
            }
        }
    }
    set.abort_all();
    while set.join_next().await.is_some() {}
}

enum BridgeOutcome {
    Ok,
    Retry,
    Stopped,
    Gone(String),
}

/// Ensure the dial-in tunnel is up for `sandbox`. Idempotent while the pool lives.
pub async fn ensure_cockpit_mcp_tunnel(os: &OpenShell, sandbox: &str) -> Result<(), String> {
    let sandbox = sandbox.trim();
    if sandbox.is_empty() {
        return Err("sandbox name required for MCP tunnel".into());
    }

    {
        let mut slot = tunnel_slot().lock();
        if let Some(state) = slot.as_mut() {
            if state.sandbox == sandbox && !state.handle.is_finished() {
                return Ok(());
            }
        }
    }
    stop_cockpit_mcp_tunnel(os).await;

    ensure_relay(os, sandbox).await?;

    let stop = Arc::new(AtomicBool::new(false));
    let handle = tokio::spawn(pool_loop(os.clone(), sandbox.to_string(), stop.clone()));
    // Register before readiness so Stop/park can tear the pool down (otherwise
    // an in-flight ensure orphans the JoinSet and it retries forever).
    *tunnel_slot().lock() = Some(TunnelState {
        sandbox: sandbox.to_string(),
        stop: stop.clone(),
        handle,
    });

    // Prove an uplink is paired: sandbox loopback must reach board /healthz.
    let mut ready = false;
    for _ in 0..20 {
        if tunnel_slot()
            .lock()
            .as_ref()
            .map(|s| s.handle.is_finished() || s.sandbox != sandbox)
            .unwrap_or(true)
        {
            break;
        }
        match os
            .exec(
                sandbox,
                &format!(
                    "curl -sS -o /dev/null -w '%{{http_code}}' --max-time 3 http://127.0.0.1:{REMOTE_MCP_PORT}/healthz || true"
                ),
                Duration::from_secs(20),
            )
            .await
        {
            Ok(out) if out.stdout.trim() == "200" => {
                ready = true;
                break;
            }
            Err(e) if sandbox_gone(&e.to_string()) => break,
            _ => {}
        }
        tokio::time::sleep(Duration::from_millis(400)).await;
    }
    if !ready {
        stop_cockpit_mcp_tunnel(os).await;
        return Err(format!(
            "MCP dial-in tunnel not reachable at 127.0.0.1:{REMOTE_MCP_PORT} from sandbox `{sandbox}` \
             (OpenShell ForwardTcp uplink pool failed readiness)"
        ));
    }

    tracing::info!(
        sandbox,
        agent_port = REMOTE_MCP_PORT,
        uplink_port = UPLINK_PORT,
        board_port = board_listen_port(),
        pool = BRIDGE_POOL_SIZE,
        "cockpit: MCP dial-in tunnel up"
    );
    Ok(())
}

/// Stop the uplink pool (idempotent). Leaves the in-sandbox socat relay running.
pub async fn stop_cockpit_mcp_tunnel(_os: &OpenShell) {
    let prev = tunnel_slot().lock().take();
    let Some(state) = prev else {
        return;
    };
    state.stop.store(true, Ordering::Relaxed);
    state.handle.abort();
    let _ = state.handle.await;
    tracing::info!(sandbox = %state.sandbox, "cockpit: MCP dial-in tunnel stopped");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_tunnel_resource_is_loopback_18080() {
        assert_eq!(
            DEFAULT_TUNNEL_MCP_RESOURCE,
            "http://127.0.0.1:18080/mcp"
        );
    }

    #[test]
    fn uplink_port_is_distinct_from_agent_port() {
        assert_ne!(REMOTE_MCP_PORT, UPLINK_PORT);
        assert_eq!(REMOTE_MCP_PORT, 18080);
        assert_eq!(UPLINK_PORT, 18081);
    }

    #[test]
    fn socat_relay_listens_uplink_before_agent() {
        let cmd = socat_relay_command();
        let up = cmd.find(&format!("TCP-LISTEN:{UPLINK_PORT}")).expect("uplink");
        let agent = cmd
            .find(&format!("TCP-LISTEN:{REMOTE_MCP_PORT}"))
            .expect("agent");
        assert!(up < agent, "uplink listen must come first for pre-dial pool: {cmd}");
        assert!(cmd.contains("fork"));
        assert!(cmd.contains("reuseport"));
        assert!(cmd.contains("bind=127.0.0.1"));
    }

    #[test]
    fn port_listen_check_does_not_embed_socat_argv() {
        // Guard the pgrep false-positive: checker must not look like the relay.
        let s = port_listening_shell(UPLINK_PORT);
        assert!(!s.contains("socat"));
        assert!(s.contains(&format!("{UPLINK_PORT:04X}")));
    }

    #[test]
    fn sandbox_gone_matches_openshell_not_found() {
        assert!(sandbox_gone(
            "create ForwardTcp session: openshell get sandbox: code: 'Some requested entity was not found', message: \"sandbox not found\""
        ));
        assert!(!sandbox_gone("connect board :8080: Connection refused"));
    }

    #[test]
    fn start_script_does_not_pkill_f_self() {
        // The starter is `bash -lc '<script>'`; `pkill -f needle` matches that
        // argv when needle appears in the script text.
        let start = format!(
            "pkill -x socat >/dev/null 2>&1 || true; nohup {} &",
            socat_relay_command()
        );
        assert!(!start.contains("pkill -f"));
        assert!(start.contains("pkill -x socat"));
    }

    #[test]
    fn shell_single_quote_escapes_embedded_quotes() {
        assert_eq!(shell_single_quote("a'b"), "'a'\"'\"'b'");
    }

    #[test]
    fn board_listen_port_defaults() {
        assert!(board_listen_port() > 0);
    }
}
