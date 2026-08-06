//! Antigravity (`agy`) OpenShell provider type + host keychain bootstrap.
//!
//! The seat must never see a host OAuth file. The gateway holds the live
//! access token; the sandbox only gets an `openshell:resolve:…` placeholder
//! via provider type `antigravity` (Bearer on Cloud Code endpoints).

use crate::model::{
    OpenShellProviderDesired, ANTIGRAVITY_PROVIDER, ANTIGRAVITY_PROVIDER_TYPE_PATH,
};
use crate::openshell::OpenShell;
use crate::secrets::seal_string_map;
use crate::store::SharedBoard;

use base64::Engine;
use std::collections::BTreeMap;

/// Credential env key declared by `sandbox/openshell/antigravity.yaml`.
pub const CREDENTIAL_KEY: &str = "ANTIGRAVITY_ACCESS_TOKEN";

/// Board provider config keys (Settings → Providers → antigravity).
/// Written into seat `settings.json` by [`crate::supervisor::setup_agy_auth`].
pub const CONFIG_PROJECT: &str = "ANTIGRAVITY_GCP_PROJECT";
pub const CONFIG_LOCATION: &str = "ANTIGRAVITY_GCP_LOCATION";

/// macOS keychain service / account for the Antigravity CLI oauth blob.
const KEYCHAIN_SERVICE: &str = "gemini";
const KEYCHAIN_ACCOUNT: &str = "antigravity";
const KEYRING_PREFIX: &str = "go-keyring-base64:";

/// Import the shipped `antigravity` provider type when the gateway lacks it.
pub async fn ensure_provider_type_imported(os: &OpenShell) -> Result<(), String> {
    let profiles = os
        .list_provider_profiles()
        .await
        .map_err(|e| e.to_string())?;
    if profiles.iter().any(|p| p.id == ANTIGRAVITY_PROVIDER) {
        return Ok(());
    }
    let yaml = std::fs::read_to_string(ANTIGRAVITY_PROVIDER_TYPE_PATH).map_err(|e| {
        format!("read {}: {e}", ANTIGRAVITY_PROVIDER_TYPE_PATH)
    })?;
    match os
        .import_provider_type_yaml(ANTIGRAVITY_PROVIDER_TYPE_PATH, &yaml)
        .await
    {
        Ok(()) => Ok(()),
        // Spike / prior sync may have imported already; list can lag or omit.
        Err(e) => {
            let msg = e.to_string();
            if msg.to_ascii_lowercase().contains("already exists") {
                Ok(())
            } else {
                Err(msg)
            }
        }
    }
}

/// Read the live Antigravity access token from the host keychain (macOS).
///
/// Does not return refresh tokens — those stay on the host.
pub fn read_access_token_from_keychain() -> Result<String, String> {
    #[cfg(target_os = "macos")]
    {
        let out = std::process::Command::new("security")
            .args([
                "find-generic-password",
                "-s",
                KEYCHAIN_SERVICE,
                "-a",
                KEYCHAIN_ACCOUNT,
                "-w",
            ])
            .output()
            .map_err(|e| format!("security: {e}"))?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            return Err(format!(
                "keychain miss ({KEYCHAIN_SERVICE}/{KEYCHAIN_ACCOUNT}): {}",
                err.trim()
            ));
        }
        let raw = String::from_utf8_lossy(&out.stdout).trim().to_string();
        access_token_from_keyring_blob(&raw)
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err("Antigravity keychain bootstrap is macOS-only".into())
    }
}

/// Parse `go-keyring-base64:<json>` (or raw JSON) into `token.access_token`.
pub fn access_token_from_keyring_blob(raw: &str) -> Result<String, String> {
    let payload = if let Some(b64) = raw.strip_prefix(KEYRING_PREFIX) {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(b64.trim())
            .map_err(|e| format!("keyring base64: {e}"))?;
        String::from_utf8(bytes).map_err(|e| format!("keyring utf8: {e}"))?
    } else {
        raw.trim().to_string()
    };
    let v: serde_json::Value =
        serde_json::from_str(&payload).map_err(|e| format!("keyring json: {e}"))?;
    let token = v
        .pointer("/token/access_token")
        .and_then(|x| x.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "keyring blob missing token.access_token".to_string())?;
    Ok(token.to_string())
}

/// GCP project/location from Board provider config (never host files).
pub fn gcp_from_board(board: &SharedBoard) -> Result<(String, String), String> {
    let provider = board
        .openshell_providers()
        .into_iter()
        .find(|p| p.name == ANTIGRAVITY_PROVIDER)
        .ok_or_else(|| {
            "no Board provider `antigravity` — add it under Settings → Providers".to_string()
        })?;
    let project = provider
        .config
        .get(CONFIG_PROJECT)
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            format!("antigravity provider missing config `{CONFIG_PROJECT}` (Settings → Providers)")
        })?;
    let location = provider
        .config
        .get(CONFIG_LOCATION)
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .unwrap_or("global");
    Ok((project.to_string(), location.to_string()))
}

/// Board desired provider for type `antigravity` with sealed access token.
///
/// `config` should include [`CONFIG_PROJECT`] / [`CONFIG_LOCATION`] (API/UI).
pub fn desired_from_access_token(
    access_token: &str,
    config: BTreeMap<String, String>,
) -> Result<OpenShellProviderDesired, String> {
    let mut credentials = BTreeMap::new();
    credentials.insert(CREDENTIAL_KEY.into(), access_token.to_string());
    let credentials_sealed = seal_string_map(&credentials).map_err(|e| e.to_string())?;
    Ok(OpenShellProviderDesired {
        name: ANTIGRAVITY_PROVIDER.into(),
        provider_type: ANTIGRAVITY_PROVIDER.into(),
        config,
        credentials_sealed: Some(credentials_sealed),
        credential_keys: vec![CREDENTIAL_KEY.into()],
        refresh: None,
    }
    .normalized())
}

/// Upsert Board provider `antigravity` from the host keychain access token.
///
/// Preserves existing Board `config` (GCP project/location). Never seals the
/// refresh token. Returns true when the Board record was written.
pub fn refresh_board_credentials_from_keychain(board: &SharedBoard) -> Result<bool, String> {
    let existing = board
        .openshell_providers()
        .into_iter()
        .find(|p| p.name == ANTIGRAVITY_PROVIDER);
    let token = read_access_token_from_keychain()?;
    let config = existing.map(|p| p.config).unwrap_or_default();
    let desired = desired_from_access_token(&token, config)?;
    board.upsert_openshell_provider(desired);
    Ok(true)
}

/// Attach `antigravity` to the running cockpit sandbox when the cockpit
/// create-spec lists it.
pub async fn attach_to_running_cockpit(board: &SharedBoard) -> Result<(), String> {
    let resolved = board.resolve_cockpit_sandbox_create();
    if !resolved
        .providers
        .iter()
        .any(|n| n == ANTIGRAVITY_PROVIDER)
    {
        return Ok(());
    }
    let Some(session) = board.cockpit_session() else {
        return Ok(());
    };
    if session.status != crate::model::CockpitSessionStatus::Running {
        return Ok(());
    }
    let Some(env) = session
        .environment
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    else {
        return Ok(());
    };
    let os = board.openshell_client();
    os.attach_sandbox_provider(env, ANTIGRAVITY_PROVIDER)
        .await
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn access_token_from_keyring_blob_parses_go_keyring_prefix() {
        let inner = serde_json::json!({
            "auth_method": "oauth",
            "token": {
                "access_token": "ya29.test-token",
                "refresh_token": "must-not-surface-in-desired",
                "expiry": "2099-01-01T00:00:00Z",
                "token_type": "Bearer"
            }
        });
        let b64 = base64::engine::general_purpose::STANDARD.encode(inner.to_string());
        let raw = format!("{KEYRING_PREFIX}{b64}");
        assert_eq!(
            access_token_from_keyring_blob(&raw).expect("parse"),
            "ya29.test-token"
        );
    }

    #[test]
    fn access_token_from_keyring_blob_accepts_raw_json() {
        let raw = r#"{"token":{"access_token":"tok"}}"#;
        assert_eq!(access_token_from_keyring_blob(raw).unwrap(), "tok");
    }

    #[test]
    fn gcp_from_board_reads_provider_config() {
        use crate::store::Board;
        use std::sync::Arc;
        let path = std::env::temp_dir().join(format!(
            "honr-agy-gcp-board-{}.json",
            std::process::id()
        ));
        let board = Arc::new(Board::new(crate::schema::Schema::default(), path));
        assert!(gcp_from_board(&board).is_err());
        let mut config = BTreeMap::new();
        config.insert(CONFIG_PROJECT.into(), "proj-a".into());
        config.insert(CONFIG_LOCATION.into(), "us-central1".into());
        board.upsert_openshell_provider(
            OpenShellProviderDesired {
                name: ANTIGRAVITY_PROVIDER.into(),
                provider_type: ANTIGRAVITY_PROVIDER.into(),
                config,
                credentials_sealed: None,
                credential_keys: vec![],
                refresh: None,
            }
            .normalized(),
        );
        assert_eq!(
            gcp_from_board(&board).unwrap(),
            ("proj-a".into(), "us-central1".into())
        );
    }
}
