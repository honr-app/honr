//! In-process OpenShell gateway client (gRPC + mTLS).
//!
//! One place that knows the gateway surface, so the supervisor never builds an
//! argv and never shells out. Certs come from the sealed Settings bundle; the
//! endpoint is board state. See `docs/sandbox.md`.
//!
//! **Everything here takes a timeout, and that is not defensive style.** Every
//! failure mode observed in phase 0 — blocked metadata server, denied egress,
//! git waiting on a credential prompt — presented as a *hang*, not an error. A
//! call without a deadline is a supervisor that stops making progress and
//! never says why.

use crate::secrets::OpenShellMtlsBundle;
use futures::StreamExt;
use openshell_core::metadata::{ObjectId, ObjectLabels, ObjectName};
use openshell_core::proto::open_shell_client::OpenShellClient;
use openshell_core::proto::datamodel::v1::{ObjectMeta, Provider};
use openshell_core::proto::{
    ConfigureProviderRefreshRequest, CreateProviderRequest, CreateSandboxRequest,
    DeleteProviderRequest, DeleteSandboxRequest, ExecSandboxEvent, ExecSandboxRequest,
    GetSandboxLogsRequest, GetSandboxRequest, HealthRequest, ListProviderProfilesRequest,
    ListProvidersRequest, ListSandboxesRequest, ProviderCredentialRefreshStrategy,
    ProviderProfile as ProtoProviderProfile, SandboxPhase, SandboxSpec as ProtoSandboxSpec,
    SandboxTemplate, ServiceStatus, UpdateProviderRequest, exec_sandbox_event,
};
use prost_types::{Struct, Value, value::Kind};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tonic::transport::{Certificate, Channel, ClientTlsConfig, Endpoint, Identity};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("openshell {op} timed out after {secs}s")]
    Timeout { op: String, secs: u64 },
    #[error("openshell {op}: {message}")]
    Failed { op: String, message: String },
    #[error("openshell not configured: {0}")]
    NotConfigured(String),
    #[error("openshell connect: {0}")]
    Connect(String),
    #[error("openshell policy: {0}")]
    Policy(String),
    #[error("openshell io: {0}")]
    Io(#[source] std::io::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

/// Outcome of a gateway health probe for Settings → OpenShell and ops surfaces.
#[derive(Debug, Clone, serde::Serialize, PartialEq, Eq)]
pub struct GatewayStatus {
    pub healthy: bool,
    /// Short human summary.
    pub summary: String,
    /// True when endpoint or mTLS material is missing (Settings incomplete).
    pub not_configured: bool,
    /// Optional detail when unhealthy.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// One sandbox, as the gateway reports it. Deliberately partial: unknown
/// fields are ignored so a gateway that grows a field doesn't break us.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct Sandbox {
    pub name: String,
    #[allow(dead_code)]
    #[serde(default)]
    pub id: Option<String>,
    #[allow(dead_code)]
    #[serde(default)]
    pub phase: Option<String>,
    #[serde(default)]
    pub labels: std::collections::BTreeMap<String, String>,
}

impl Sandbox {
    /// The work item this sandbox belongs to, from the `honr.item` label.
    pub fn item_id(&self) -> Option<u64> {
        self.labels.get(LABEL_ITEM)?.parse().ok()
    }

    /// Control-plane ops seat sandbox (`honr.ops=1`), not a card worker.
    pub fn is_ops(&self) -> bool {
        self.labels
            .get(LABEL_OPS)
            .is_some_and(|v| v == "1" || v.eq_ignore_ascii_case("true"))
    }
}

/// How a sandbox is created. Mirrors the flags proven in phase 0.
#[derive(Debug, Clone)]
pub struct SandboxSpec {
    pub name: String,
    /// OCI image reference (`honr-sandbox:latest`). Same semantics as CLI `--from`.
    pub from: String,
    /// Provider names to attach (from honr desired providers with attach=true).
    pub providers: Vec<String>,
    /// Inline OpenShell policy YAML.
    pub policy: Option<String>,
    pub env: Vec<(String, String)>,
    pub labels: Vec<(String, String)>,
    pub cpu: Option<String>,
    pub memory: Option<String>,
}

/// Gateway provider record (secrets never included — gateway omits values on list).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct GatewayProvider {
    pub name: String,
    #[serde(rename = "type")]
    pub provider_type: String,
    pub credential_keys: Vec<String>,
    pub config_keys: Vec<String>,
}

/// Provider type profile for Settings form scaffolding.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct ProviderTypeProfile {
    pub id: String,
    pub display_name: String,
    pub description: String,
    pub category: String,
    pub credential_env_vars: Vec<String>,
    pub config_keys: Vec<String>,
}

/// Refresh bootstrap applied after CreateProvider (gcloud ADC, etc.).
#[derive(Debug, Clone)]
pub struct ProviderRefreshSpec {
    pub credential_key: String,
    pub strategy: String,
    pub material: BTreeMap<String, String>,
    pub secret_material_keys: Vec<String>,
}

/// What a finished command produced.
#[derive(Debug, Clone)]
pub struct Output {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

impl Output {
    pub fn ok(&self) -> bool {
        self.code == 0
    }
}

pub const LABEL_ITEM: &str = "honr.item";
/// Marks the durable control-plane ops seat sandbox (not a card worker).
pub const LABEL_OPS: &str = "honr.ops";

#[cfg(test)]
type MockHandler = std::sync::Arc<dyn Fn(&[String]) -> Output + Send + Sync>;

#[derive(Clone)]
pub struct OpenShell {
    endpoint: Option<String>,
    mtls: Option<OpenShellMtlsBundle>,
    /// Applies to control-plane calls (create, list, delete). Exec carries its
    /// own, because an agent legitimately runs for minutes.
    default_timeout: Duration,
    /// In-process stand-in for unit tests. Receives a synthesized argv-shaped
    /// slice so existing supervisor/store mocks keep working without a gateway.
    #[cfg(test)]
    mock: Option<MockHandler>,
}

impl std::fmt::Debug for OpenShell {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenShell")
            .field("endpoint", &self.endpoint)
            .field("mtls_configured", &self.mtls.is_some())
            .field("default_timeout", &self.default_timeout)
            .finish()
    }
}

impl Default for OpenShell {
    fn default() -> Self {
        Self {
            endpoint: None,
            mtls: None,
            default_timeout: Duration::from_secs(120),
            #[cfg(test)]
            mock: None,
        }
    }
}

impl OpenShell {
    pub fn new(
        endpoint: Option<String>,
        mtls: Option<OpenShellMtlsBundle>,
        default_timeout: Duration,
    ) -> Self {
        Self {
            endpoint: endpoint
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty()),
            mtls,
            default_timeout,
            #[cfg(test)]
            mock: None,
        }
    }

    /// In-process stand-in — no network. Handler sees argv-shaped calls
    /// (`sandbox exec …`, `sandbox delete …`) matching the old CLI mock surface.
    #[cfg(test)]
    pub fn mock(
        handler: impl Fn(&[String]) -> Output + Send + Sync + 'static,
        default_timeout: Duration,
    ) -> Self {
        Self {
            endpoint: Some("mock://openshell".into()),
            mtls: None,
            default_timeout,
            mock: Some(std::sync::Arc::new(handler)),
        }
    }

    fn configured(&self) -> Result<()> {
        if self.endpoint.is_none() {
            return Err(Error::NotConfigured(
                "set gateway endpoint in Settings → OpenShell".into(),
            ));
        }
        #[cfg(test)]
        if self.mock.is_some() {
            return Ok(());
        }
        match &self.mtls {
            Some(b) if mtls_bundle_complete(b) => Ok(()),
            _ => Err(Error::NotConfigured(
                "paste or import mTLS PEMs in Settings → OpenShell".into(),
            )),
        }
    }

    async fn connect(&self) -> Result<OpenShellClient<Channel>> {
        self.configured()?;
        #[cfg(test)]
        if self.mock.is_some() {
            return Err(Error::Failed {
                op: "connect".into(),
                message: "mock client has no gRPC channel".into(),
            });
        }
        let endpoint = self.endpoint.as_deref().unwrap();
        let mtls = self.mtls.as_ref().unwrap();
        let tls = ClientTlsConfig::new()
            .ca_certificate(Certificate::from_pem(mtls.ca_pem.as_bytes()))
            .identity(Identity::from_pem(
                mtls.client_cert_pem.as_bytes(),
                mtls.client_key_pem.as_bytes(),
            ));
        let channel = Endpoint::from_shared(endpoint.to_string())
            .map_err(|e| Error::Connect(format!("invalid gateway URL: {e}")))?
            .connect_timeout(Duration::from_secs(10))
            .http2_adaptive_window(true)
            .http2_keep_alive_interval(Duration::from_secs(10))
            .keep_alive_while_idle(true)
            .tls_config(tls)
            .map_err(|e| Error::Connect(format!("tls config: {e}")))?
            .connect()
            .await
            .map_err(|e| Error::Connect(e.to_string()))?;
        Ok(OpenShellClient::new(channel))
    }

    async fn with_timeout<T, F, Fut>(&self, op: &str, timeout: Duration, f: F) -> Result<T>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<T>>,
    {
        match tokio::time::timeout(timeout, f()).await {
            Ok(r) => r,
            Err(_) => Err(Error::Timeout {
                op: op.into(),
                secs: timeout.as_secs(),
            }),
        }
    }

    // -------------------------------------------------------- the verbs

    /// Is the gateway reachable? Cheap enough to call before claiming a card,
    /// and worth it: the compute driver stops on its own.
    pub async fn healthy(&self) -> bool {
        self.gateway_status().await.healthy
    }

    /// Probe gateway Health over mTLS for Settings / ops.
    pub async fn gateway_status(&self) -> GatewayStatus {
        #[cfg(test)]
        if let Some(mock) = &self.mock {
            let out = mock(&["status".into()]);
            return if out.ok() {
                GatewayStatus {
                    healthy: true,
                    summary: if out.stdout.trim().is_empty() {
                        "Connected".into()
                    } else {
                        out.stdout.trim().chars().take(2000).collect()
                    },
                    not_configured: false,
                    error: None,
                }
            } else {
                let summary = if !out.stderr.trim().is_empty() {
                    out.stderr.trim().chars().take(2000).collect()
                } else {
                    format!("health check exited {}", out.code)
                };
                GatewayStatus {
                    healthy: false,
                    summary,
                    not_configured: false,
                    error: Some(format!("health exited {}", out.code)),
                }
            };
        }

        if let Err(e) = self.configured() {
            return GatewayStatus {
                healthy: false,
                summary: e.to_string(),
                not_configured: true,
                error: Some(e.to_string()),
            };
        }

        match self
            .with_timeout("health", Duration::from_secs(15), || async {
                let mut client = self.connect().await?;
                let resp = client
                    .health(HealthRequest {})
                    .await
                    .map_err(|e| Error::Failed {
                        op: "health".into(),
                        message: e.to_string(),
                    })?;
                Ok(resp.into_inner())
            })
            .await
        {
            Ok(h) => {
                let status = ServiceStatus::try_from(h.status).unwrap_or(ServiceStatus::Unspecified);
                let healthy = status == ServiceStatus::Healthy;
                let summary = if h.version.is_empty() {
                    format!("{status:?}")
                } else {
                    format!("{status:?} (gateway {})", h.version)
                };
                GatewayStatus {
                    healthy,
                    summary,
                    not_configured: false,
                    error: (!healthy).then(|| format!("service status {status:?}")),
                }
            }
            Err(e) => GatewayStatus {
                healthy: false,
                summary: e.to_string(),
                not_configured: false,
                error: Some(e.to_string()),
            },
        }
    }

    pub async fn list(&self) -> Result<Vec<Sandbox>> {
        #[cfg(test)]
        if let Some(mock) = &self.mock {
            let out = mock(&["sandbox".into(), "list".into(), "-o".into(), "json".into()]);
            if !out.ok() {
                return Err(Error::Failed {
                    op: "sandbox list".into(),
                    message: out.stderr.trim().to_string(),
                });
            }
            return serde_json::from_str(&out.stdout).map_err(|e| Error::Failed {
                op: "sandbox list".into(),
                message: e.to_string(),
            });
        }

        self.with_timeout("sandbox list", self.default_timeout, || async {
            let mut client = self.connect().await?;
            let resp = client
                .list_sandboxes(ListSandboxesRequest {
                    limit: 0,
                    offset: 0,
                    label_selector: String::new(),
                    workspace: String::new(),
                    all_workspaces: false,
                })
                .await
                .map_err(|e| Error::Failed {
                    op: "sandbox list".into(),
                    message: e.to_string(),
                })?;
            Ok(resp
                .into_inner()
                .sandboxes
                .into_iter()
                .map(|s| Sandbox {
                    name: s.object_name().to_string(),
                    id: {
                        let id = s.object_id();
                        if id.is_empty() {
                            None
                        } else {
                            Some(id.to_string())
                        }
                    },
                    // CLI/JSON use Ready/Error/… — not the raw prost i32 Debug ("2").
                    // Supervisor readiness polls this string.
                    phase: Some(phase_label(s.phase())),
                    labels: s
                        .object_labels()
                        .unwrap_or_default()
                        .into_iter()
                        .collect(),
                })
                .collect())
        })
        .await
    }

    /// Sandboxes this honr created, keyed by work item.
    pub async fn list_ours(&self) -> Result<Vec<Sandbox>> {
        Ok(self.list().await?.into_iter().filter(|s| s.item_id().is_some()).collect())
    }

    /// Ops-seat sandboxes (`honr.ops`), distinct from card `list_ours`.
    pub async fn list_ops(&self) -> Result<Vec<Sandbox>> {
        Ok(self.list().await?.into_iter().filter(|s| s.is_ops()).collect())
    }

    /// Create and wait until Ready. We exec into it afterwards.
    pub async fn create(&self, spec: &SandboxSpec) -> Result<()> {
        #[cfg(test)]
        if let Some(mock) = &self.mock {
            let args = mock_create_args(spec);
            let out = mock(&args);
            if !out.ok() {
                return Err(Error::Failed {
                    op: "sandbox create".into(),
                    message: out.stderr.trim().to_string(),
                });
            }
            return Ok(());
        }

        let request = build_create_request(spec)?;
        self.with_timeout("sandbox create", Duration::from_secs(300), || async {
            let mut client = self.connect().await?;
            client
                .create_sandbox(request)
                .await
                .map_err(|e| Error::Failed {
                    op: "sandbox create".into(),
                    message: e.to_string(),
                })?;
            wait_ready(&mut client, &spec.name, Duration::from_secs(240)).await
        })
        .await
    }

    pub async fn list_providers(&self) -> Result<Vec<GatewayProvider>> {
        #[cfg(test)]
        if let Some(mock) = &self.mock {
            let out = mock(&["provider".into(), "list".into(), "-o".into(), "json".into()]);
            if !out.ok() {
                return Err(Error::Failed {
                    op: "provider list".into(),
                    message: out.stderr.trim().to_string(),
                });
            }
            if out.stdout.trim().is_empty() {
                return Ok(Vec::new());
            }
            return serde_json::from_str::<Vec<GatewayProvider>>(out.stdout.trim()).map_err(|e| {
                Error::Failed {
                    op: "provider list".into(),
                    message: format!("parse mock json: {e}"),
                }
            });
        }

        self.with_timeout("provider list", self.default_timeout, || async {
            let mut client = self.connect().await?;
            let resp = client
                .list_providers(ListProvidersRequest {
                    limit: 0,
                    offset: 0,
                    workspace: String::new(),
                    all_workspaces: false,
                })
                .await
                .map_err(|e| Error::Failed {
                    op: "provider list".into(),
                    message: e.to_string(),
                })?;
            Ok(resp
                .into_inner()
                .providers
                .into_iter()
                .map(gateway_provider_from_proto)
                .collect())
        })
        .await
    }

    pub async fn create_provider(
        &self,
        name: &str,
        provider_type: &str,
        credentials: BTreeMap<String, String>,
        config: BTreeMap<String, String>,
    ) -> Result<GatewayProvider> {
        #[cfg(test)]
        if let Some(mock) = &self.mock {
            let out = mock(&[
                "provider".into(),
                "create".into(),
                "--name".into(),
                name.into(),
                "--type".into(),
                provider_type.into(),
            ]);
            if !out.ok() {
                return Err(Error::Failed {
                    op: "provider create".into(),
                    message: out.stderr.trim().to_string(),
                });
            }
            return Ok(GatewayProvider {
                name: name.to_string(),
                provider_type: provider_type.to_string(),
                credential_keys: credentials.keys().cloned().collect(),
                config_keys: config.keys().cloned().collect(),
            });
        }

        self.with_timeout("provider create", self.default_timeout, || async {
            let mut client = self.connect().await?;
            let resp = client
                .create_provider(CreateProviderRequest {
                    provider: Some(Provider {
                        metadata: Some(ObjectMeta {
                            name: name.to_string(),
                            ..Default::default()
                        }),
                        r#type: provider_type.to_string(),
                        credentials: credentials.clone().into_iter().collect(),
                        config: config.clone().into_iter().collect(),
                        credential_expires_at_ms: Default::default(),
                        profile_workspace: String::new(),
                    }),
                    workspace: String::new(),
                })
                .await
                .map_err(|e| Error::Failed {
                    op: "provider create".into(),
                    message: e.to_string(),
                })?;
            let p = resp.into_inner().provider.ok_or_else(|| Error::Failed {
                op: "provider create".into(),
                message: "empty provider in response".into(),
            })?;
            Ok(gateway_provider_from_proto(p))
        })
        .await
    }

    pub async fn update_provider(
        &self,
        name: &str,
        provider_type: &str,
        credentials: BTreeMap<String, String>,
        config: BTreeMap<String, String>,
    ) -> Result<GatewayProvider> {
        #[cfg(test)]
        if let Some(mock) = &self.mock {
            let out = mock(&[
                "provider".into(),
                "update".into(),
                name.into(),
                "--type".into(),
                provider_type.into(),
            ]);
            if !out.ok() {
                return Err(Error::Failed {
                    op: "provider update".into(),
                    message: out.stderr.trim().to_string(),
                });
            }
            return Ok(GatewayProvider {
                name: name.to_string(),
                provider_type: provider_type.to_string(),
                credential_keys: credentials.keys().cloned().collect(),
                config_keys: config.keys().cloned().collect(),
            });
        }

        self.with_timeout("provider update", self.default_timeout, || async {
            let mut client = self.connect().await?;
            let resp = client
                .update_provider(UpdateProviderRequest {
                    provider: Some(Provider {
                        metadata: Some(ObjectMeta {
                            name: name.to_string(),
                            ..Default::default()
                        }),
                        r#type: provider_type.to_string(),
                        credentials: credentials.clone().into_iter().collect(),
                        config: config.clone().into_iter().collect(),
                        credential_expires_at_ms: Default::default(),
                        profile_workspace: String::new(),
                    }),
                    credential_expires_at_ms: Default::default(),
                    workspace: String::new(),
                })
                .await
                .map_err(|e| Error::Failed {
                    op: "provider update".into(),
                    message: e.to_string(),
                })?;
            let p = resp.into_inner().provider.ok_or_else(|| Error::Failed {
                op: "provider update".into(),
                message: "empty provider in response".into(),
            })?;
            Ok(gateway_provider_from_proto(p))
        })
        .await
    }

    pub async fn delete_provider(&self, name: &str) -> Result<()> {
        #[cfg(test)]
        if let Some(mock) = &self.mock {
            let out = mock(&["provider".into(), "delete".into(), name.into()]);
            if !out.ok() {
                return Err(Error::Failed {
                    op: "provider delete".into(),
                    message: out.stderr.trim().to_string(),
                });
            }
            return Ok(());
        }

        self.with_timeout("provider delete", self.default_timeout, || async {
            let mut client = self.connect().await?;
            let _ = client
                .delete_provider(DeleteProviderRequest {
                    name: name.to_string(),
                    workspace: String::new(),
                })
                .await
                .map_err(|e| Error::Failed {
                    op: "provider delete".into(),
                    message: e.to_string(),
                })?;
            Ok(())
        })
        .await
    }

    pub async fn list_provider_profiles(&self) -> Result<Vec<ProviderTypeProfile>> {
        #[cfg(test)]
        if let Some(mock) = &self.mock {
            let out = mock(&["provider".into(), "list-profiles".into(), "-o".into(), "json".into()]);
            if !out.ok() {
                return Err(Error::Failed {
                    op: "provider list-profiles".into(),
                    message: out.stderr.trim().to_string(),
                });
            }
            if out.stdout.trim().is_empty() {
                return Ok(Vec::new());
            }
            return serde_json::from_str(out.stdout.trim()).map_err(|e| Error::Failed {
                op: "provider list-profiles".into(),
                message: format!("parse mock json: {e}"),
            });
        }

        self.with_timeout("provider list-profiles", self.default_timeout, || async {
            let mut client = self.connect().await?;
            let resp = client
                .list_provider_profiles(ListProviderProfilesRequest {
                    limit: 0,
                    offset: 0,
                    workspace: String::new(),
                })
                .await
                .map_err(|e| Error::Failed {
                    op: "provider list-profiles".into(),
                    message: e.to_string(),
                })?;
            Ok(resp
                .into_inner()
                .profiles
                .into_iter()
                .map(provider_type_profile_from_proto)
                .collect())
        })
        .await
    }

    pub async fn configure_provider_refresh(
        &self,
        provider: &str,
        refresh: &ProviderRefreshSpec,
    ) -> Result<()> {
        #[cfg(test)]
        if let Some(mock) = &self.mock {
            let out = mock(&[
                "provider".into(),
                "refresh".into(),
                "configure".into(),
                provider.into(),
            ]);
            if !out.ok() {
                return Err(Error::Failed {
                    op: "provider refresh configure".into(),
                    message: out.stderr.trim().to_string(),
                });
            }
            return Ok(());
        }

        let strategy = refresh_strategy_from_name(&refresh.strategy)?;
        self.with_timeout("provider refresh configure", self.default_timeout, || async {
            let mut client = self.connect().await?;
            let _ = client
                .configure_provider_refresh(ConfigureProviderRefreshRequest {
                    provider: provider.to_string(),
                    credential_key: refresh.credential_key.clone(),
                    strategy: strategy as i32,
                    material: refresh.material.clone().into_iter().collect(),
                    secret_material_keys: refresh.secret_material_keys.clone(),
                    expires_at_ms: None,
                    workspace: String::new(),
                })
                .await
                .map_err(|e| Error::Failed {
                    op: "provider refresh configure".into(),
                    message: e.to_string(),
                })?;
            Ok(())
        })
        .await
    }

    /// Create-or-update a provider on the gateway, then apply refresh if given.
    pub async fn apply_provider(
        &self,
        name: &str,
        provider_type: &str,
        credentials: BTreeMap<String, String>,
        config: BTreeMap<String, String>,
        refresh: Option<&ProviderRefreshSpec>,
    ) -> Result<GatewayProvider> {
        let existing = self.list_providers().await.unwrap_or_default();
        let on_gateway = existing.iter().any(|p| p.name == name);
        let gw = if on_gateway {
            self.update_provider(name, provider_type, credentials, config)
                .await?
        } else {
            self.create_provider(name, provider_type, credentials, config)
                .await?
        };
        if let Some(r) = refresh {
            self.configure_provider_refresh(name, r).await?;
        }
        Ok(gw)
    }

    pub async fn delete(&self, name: &str) -> Result<()> {
        #[cfg(test)]
        if let Some(mock) = &self.mock {
            let out = mock(&["sandbox".into(), "delete".into(), name.into()]);
            if !out.ok() {
                return Err(Error::Failed {
                    op: "sandbox delete".into(),
                    message: out.stderr.trim().to_string(),
                });
            }
            return Ok(());
        }

        self.with_timeout("sandbox delete", self.default_timeout, || async {
            let mut client = self.connect().await?;
            let _ = client
                .delete_sandbox(DeleteSandboxRequest {
                    name: name.to_string(),
                    workspace: String::new(),
                })
                .await
                .map_err(|e| Error::Failed {
                    op: "sandbox delete".into(),
                    message: e.to_string(),
                })?;
            Ok(())
        })
        .await
    }

    pub async fn upload(&self, name: &str, local: &str, dest: &str) -> Result<()> {
        #[cfg(test)]
        if let Some(mock) = &self.mock {
            let out = mock(&[
                "sandbox".into(),
                "upload".into(),
                name.into(),
                local.into(),
                dest.into(),
            ]);
            if !out.ok() {
                return Err(Error::Failed {
                    op: "sandbox upload".into(),
                    message: out.stderr.trim().to_string(),
                });
            }
            return Ok(());
        }

        let local_path = PathBuf::from(local);
        let (dest_dir, tar_name) = upload_dest_parts(&local_path, dest)?;
        let archive = build_upload_tar(&local_path, &tar_name)?;
        let script = format!(
            "mkdir -p {dest} && tar xf - -C {dest}",
            dest = shell_single_quote(&dest_dir)
        );
        let out = self
            .exec_with_stdin(name, &script, archive, self.default_timeout)
            .await?;
        if !out.ok() {
            return Err(Error::Failed {
                op: "sandbox upload".into(),
                message: out.stderr.trim().to_string(),
            });
        }
        Ok(())
    }

    /// Download a file from a sandbox to the host (verdict file protocol).
    pub async fn download(&self, name: &str, remote: &str, dest: &str) -> Result<()> {
        #[cfg(test)]
        if let Some(mock) = &self.mock {
            let out = mock(&[
                "sandbox".into(),
                "download".into(),
                name.into(),
                remote.into(),
                dest.into(),
            ]);
            if !out.ok() {
                return Err(Error::Failed {
                    op: "sandbox download".into(),
                    message: out.stderr.trim().to_string(),
                });
            }
            return Ok(());
        }

        let (parent, base) = split_sandbox_path(remote);
        let script = format!(
            "tar cf - -C {parent} {base}",
            parent = shell_single_quote(parent),
            base = shell_single_quote(base)
        );
        let out = self.exec(name, &script, self.default_timeout).await?;
        if !out.ok() {
            return Err(Error::Failed {
                op: "sandbox download".into(),
                message: out.stderr.trim().to_string(),
            });
        }
        extract_download_tar(out.stdout.as_bytes(), dest, base)?;
        Ok(())
    }

    /// Unused by the supervisor; logs are currently a human's tool.
    #[allow(dead_code)]
    pub async fn logs(&self, name: &str, tail: u32) -> Result<String> {
        #[cfg(test)]
        if let Some(mock) = &self.mock {
            let out = mock(&["logs".into(), name.into(), "-n".into(), tail.to_string()]);
            return Ok(out.stdout);
        }

        self.with_timeout("sandbox logs", self.default_timeout, || async {
            let mut client = self.connect().await?;
            let sb = get_sandbox(&mut client, name).await?;
            let resp = client
                .get_sandbox_logs(GetSandboxLogsRequest {
                    sandbox_id: sb.object_id().to_string(),
                    lines: tail,
                    since_ms: 0,
                    sources: vec![],
                    min_level: String::new(),
                    workspace: String::new(),
                })
                .await
                .map_err(|e| Error::Failed {
                    op: "sandbox logs".into(),
                    message: e.to_string(),
                })?;
            let lines = resp.into_inner().logs;
            Ok(lines
                .into_iter()
                .map(|l| l.message)
                .collect::<Vec<_>>()
                .join("\n"))
        })
        .await
    }

    /// Run a command in a sandbox and wait for it.
    pub async fn exec(&self, name: &str, script: &str, timeout: Duration) -> Result<Output> {
        self.exec_with_stdin(name, script, Vec::new(), timeout).await
    }

    async fn exec_with_stdin(
        &self,
        name: &str,
        script: &str,
        stdin: Vec<u8>,
        timeout: Duration,
    ) -> Result<Output> {
        #[cfg(test)]
        if let Some(mock) = &self.mock {
            let _ = &stdin;
            let remote = timeout.as_secs().saturating_sub(5).max(1);
            let args = [
                "sandbox".into(),
                "exec".into(),
                "-n".into(),
                name.into(),
                "--timeout".into(),
                remote.to_string(),
                "--".into(),
                "bash".into(),
                "-lc".into(),
                script.into(),
            ];
            return Ok(mock(&args));
        }

        let remote = timeout.as_secs().saturating_sub(5).max(1);
        self.with_timeout(&format!("sandbox exec {name}"), timeout, || async {
            let mut client = self.connect().await?;
            let sb = get_sandbox(&mut client, name).await?;
            let request = ExecSandboxRequest {
                sandbox_id: sb.object_id().to_string(),
                command: vec!["bash".into(), "-lc".into(), script.to_string()],
                workdir: String::new(),
                environment: Default::default(),
                timeout_seconds: u32::try_from(remote).unwrap_or(u32::MAX),
                stdin,
                tty: false,
                cols: 0,
                rows: 0,
            };
            let mut stream = client
                .exec_sandbox(request)
                .await
                .map_err(|e| Error::Failed {
                    op: "sandbox exec".into(),
                    message: e.to_string(),
                })?
                .into_inner();

            let mut stdout = Vec::new();
            let mut stderr = Vec::new();
            let mut code = -1;
            while let Some(ev) = stream.next().await {
                let ev = ev.map_err(|e| Error::Failed {
                    op: "sandbox exec".into(),
                    message: e.to_string(),
                })?;
                apply_exec_event(ev, &mut stdout, &mut stderr, &mut code);
            }
            Ok(Output {
                code,
                stdout: String::from_utf8_lossy(&stdout).into_owned(),
                stderr: String::from_utf8_lossy(&stderr).into_owned(),
            })
        })
        .await
    }

    /// Run a command and hand every stdout line to `on_line` as it arrives.
    ///
    /// This is how liveness and cost stay *observed rather than self-reported*:
    /// the supervisor watches `claude --output-format stream-json` go by and
    /// heartbeats on real activity, so a hung agent cannot claim to be fine.
    ///
    /// `on_line` is called from the read loop, so it must not block.
    pub async fn exec_streaming<F>(
        &self,
        name: &str,
        script: &str,
        timeout: Duration,
        mut on_line: F,
    ) -> Result<Output>
    where
        F: FnMut(&str) + Send,
    {
        #[cfg(test)]
        if let Some(mock) = &self.mock {
            let out = self.exec(name, script, timeout).await?;
            for line in out.stdout.lines() {
                on_line(line);
            }
            let _ = mock;
            return Ok(out);
        }

        let remote = timeout.as_secs().saturating_sub(5).max(1);
        self.with_timeout(&format!("sandbox exec {name}"), timeout, || async {
            let mut client = self.connect().await?;
            let sb = get_sandbox(&mut client, name).await?;
            let request = ExecSandboxRequest {
                sandbox_id: sb.object_id().to_string(),
                command: vec!["bash".into(), "-lc".into(), script.to_string()],
                workdir: String::new(),
                environment: Default::default(),
                timeout_seconds: u32::try_from(remote).unwrap_or(u32::MAX),
                stdin: Vec::new(),
                tty: false,
                cols: 0,
                rows: 0,
            };
            let mut stream = client
                .exec_sandbox(request)
                .await
                .map_err(|e| Error::Failed {
                    op: "sandbox exec".into(),
                    message: e.to_string(),
                })?
                .into_inner();

            let mut stdout = Vec::new();
            let mut stderr = Vec::new();
            let mut code = -1;
            let mut line_buf = String::new();
            while let Some(ev) = stream.next().await {
                let ev = ev.map_err(|e| Error::Failed {
                    op: "sandbox exec".into(),
                    message: e.to_string(),
                })?;
                if let Some(exec_sandbox_event::Payload::Stdout(chunk)) = &ev.payload {
                    stdout.extend_from_slice(&chunk.data);
                    let text = String::from_utf8_lossy(&chunk.data);
                    line_buf.push_str(&text);
                    while let Some(pos) = line_buf.find('\n') {
                        let line = line_buf[..pos].to_string();
                        line_buf.drain(..=pos);
                        on_line(&line);
                    }
                } else {
                    apply_exec_event(ev, &mut stdout, &mut stderr, &mut code);
                }
            }
            if !line_buf.is_empty() {
                on_line(&line_buf);
            }
            Ok(Output {
                code,
                stdout: String::from_utf8_lossy(&stdout).into_owned(),
                stderr: String::from_utf8_lossy(&stderr).into_owned(),
            })
        })
        .await
    }
}

fn mtls_bundle_complete(b: &OpenShellMtlsBundle) -> bool {
    !b.ca_pem.trim().is_empty()
        && !b.client_cert_pem.trim().is_empty()
        && !b.client_key_pem.trim().is_empty()
}

fn apply_exec_event(
    ev: ExecSandboxEvent,
    stdout: &mut Vec<u8>,
    stderr: &mut Vec<u8>,
    code: &mut i32,
) {
    match ev.payload {
        Some(exec_sandbox_event::Payload::Stdout(chunk)) => stdout.extend_from_slice(&chunk.data),
        Some(exec_sandbox_event::Payload::Stderr(chunk)) => stderr.extend_from_slice(&chunk.data),
        Some(exec_sandbox_event::Payload::Exit(exit)) => *code = exit.exit_code,
        None => {}
    }
}

async fn get_sandbox(
    client: &mut OpenShellClient<Channel>,
    name: &str,
) -> Result<openshell_core::proto::Sandbox> {
    let resp = client
        .get_sandbox(GetSandboxRequest {
            name: name.to_string(),
            workspace: String::new(),
        })
        .await
        .map_err(|e| Error::Failed {
            op: "get sandbox".into(),
            message: e.to_string(),
        })?;
    resp.into_inner().sandbox.ok_or_else(|| Error::Failed {
        op: "get sandbox".into(),
        message: format!("sandbox `{name}` missing from response"),
    })
}

/// Human/CLI phase names for [`Sandbox::phase`]. Must stay aligned with
/// `wait_until_sandbox_ready` (and OpenShell `sandbox list -o json`).
fn phase_label(phase: i32) -> String {
    match SandboxPhase::try_from(phase).unwrap_or(SandboxPhase::Unspecified) {
        SandboxPhase::Ready => "Ready".into(),
        SandboxPhase::Provisioning => "Provisioning".into(),
        SandboxPhase::Error => "Error".into(),
        SandboxPhase::Deleting => "Deleting".into(),
        SandboxPhase::Unknown => "Unknown".into(),
        SandboxPhase::Unspecified => "Unspecified".into(),
    }
}

async fn wait_ready(
    client: &mut OpenShellClient<Channel>,
    name: &str,
    timeout: Duration,
) -> Result<()> {
    let deadline = Instant::now() + timeout;
    let mut delay = Duration::from_millis(250);
    loop {
        let sb = get_sandbox(client, name).await?;
        let phase = SandboxPhase::try_from(sb.phase()).unwrap_or(SandboxPhase::Unspecified);
        match phase {
            SandboxPhase::Ready => return Ok(()),
            SandboxPhase::Error => {
                return Err(Error::Failed {
                    op: "sandbox create".into(),
                    message: format!("sandbox `{name}` entered error phase"),
                });
            }
            _ => {}
        }
        if Instant::now() >= deadline {
            return Err(Error::Timeout {
                op: format!("wait ready {name}"),
                secs: timeout.as_secs(),
            });
        }
        tokio::time::sleep(delay).await;
        delay = (delay * 2).min(Duration::from_secs(2));
    }
}

fn build_create_request(spec: &SandboxSpec) -> Result<CreateSandboxRequest> {
    let policy = match &spec.policy {
        Some(yaml) if !yaml.trim().is_empty() => Some(
            openshell_policy::parse_sandbox_policy(yaml)
                .map_err(|e| Error::Policy(e.to_string()))?,
        ),
        _ => None,
    };
    let resources = resource_limits(spec.cpu.as_deref(), spec.memory.as_deref())?;
    let template = Some(SandboxTemplate {
        image: spec.from.clone(),
        resources,
        ..SandboxTemplate::default()
    });
    let environment: BTreeMap<String, String> = spec.env.iter().cloned().collect();
    let labels: BTreeMap<String, String> = spec.labels.iter().cloned().collect();
    Ok(CreateSandboxRequest {
        spec: Some(ProtoSandboxSpec {
            environment: environment.into_iter().collect(),
            policy,
            providers: spec.providers.clone(),
            template,
            ..ProtoSandboxSpec::default()
        }),
        name: spec.name.clone(),
        labels: labels.into_iter().collect(),
        annotations: Default::default(),
        workspace: String::new(),
    })
}

fn resource_limits(cpu: Option<&str>, memory: Option<&str>) -> Result<Option<Struct>> {
    let mut limits = BTreeMap::new();
    if let Some(cpu) = cpu.map(str::trim).filter(|s| !s.is_empty()) {
        limits.insert(
            "cpu".into(),
            Value {
                kind: Some(Kind::StringValue(cpu.to_string())),
            },
        );
    }
    if let Some(memory) = memory.map(str::trim).filter(|s| !s.is_empty()) {
        limits.insert(
            "memory".into(),
            Value {
                kind: Some(Kind::StringValue(memory.to_string())),
            },
        );
    }
    if limits.is_empty() {
        return Ok(None);
    }
    let mut fields = BTreeMap::new();
    fields.insert(
        "limits".into(),
        Value {
            kind: Some(Kind::StructValue(Struct { fields: limits })),
        },
    );
    Ok(Some(Struct { fields }))
}

fn shell_single_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

fn gateway_provider_from_proto(p: Provider) -> GatewayProvider {
    let name = p
        .metadata
        .as_ref()
        .map(|m| m.name.clone())
        .unwrap_or_default();
    let mut credential_keys: Vec<_> = p.credentials.keys().cloned().collect();
    credential_keys.sort();
    let mut config_keys: Vec<_> = p.config.keys().cloned().collect();
    config_keys.sort();
    GatewayProvider {
        name,
        provider_type: p.r#type,
        credential_keys,
        config_keys,
    }
}

fn provider_type_profile_from_proto(p: ProtoProviderProfile) -> ProviderTypeProfile {
    let mut credential_env_vars = Vec::new();
    for cred in &p.credentials {
        for env in &cred.env_vars {
            if !env.trim().is_empty() {
                credential_env_vars.push(env.clone());
            }
        }
    }
    credential_env_vars.sort();
    credential_env_vars.dedup();
    // Profiles rarely declare config keys in proto; leave empty for freeform UI.
    ProviderTypeProfile {
        id: p.id,
        display_name: p.display_name,
        description: p.description,
        category: format!("{:?}", p.category),
        credential_env_vars,
        config_keys: Vec::new(),
    }
}

fn refresh_strategy_from_name(name: &str) -> Result<ProviderCredentialRefreshStrategy> {
    let n = name.trim().to_ascii_lowercase().replace('-', "_");
    Ok(match n.as_str() {
        "oauth2_refresh_token" | "oauth2refreshtoken" => {
            ProviderCredentialRefreshStrategy::Oauth2RefreshToken
        }
        "google_service_account_jwt" | "googleserviceaccountjwt" => {
            ProviderCredentialRefreshStrategy::GoogleServiceAccountJwt
        }
        "static" => ProviderCredentialRefreshStrategy::Static,
        "external" => ProviderCredentialRefreshStrategy::External,
        "oauth2_client_credentials" => {
            ProviderCredentialRefreshStrategy::Oauth2ClientCredentials
        }
        "aws_sts_assume_role" => ProviderCredentialRefreshStrategy::AwsStsAssumeRole,
        other => {
            return Err(Error::Failed {
                op: "provider refresh".into(),
                message: format!("unknown refresh strategy {other:?}"),
            });
        }
    })
}

fn split_sandbox_path(path: &str) -> (&str, &str) {
    match path.rfind('/') {
        Some(0) => ("/", &path[1..]),
        Some(pos) => (&path[..pos], &path[pos + 1..]),
        None => (".", path),
    }
}

fn upload_dest_parts(local: &Path, dest: &str) -> Result<(String, String)> {
    // Dest is always a directory (OpenShell CLI / docs). Treating paths like
    // `/sandbox/.honr` as a *file* named `.honr` wrote report.schema.json on
    // top of the verdict dir and broke escalate/report.
    let tar_name = local
        .file_name()
        .ok_or_else(|| Error::Failed {
            op: "sandbox upload".into(),
            message: format!("path has no file name: {}", local.display()),
        })?
        .to_string_lossy()
        .into_owned();
    let dest_dir = dest.trim_end_matches('/').to_string();
    if dest_dir.is_empty() {
        return Err(Error::Failed {
            op: "sandbox upload".into(),
            message: "destination directory is empty".into(),
        });
    }
    Ok((dest_dir, tar_name))
}

fn build_upload_tar(local: &Path, tar_name: &str) -> Result<Vec<u8>> {
    let mut buf = Vec::new();
    {
        let mut ar = tar::Builder::new(&mut buf);
        if local.is_dir() {
            ar.append_dir_all(tar_name, local).map_err(Error::Io)?;
        } else {
            let mut file = std::fs::File::open(local).map_err(Error::Io)?;
            ar.append_file(tar_name, &mut file).map_err(Error::Io)?;
        }
        ar.finish().map_err(Error::Io)?;
    }
    Ok(buf)
}

fn extract_download_tar(bytes: &[u8], dest: &str, expected_base: &str) -> Result<()> {
    let dest_path = PathBuf::from(dest);
    if let Some(parent) = dest_path.parent() {
        std::fs::create_dir_all(parent).map_err(Error::Io)?;
    }
    let mut ar = tar::Archive::new(std::io::Cursor::new(bytes));
    let mut wrote = false;
    for entry in ar.entries().map_err(Error::Io)? {
        let mut entry = entry.map_err(Error::Io)?;
        let path = entry.path().map_err(Error::Io)?.into_owned();
        let name = path
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        if name != expected_base && path.to_string_lossy() != expected_base {
            continue;
        }
        if dest_path
            .parent()
            .map(|p| p.exists() && p.is_dir() && dest_path.extension().is_none() && !dest.ends_with('/'))
            .unwrap_or(false)
            && dest_path.is_dir()
        {
            let target = dest_path.join(&name);
            let mut out = std::fs::File::create(&target).map_err(Error::Io)?;
            std::io::copy(&mut entry, &mut out).map_err(Error::Io)?;
        } else {
            let mut out = std::fs::File::create(&dest_path).map_err(Error::Io)?;
            std::io::copy(&mut entry, &mut out).map_err(Error::Io)?;
            out.flush().map_err(Error::Io)?;
        }
        wrote = true;
        break;
    }
    if !wrote {
        // Fallback: treat stdout as raw file bytes (cat semantics).
        std::fs::write(&dest_path, bytes).map_err(Error::Io)?;
    }
    Ok(())
}

/// Argv-shaped create call for unit-test mocks (image flag remains `--from`).
#[cfg(test)]
fn mock_create_args(spec: &SandboxSpec) -> Vec<String> {
    let mut args: Vec<String> = vec![
        "sandbox".into(),
        "create".into(),
        "--name".into(),
        spec.name.clone(),
        "--from".into(),
        spec.from.clone(),
        "--no-tty".into(),
    ];
    for p in &spec.providers {
        args.push("--provider".into());
        args.push(p.clone());
    }
    if let Some(policy) = &spec.policy {
        args.push("--policy".into());
        args.push(policy.clone());
    }
    for (k, v) in &spec.env {
        args.push("--env".into());
        args.push(format!("{k}={v}"));
    }
    for (k, v) in &spec.labels {
        args.push("--label".into());
        args.push(format!("{k}={v}"));
    }
    if let Some(cpu) = &spec.cpu {
        args.push("--cpu".into());
        args.push(cpu.clone());
    }
    if let Some(mem) = &spec.memory {
        args.push("--memory".into());
        args.push(mem.clone());
    }
    args
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> SandboxSpec {
        SandboxSpec {
            name: "honr-card-7".into(),
            from: "honr-sandbox:latest".into(),
            providers: vec!["vertex".into(), "gh-clankr".into()],
            policy: Some("version: 1\nfilesystem_policy:\n  include_workdir: true\n".into()),
            env: vec![("DISABLE_TELEMETRY".into(), "1".into())],
            labels: vec![(LABEL_ITEM.into(), "7".into())],
            cpu: Some("2".into()),
            memory: Some("4Gi".into()),
        }
    }

    #[tokio::test]
    async fn gateway_status_healthy_when_mock_status_ok() {
        let os = OpenShell::mock(
            |args| {
                assert_eq!(args, &["status".to_string()]);
                Output {
                    code: 0,
                    stdout: "Connected\nAuthenticated (mTLS transport)\n".into(),
                    stderr: String::new(),
                }
            },
            Duration::from_secs(5),
        );
        let st = os.gateway_status().await;
        assert!(st.healthy);
        assert!(!st.not_configured);
        assert!(st.summary.contains("Connected"));
        assert!(os.healthy().await);
    }

    #[tokio::test]
    async fn gateway_status_unhealthy_when_mock_fails() {
        let os = OpenShell::mock(
            |_| Output {
                code: 1,
                stdout: String::new(),
                stderr: "gateway unreachable".into(),
            },
            Duration::from_secs(5),
        );
        let st = os.gateway_status().await;
        assert!(!st.healthy);
        assert!(!st.not_configured);
        assert!(st.summary.contains("gateway unreachable"));
        assert!(!os.healthy().await);
    }

    #[tokio::test]
    async fn gateway_status_not_configured_without_endpoint() {
        let os = OpenShell::new(None, None, Duration::from_secs(5));
        let st = os.gateway_status().await;
        assert!(!st.healthy);
        assert!(st.not_configured, "summary={}", st.summary);
        assert!(st.summary.contains("endpoint") || st.summary.contains("not configured"));
        assert!(!os.healthy().await);
    }

    /// The image flag is `--from` in the mock argv surface (and in CreateSandbox
    /// template.image for the real client). Getting this wrong used to yield a
    /// confusing registry lookup.
    #[test]
    fn image_is_passed_as_from() {
        let args = mock_create_args(&spec());
        assert!(args.windows(2).any(|w| w[0] == "--from" && w[1] == "honr-sandbox:latest"));
        assert!(!args.iter().any(|a| a == "--image"));
    }

    #[test]
    fn create_request_sets_image_providers_labels_and_policy() {
        let req = build_create_request(&spec()).expect("policy parses");
        assert_eq!(req.name, "honr-card-7");
        assert_eq!(req.labels.get("honr.item").map(String::as_str), Some("7"));
        let sandbox_spec = req.spec.expect("spec");
        assert_eq!(sandbox_spec.providers, vec!["vertex", "gh-clankr"]);
        assert_eq!(
            sandbox_spec.template.as_ref().map(|t| t.image.as_str()),
            Some("honr-sandbox:latest")
        );
        assert!(sandbox_spec.policy.is_some());
        assert_eq!(
            sandbox_spec.environment.get("DISABLE_TELEMETRY").map(String::as_str),
            Some("1")
        );
    }

    #[tokio::test]
    async fn create_passes_inline_policy_yaml_to_mock() {
        let yaml = "version: 1\nfilesystem_policy:\n  include_workdir: true\n";
        let seen = std::sync::Arc::new(parking_lot::Mutex::new(None::<String>));
        let seen_c = seen.clone();
        let os = OpenShell::mock(
            move |args| {
                let i = args.iter().position(|a| a == "--policy").expect("--policy");
                *seen_c.lock() = Some(args[i + 1].clone());
                Output {
                    code: 0,
                    stdout: String::new(),
                    stderr: String::new(),
                }
            },
            Duration::from_secs(5),
        );
        let mut s = spec();
        s.policy = Some(yaml.into());
        os.create(&s).await.expect("create");
        assert_eq!(seen.lock().as_deref(), Some(yaml));
    }

    #[tokio::test]
    async fn unconfigured_client_is_not_healthy() {
        assert!(!OpenShell::default().healthy().await);
    }

    #[test]
    fn phase_label_matches_cli_json_not_prost_debug() {
        // Ready is protobuf value 2 — Debug would be "2", which broke readiness polls.
        assert_eq!(phase_label(SandboxPhase::Ready as i32), "Ready");
        assert_eq!(phase_label(SandboxPhase::Error as i32), "Error");
        assert_eq!(phase_label(SandboxPhase::Deleting as i32), "Deleting");
        assert_eq!(phase_label(99), "Unspecified");
    }

    #[test]
    fn upload_dest_is_always_a_directory() {
        // Regression: `/sandbox/.honr` used to be treated as a file named `.honr`.
        let (dir, name) = upload_dest_parts(
            Path::new("docs/schemas/report.schema.json"),
            "/sandbox/.honr",
        )
        .expect("parts");
        assert_eq!(dir, "/sandbox/.honr");
        assert_eq!(name, "report.schema.json");
        let (dir2, name2) =
            upload_dest_parts(Path::new("sandbox/metadata-shim.py"), "/tmp").expect("shim");
        assert_eq!(dir2, "/tmp");
        assert_eq!(name2, "metadata-shim.py");
    }

    // ---- gateway-backed. `cargo test -- --ignored` with gateway + Settings mTLS.
    #[tokio::test]
    #[ignore = "needs a running OpenShell gateway with Settings mTLS configured"]
    async fn real_gateway_health_and_list() {
        let endpoint = std::env::var("HONR_OPENSHELL_ENDPOINT")
            .unwrap_or_else(|_| "https://127.0.0.1:17670".into());
        let bundle = crate::secrets::import_openshell_cli_mtls("openshell")
            .expect("import local OpenShell mTLS bundle");
        let os = OpenShell::new(Some(endpoint), Some(bundle), Duration::from_secs(30));
        assert!(os.healthy().await);
        os.list().await.expect("sandbox list");
    }
}
