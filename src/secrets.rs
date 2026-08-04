//! At-rest encryption for operator secrets (OpenShell mTLS PEMs, etc.).
//!
//! Ciphertext lives in the board database. The only host file is a 32-byte
//! master key at `~/.config/honr/master.key` (mode 0600), auto-created on first
//! use. Override with `HONR_MASTER_KEY_PATH` or `HONR_MASTER_KEY` (64 hex chars)
//! for tests / alternate installs.
//!
//! GET APIs must never return decrypted PEMs — only presence flags.

use chacha20poly1305::{
    aead::{Aead, Key, KeyInit},
    ChaCha20Poly1305, Nonce,
};
use rand::RngExt;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

const KEY_LEN: usize = 32;
const NONCE_LEN: usize = 12;
/// File magic so we can change the AEAD later without guessing.
const BLOB_PREFIX: &[u8] = b"honr1";

#[derive(Debug, thiserror::Error)]
pub enum SecretsError {
    #[error("master key: {0}")]
    MasterKey(String),
    #[error("encrypt: {0}")]
    Encrypt(String),
    #[error("decrypt: {0}")]
    Decrypt(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
}

/// OpenShell gateway client mTLS material (plaintext, in memory only).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OpenShellMtlsBundle {
    pub ca_pem: String,
    pub client_cert_pem: String,
    pub client_key_pem: String,
}

impl OpenShellMtlsBundle {
    /// Soft check that pasted text looks like PEM (not a path / garbage).
    pub fn validate_pem_shape(&self) -> Result<(), SecretsError> {
        for (label, pem) in [
            ("ca", &self.ca_pem),
            ("client_cert", &self.client_cert_pem),
            ("client_key", &self.client_key_pem),
        ] {
            let t = pem.trim();
            if t.is_empty() {
                return Err(SecretsError::Encrypt(format!("{label}: empty PEM")));
            }
            if !t.contains("BEGIN") || !t.contains("END") {
                return Err(SecretsError::Encrypt(format!(
                    "{label}: expected PEM block (BEGIN … END)"
                )));
            }
        }
        Ok(())
    }
}

/// Presence flags safe to return over the API.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct OpenShellMtlsStatus {
    pub ca: bool,
    pub client_cert: bool,
    pub client_key: bool,
    pub complete: bool,
}

impl From<&OpenShellMtlsBundle> for OpenShellMtlsStatus {
    fn from(b: &OpenShellMtlsBundle) -> Self {
        let ca = !b.ca_pem.trim().is_empty();
        let client_cert = !b.client_cert_pem.trim().is_empty();
        let client_key = !b.client_key_pem.trim().is_empty();
        Self {
            ca,
            client_cert,
            client_key,
            complete: ca && client_cert && client_key,
        }
    }
}

/// Resolve the master-key path (tests: `HONR_MASTER_KEY_PATH`).
pub fn master_key_path() -> PathBuf {
    if let Ok(p) = std::env::var("HONR_MASTER_KEY_PATH") {
        let t = p.trim();
        if !t.is_empty() {
            return PathBuf::from(t);
        }
    }
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("honr")
        .join("master.key")
}

fn load_or_create_master_key(path: &Path) -> Result<[u8; KEY_LEN], SecretsError> {
    if let Ok(hex) = std::env::var("HONR_MASTER_KEY") {
        let hex = hex.trim();
        if !hex.is_empty() {
            return parse_hex_key(hex);
        }
    }
    if path.exists() {
        let raw = fs::read(path)?;
        if raw.len() != KEY_LEN {
            return Err(SecretsError::MasterKey(format!(
                "{}: expected {KEY_LEN} bytes, got {}",
                path.display(),
                raw.len()
            )));
        }
        let mut key = [0u8; KEY_LEN];
        key.copy_from_slice(&raw);
        return Ok(key);
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(parent, fs::Permissions::from_mode(0o700));
        }
    }
    let mut key = [0u8; KEY_LEN];
    rand::rng().fill(&mut key);
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        f.set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    f.write_all(&key)?;
    f.sync_all()?;
    Ok(key)
}

fn parse_hex_key(hex: &str) -> Result<[u8; KEY_LEN], SecretsError> {
    if hex.len() != KEY_LEN * 2 {
        return Err(SecretsError::MasterKey(format!(
            "HONR_MASTER_KEY: expected {} hex chars, got {}",
            KEY_LEN * 2,
            hex.len()
        )));
    }
    let mut key = [0u8; KEY_LEN];
    for i in 0..KEY_LEN {
        key[i] = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).map_err(|e| {
            SecretsError::MasterKey(format!("HONR_MASTER_KEY: invalid hex: {e}"))
        })?;
    }
    Ok(key)
}

fn cipher(key_bytes: &[u8; KEY_LEN]) -> ChaCha20Poly1305 {
    let key = Key::<ChaCha20Poly1305>::from(*key_bytes);
    ChaCha20Poly1305::new(&key)
}

/// Seal a UTF-8 JSON (or any bytes) payload → `honr1` || nonce || ciphertext, base64.
pub fn seal(plaintext: &[u8]) -> Result<String, SecretsError> {
    let key = load_or_create_master_key(&master_key_path())?;
    let cipher = cipher(&key);
    let mut nonce_bytes = [0u8; NONCE_LEN];
    rand::rng().fill(&mut nonce_bytes);
    let nonce = Nonce::try_from(nonce_bytes.as_slice())
        .map_err(|e| SecretsError::Encrypt(format!("nonce: {e}")))?;
    let ct = cipher
        .encrypt(&nonce, plaintext)
        .map_err(|e| SecretsError::Encrypt(e.to_string()))?;
    let mut out = Vec::with_capacity(BLOB_PREFIX.len() + NONCE_LEN + ct.len());
    out.extend_from_slice(BLOB_PREFIX);
    out.extend_from_slice(&nonce_bytes);
    out.extend_from_slice(&ct);
    Ok(base64::Engine::encode(
        &base64::engine::general_purpose::STANDARD,
        &out,
    ))
}

/// Open a blob produced by [`seal`].
pub fn open(sealed_b64: &str) -> Result<Vec<u8>, SecretsError> {
    let raw = base64::Engine::decode(
        &base64::engine::general_purpose::STANDARD,
        sealed_b64.trim(),
    )
    .map_err(|e| SecretsError::Decrypt(format!("base64: {e}")))?;
    if raw.len() < BLOB_PREFIX.len() + NONCE_LEN + 16 {
        return Err(SecretsError::Decrypt("blob too short".into()));
    }
    if &raw[..BLOB_PREFIX.len()] != BLOB_PREFIX {
        return Err(SecretsError::Decrypt("unknown blob version".into()));
    }
    let nonce_start = BLOB_PREFIX.len();
    let ct_start = nonce_start + NONCE_LEN;
    let nonce = Nonce::try_from(&raw[nonce_start..ct_start])
        .map_err(|e| SecretsError::Decrypt(format!("nonce: {e}")))?;
    let key = load_or_create_master_key(&master_key_path())?;
    let cipher = cipher(&key);
    cipher
        .decrypt(&nonce, &raw[ct_start..])
        .map_err(|e| SecretsError::Decrypt(e.to_string()))
}

pub fn seal_mtls(bundle: &OpenShellMtlsBundle) -> Result<String, SecretsError> {
    bundle.validate_pem_shape()?;
    let json = serde_json::to_vec(bundle)?;
    seal(&json)
}

/// Seal a string map (provider credentials / refresh material) as JSON.
pub fn seal_string_map(map: &std::collections::BTreeMap<String, String>) -> Result<String, SecretsError> {
    let json = serde_json::to_vec(map)?;
    seal(&json)
}

/// Open a blob produced by [`seal_string_map`].
pub fn open_string_map(
    sealed_b64: &str,
) -> Result<std::collections::BTreeMap<String, String>, SecretsError> {
    let plain = open(sealed_b64)?;
    Ok(serde_json::from_slice(&plain)?)
}

pub fn open_mtls(sealed_b64: &str) -> Result<OpenShellMtlsBundle, SecretsError> {
    let plain = open(sealed_b64)?;
    let bundle: OpenShellMtlsBundle = serde_json::from_slice(&plain)?;
    Ok(bundle)
}

pub fn mtls_status_from_sealed(sealed: Option<&str>) -> OpenShellMtlsStatus {
    match sealed.map(str::trim).filter(|s| !s.is_empty()) {
        None => OpenShellMtlsStatus::default(),
        Some(s) => match open_mtls(s) {
            Ok(b) => OpenShellMtlsStatus::from(&b),
            Err(_) => OpenShellMtlsStatus {
                // Sealed blob present but unreadable (wrong master key) —
                // surface as incomplete so the operator re-uploads.
                ca: false,
                client_cert: false,
                client_key: false,
                complete: false,
            },
        },
    }
}

/// OpenShell CLI config root (`$XDG_CONFIG_HOME/openshell` or `~/.config/openshell`).
///
/// Deliberately not `dirs::config_dir()` — on macOS that is Application Support,
/// while the OpenShell CLI always writes under the XDG `~/.config` layout.
fn openshell_cli_config_dir() -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        let t = xdg.trim();
        if !t.is_empty() {
            return PathBuf::from(t).join("openshell");
        }
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".config")
        .join("openshell")
}

/// Copy PEMs from a local OpenShell CLI gateway mtls directory into a bundle.
pub fn import_openshell_cli_mtls(gateway_name: &str) -> Result<OpenShellMtlsBundle, SecretsError> {
    let name = gateway_name.trim();
    let name = if name.is_empty() { "openshell" } else { name };
    let dir = openshell_cli_config_dir()
        .join("gateways")
        .join(name)
        .join("mtls");
    let ca = fs::read_to_string(dir.join("ca.crt")).map_err(|e| {
        SecretsError::Io(std::io::Error::new(
            e.kind(),
            format!("{} ({})", e, dir.join("ca.crt").display()),
        ))
    })?;
    let cert = fs::read_to_string(dir.join("tls.crt"))?;
    let key = fs::read_to_string(dir.join("tls.key"))?;
    let bundle = OpenShellMtlsBundle {
        ca_pem: ca,
        client_cert_pem: cert,
        client_key_pem: key,
    };
    bundle.validate_pem_shape()?;
    Ok(bundle)
}

/// Serialize + restore `HONR_MASTER_KEY*` across tests (process-global env).
#[cfg(test)]
pub(crate) mod master_key_env {
    use std::path::Path;
    use std::sync::{Mutex, MutexGuard};

    static LOCK: Mutex<()> = Mutex::new(());

    pub(crate) struct Guard {
        _lock: MutexGuard<'static, ()>,
        prev_path: Option<String>,
        prev_hex: Option<String>,
    }

    impl Guard {
        fn take_lock() -> MutexGuard<'static, ()> {
            LOCK.lock().unwrap_or_else(|p| p.into_inner())
        }

        fn capture() -> (Option<String>, Option<String>) {
            (
                std::env::var("HONR_MASTER_KEY_PATH").ok(),
                std::env::var("HONR_MASTER_KEY").ok(),
            )
        }

        /// Exclusive use of a file-backed master key path.
        pub(crate) fn with_key_path(path: impl AsRef<Path>) -> Self {
            let _lock = Self::take_lock();
            let (prev_path, prev_hex) = Self::capture();
            std::env::set_var("HONR_MASTER_KEY_PATH", path.as_ref());
            std::env::remove_var("HONR_MASTER_KEY");
            Self {
                _lock,
                prev_path,
                prev_hex,
            }
        }

        /// Exclusive use of `HONR_MASTER_KEY` hex (no path override).
        pub(crate) fn with_hex_key(hex: &str) -> Self {
            let _lock = Self::take_lock();
            let (prev_path, prev_hex) = Self::capture();
            std::env::remove_var("HONR_MASTER_KEY_PATH");
            std::env::set_var("HONR_MASTER_KEY", hex);
            Self {
                _lock,
                prev_path,
                prev_hex,
            }
        }
    }

    impl Drop for Guard {
        fn drop(&mut self) {
            match &self.prev_path {
                Some(p) => std::env::set_var("HONR_MASTER_KEY_PATH", p),
                None => std::env::remove_var("HONR_MASTER_KEY_PATH"),
            }
            match &self.prev_hex {
                Some(h) => std::env::set_var("HONR_MASTER_KEY", h),
                None => std::env::remove_var("HONR_MASTER_KEY"),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_bundle() -> OpenShellMtlsBundle {
        OpenShellMtlsBundle {
            ca_pem: "-----BEGIN CERTIFICATE-----\nCA\n-----END CERTIFICATE-----\n".into(),
            client_cert_pem: "-----BEGIN CERTIFICATE-----\nCERT\n-----END CERTIFICATE-----\n"
                .into(),
            client_key_pem: "-----BEGIN PRIVATE KEY-----\nKEY\n-----END PRIVATE KEY-----\n"
                .into(),
        }
    }

    #[test]
    fn seal_open_round_trip() {
        let dir = std::env::temp_dir().join(format!(
            "honr-secrets-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = fs::create_dir_all(&dir);
        let key_path = dir.join("master.key");
        let _env = master_key_env::Guard::with_key_path(&key_path);

        let bundle = sample_bundle();
        let sealed = seal_mtls(&bundle).expect("seal");
        assert!(!sealed.contains("BEGIN"));
        let opened = open_mtls(&sealed).expect("open");
        assert_eq!(opened, bundle);
        assert!(key_path.exists());

        drop(_env);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn hex_env_master_key() {
        let hex = "aa".repeat(KEY_LEN);
        let _env = master_key_env::Guard::with_hex_key(&hex);
        let sealed = seal(b"hello").expect("seal");
        let plain = open(&sealed).expect("open");
        assert_eq!(plain, b"hello");
    }

    #[test]
    fn rejects_non_pem() {
        let b = OpenShellMtlsBundle {
            ca_pem: "/tmp/ca.crt".into(),
            client_cert_pem: sample_bundle().client_cert_pem,
            client_key_pem: sample_bundle().client_key_pem,
        };
        assert!(b.validate_pem_shape().is_err());
    }

    #[test]
    fn seal_string_map_round_trip() {
        let hex = "bb".repeat(KEY_LEN);
        let _env = master_key_env::Guard::with_hex_key(&hex);
        let mut map = std::collections::BTreeMap::new();
        map.insert("GITHUB_TOKEN".into(), "ghp_secret_value".into());
        map.insert("OTHER".into(), "x".into());
        let sealed = seal_string_map(&map).expect("seal");
        assert!(!sealed.contains("ghp_secret_value"));
        assert_eq!(open_string_map(&sealed).expect("open"), map);
    }
}
