//! Inject MCP client config + Bearer tokens into the cockpit sandbox.
//!
//! Cockpit / supervisor mint JWTs (`honr-cockpit`) and write them under
//! `/sandbox/.honr/mcp/` so agents inside the seat can call host `/mcp`
//! without browser OAuth. Refresh tokens stay on disk in the sandbox only —
//! never in browser JS.

use crate::mcp_oauth::{self, OpsMcpTokens};
use crate::openshell::OpenShell;
use crate::store::SharedBoard;

use serde_json::json;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

pub const COCKPIT_MCP_DIR: &str = "/sandbox/.honr/mcp";

/// Fallback principal when no browser session is available (supervisor reconcile).
pub const COCKPIT_FALLBACK_SUB: &str = "cockpit";

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Msg(String),
    #[error("openshell: {0}")]
    OpenShell(#[from] crate::openshell::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

impl From<String> for Error {
    fn from(s: String) -> Self {
        Error::Msg(s)
    }
}

/// Mint tokens for `sub` and write MCP config into `sandbox`.
pub async fn provision_cockpit_mcp(
    board: &SharedBoard,
    os: &OpenShell,
    sandbox: &str,
    sub: &str,
) -> Result<OpsMcpTokens> {
    let resource = mcp_oauth::cockpit_mcp_resource();
    let tokens = mcp_oauth::mint_cockpit_seat_tokens(board, sub, &resource)?;
    // Fail closed if mint and verify disagree (keeps inject from shipping junk).
    if mcp_oauth::verify_cockpit_access_token(board, &tokens.access_token, &resource).as_deref()
        != Some(sub.trim())
    {
        return Err(Error::Msg(
            "minted cockpit access token failed resource verify".into(),
        ));
    }
    inject_cockpit_mcp(os, sandbox, &tokens).await?;
    Ok(tokens)
}

/// Write token.json, mcp.json, claude snippet, and env.sh into the sandbox.
pub async fn inject_cockpit_mcp(
    os: &OpenShell,
    sandbox: &str,
    tokens: &OpsMcpTokens,
) -> Result<()> {
    let staging = staging_dir()?;
    std::fs::create_dir_all(&staging)?;

    let token_path = staging.join("token.json");
    let mcp_path = staging.join("mcp.json");
    let claude_path = staging.join("claude_mcp.json");
    let env_path = staging.join("env.sh");

    let token_doc = json!({
        "access_token": tokens.access_token,
        "refresh_token": tokens.refresh_token,
        "expires_at": tokens.expires_at,
        "expires_in": tokens.expires_in,
        "resource": tokens.resource,
        "client_id": tokens.client_id,
        "sub": tokens.sub,
    });
    std::fs::write(
        &token_path,
        serde_json::to_vec_pretty(&token_doc).map_err(|e| Error::Msg(e.to_string()))?,
    )?;

    let mcp_doc = mcp_json_document(tokens);
    let mcp_bytes = serde_json::to_vec_pretty(&mcp_doc).map_err(|e| Error::Msg(e.to_string()))?;
    std::fs::write(&mcp_path, &mcp_bytes)?;
    // Same HTTP MCP shape for Claude Code's config reader.
    std::fs::write(&claude_path, &mcp_bytes)?;

    let env_sh = format!(
        "# honr cockpit MCP — sourced from ~/.bashrc when present\n\
         export HONR_MCP_URL={url}\n\
         export HONR_MCP_ACCESS_TOKEN={token}\n\
         export HONR_MCP_CLIENT_ID={client}\n",
        url = shell_single_quote(&tokens.resource),
        token = shell_single_quote(&tokens.access_token),
        client = shell_single_quote(&tokens.client_id),
    );
    std::fs::write(&env_path, env_sh)?;

    // Ensure destination exists; upload takes a directory.
    let mkdir = os
        .exec(
            sandbox,
            &format!("mkdir -p {COCKPIT_MCP_DIR} /sandbox/.cursor /sandbox/repo/.cursor 2>/dev/null || mkdir -p {COCKPIT_MCP_DIR}"),
            std::time::Duration::from_secs(60),
        )
        .await?;
    if !mkdir.ok() {
        return Err(Error::Msg(format!(
            "mkdir mcp dir failed: {}",
            mkdir.stderr.trim()
        )));
    }

    os.upload(sandbox, token_path.to_str().unwrap(), COCKPIT_MCP_DIR)
        .await?;
    os.upload(sandbox, mcp_path.to_str().unwrap(), COCKPIT_MCP_DIR)
        .await?;
    os.upload(sandbox, claude_path.to_str().unwrap(), COCKPIT_MCP_DIR)
        .await?;
    os.upload(sandbox, env_path.to_str().unwrap(), COCKPIT_MCP_DIR)
        .await?;

    // Project / home Cursor config copies + bashrc hook (best effort).
    let place = format!(
        r#"set -e
chmod 600 {COCKPIT_MCP_DIR}/token.json {COCKPIT_MCP_DIR}/env.sh 2>/dev/null || true
mkdir -p /sandbox/.cursor /sandbox/repo/.cursor 2>/dev/null || true
cp -f {COCKPIT_MCP_DIR}/mcp.json /sandbox/.cursor/mcp.json 2>/dev/null || true
cp -f {COCKPIT_MCP_DIR}/mcp.json /sandbox/repo/.cursor/mcp.json 2>/dev/null || true
# Login shells: export Bearer for tools that read the env.
touch /sandbox/.bashrc
if ! grep -q 'honr/mcp/env.sh' /sandbox/.bashrc 2>/dev/null; then
  printf '\n# honr cockpit MCP\n[ -f {COCKPIT_MCP_DIR}/env.sh ] && . {COCKPIT_MCP_DIR}/env.sh\n' >> /sandbox/.bashrc
fi
"#
    );
    let _ = os
        .exec(sandbox, &place, std::time::Duration::from_secs(60))
        .await?;

    let _ = std::fs::remove_dir_all(&staging);
    Ok(())
}

/// Best-effort wipe of injected MCP material (before Stop deletes the box).
pub async fn clear_cockpit_mcp(os: &OpenShell, sandbox: &str) -> Result<()> {
    let out = os
        .exec(
            sandbox,
            &format!("rm -rf {COCKPIT_MCP_DIR} /sandbox/.cursor/mcp.json /sandbox/repo/.cursor/mcp.json 2>/dev/null || true"),
            std::time::Duration::from_secs(60),
        )
        .await?;
    if !out.ok() {
        return Err(Error::Msg(format!(
            "clear mcp failed: {}",
            out.stderr.trim()
        )));
    }
    Ok(())
}

fn staging_dir() -> Result<PathBuf> {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!(
        "honr-cockpit-mcp-{}-{}",
        std::process::id(),
        nanos
    ));
    Ok(dir)
}

fn shell_single_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Build JSON bodies for tests without uploading.
pub fn mcp_json_document(tokens: &OpsMcpTokens) -> serde_json::Value {
    json!({
        "mcpServers": {
            "honr": {
                "type": "http",
                "url": tokens.resource,
                "headers": {
                    "Authorization": format!("Bearer {}", tokens.access_token)
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::openshell::{OpenShell, Output};
    use std::sync::Arc;

    #[test]
    fn mcp_json_includes_bearer_header() {
        let tokens = OpsMcpTokens {
            access_token: "tok-access".into(),
            refresh_token: "tok-refresh".into(),
            expires_in: 3600,
            expires_at: 999,
            resource: "http://host.docker.internal:8080/mcp".into(),
            client_id: mcp_oauth::COCKPIT_CLIENT_ID.into(),
            sub: "admin".into(),
        };
        let doc = mcp_json_document(&tokens);
        assert_eq!(
            doc["mcpServers"]["honr"]["headers"]["Authorization"],
            "Bearer tok-access"
        );
        assert_eq!(
            doc["mcpServers"]["honr"]["url"],
            "http://host.docker.internal:8080/mcp"
        );
    }

    #[tokio::test]
    async fn inject_cockpit_mcp_mkdir_and_uploads_via_mock() {
        let seen = Arc::new(parking_lot::Mutex::new(Vec::<Vec<String>>::new()));
        let seen_c = seen.clone();
        let os = OpenShell::mock(
            move |args| {
                seen_c.lock().push(args.to_vec());
                Output {
                    code: 0,
                    stdout: String::new(),
                    stderr: String::new(),
                }
            },
            std::time::Duration::from_secs(5),
        );
        let tokens = OpsMcpTokens {
            access_token: "a".into(),
            refresh_token: "r".into(),
            expires_in: 3600,
            expires_at: 1,
            resource: mcp_oauth::DEFAULT_COCKPIT_MCP_RESOURCE.into(),
            client_id: mcp_oauth::COCKPIT_CLIENT_ID.into(),
            sub: "cockpit".into(),
        };
        inject_cockpit_mcp(&os, "honr-cockpit", &tokens)
            .await
            .expect("inject");
        let calls = seen.lock().clone();
        assert!(
            calls.iter().any(|a| a.windows(2).any(|w| w[0] == "exec" || a.contains(&"sandbox".into()))),
            "expected sandbox cockpit: {calls:?}"
        );
        // mkdir + four uploads + place script
        let uploads = calls
            .iter()
            .filter(|a| a.iter().any(|s| s == "upload"))
            .count();
        assert!(uploads >= 4, "expected >=4 uploads, got {uploads}: {calls:?}");
    }
}
