//! Antigravity (`agy`) OpenShell provider type.
//!
//! The seat must never see a host OAuth file. The gateway holds the live
//! access token; the sandbox only gets an `openshell:resolve:…` placeholder
//! via provider type `antigravity` (Bearer on Cloud Code endpoints).
//!
//! The token arrives over the API like every other credential. honr does not
//! read the host keychain: reaching into a developer's credential store is a
//! guess about the machine honr happens to be running on, and it silently
//! adopted tokens that were put there for something else entirely.

use crate::model::{ANTIGRAVITY_PROVIDER, ANTIGRAVITY_PROVIDER_TYPE_NAME};
use crate::openshell::OpenShell;
use crate::store::SharedBoard;

/// Compiled in rather than read from `sandbox/openshell/…` at runtime: a
/// relative path only resolves when honr happens to be run from the repo root.
const PROVIDER_TYPE_YAML: &str = include_str!("../sandbox/openshell/antigravity.yaml");

/// Board provider config keys (Settings → Providers → antigravity).
/// Written into seat `settings.json` by [`crate::supervisor::setup_agy_auth`].
pub const CONFIG_PROJECT: &str = "ANTIGRAVITY_GCP_PROJECT";
pub const CONFIG_LOCATION: &str = "ANTIGRAVITY_GCP_LOCATION";

/// Import the shipped `antigravity` provider type when the gateway lacks it.
pub async fn ensure_provider_type_imported(os: &OpenShell) -> Result<(), String> {
    let profiles = os
        .list_provider_profiles()
        .await
        .map_err(|e| e.to_string())?;
    if profiles.iter().any(|p| p.id == ANTIGRAVITY_PROVIDER) {
        return Ok(());
    }
    match os
        .import_provider_type_yaml(ANTIGRAVITY_PROVIDER_TYPE_NAME, PROVIDER_TYPE_YAML)
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
    use crate::model::OpenShellProviderDesired;
    use std::collections::BTreeMap;

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
