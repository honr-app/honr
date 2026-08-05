import { useCallback, useEffect, useState, type ReactNode } from "react";
import { api } from "../api.js";
import type { OpenShellSettings, OpenShellStatus } from "../types.js";
import { OpenShellProvidersPanel } from "./OpenShellProviders.js";
import { SandboxesPanel } from "./OpenShellProfiles.js";

export function OpenShellPanelView({
  status,
  gatewayEndpoint,
  caPem,
  clientCertPem,
  clientKeyPem,
  mtls,
  busy,
  error,
  savedHint,
  onGatewayEndpointChange,
  onCaPemChange,
  onClientCertPemChange,
  onClientKeyPemChange,
  onRefresh,
  onSave,
  onImportCliMtls,
  onClearMtls,
  providers,
  profiles,
}: {
  status: OpenShellStatus | null;
  gatewayEndpoint: string;
  caPem: string;
  clientCertPem: string;
  clientKeyPem: string;
  mtls?: OpenShellSettings["mtls"];
  busy?: boolean;
  error?: string | null;
  savedHint?: string | null;
  onGatewayEndpointChange: (next: string) => void;
  onCaPemChange: (next: string) => void;
  onClientCertPemChange: (next: string) => void;
  onClientKeyPemChange: (next: string) => void;
  onRefresh: () => void;
  onSave: () => void;
  onImportCliMtls: () => void;
  onClearMtls: () => void;
  /** Optional providers band (live panel passes a mounted subview). */
  providers?: ReactNode;
  /** Optional profiles band (live panel passes SandboxesPanel). */
  profiles?: ReactNode;
}) {
  const healthLabel = !status
    ? "…"
    : status.healthy
      ? "Healthy"
      : "Unhealthy";
  const healthClass = !status
    ? "dim"
    : status.healthy
      ? "openshell-health-ok"
      : "openshell-health-bad";
  const mtlsLabel = mtls?.complete
    ? "Configured (encrypted in board DB)"
    : mtls?.ca || mtls?.client_cert || mtls?.client_key
      ? "Incomplete"
      : "Not configured";

  return (
    <section aria-labelledby="openshell-title" data-testid="openshell-panel">
      <h2 id="openshell-title">OpenShell</h2>
      <p className="dim">
        Gateway connectivity, providers, and sandbox profiles. Paste endpoint +
        certs (or import from the local OpenShell config dir). PEMs are sealed
        into the board database with a host master key (
        <code>~/.config/honr/master.key</code>); the API never returns private
        key material. Host Docker / Colima stay outside honr.
      </p>

      {error && <div className="err">{error}</div>}
      {savedHint && (
        <p className="dim" data-testid="openshell-saved-hint">
          {savedHint}
        </p>
      )}

      <div className="openshell-health" data-testid="openshell-health">
        <div className="openshell-health-row">
          <span className="dim">Gateway</span>
          <strong
            className={healthClass}
            data-testid="openshell-health-label"
            data-healthy={status?.healthy ? "true" : "false"}
          >
            {healthLabel}
          </strong>
        </div>
        <div className="openshell-health-row">
          <span className="dim">mTLS material</span>
          <strong data-testid="openshell-mtls-label">{mtlsLabel}</strong>
        </div>
        {status?.summary && (
          <pre className="openshell-health-summary" data-testid="openshell-health-summary">
            {status.summary}
          </pre>
        )}
        <div className="btns">
          <button
            type="button"
            className="primary"
            disabled={busy}
            onClick={onRefresh}
            data-testid="openshell-refresh"
          >
            Refresh status
          </button>
        </div>
      </div>

      <form
        className="sandbox-profile-form workspace-form"
        data-testid="openshell-gateway-form"
        onSubmit={(e) => {
          e.preventDefault();
          onSave();
        }}
      >
        <label>
          Gateway endpoint
          <input
            className="search-input"
            value={gatewayEndpoint}
            disabled={busy}
            placeholder="https://127.0.0.1:17670"
            onChange={(e) => onGatewayEndpointChange(e.target.value)}
            data-testid="openshell-field-endpoint"
          />
        </label>
        <label>
          CA certificate (PEM)
          <textarea
            className="search-input"
            rows={4}
            value={caPem}
            disabled={busy}
            placeholder={
              mtls?.ca
                ? "Configured — paste to replace"
                : "-----BEGIN CERTIFICATE-----"
            }
            onChange={(e) => onCaPemChange(e.target.value)}
            data-testid="openshell-field-ca"
          />
        </label>
        <label>
          Client certificate (PEM)
          <textarea
            className="search-input"
            rows={4}
            value={clientCertPem}
            disabled={busy}
            placeholder={
              mtls?.client_cert
                ? "Configured — paste to replace"
                : "-----BEGIN CERTIFICATE-----"
            }
            onChange={(e) => onClientCertPemChange(e.target.value)}
            data-testid="openshell-field-client-cert"
          />
        </label>
        <label>
          Client private key (PEM)
          <textarea
            className="search-input"
            rows={4}
            value={clientKeyPem}
            disabled={busy}
            placeholder={
              mtls?.client_key
                ? "Configured — paste to replace"
                : "-----BEGIN PRIVATE KEY-----"
            }
            onChange={(e) => onClientKeyPemChange(e.target.value)}
            data-testid="openshell-field-client-key"
          />
        </label>
        <div className="btns">
          <button type="submit" className="primary" disabled={busy} data-testid="openshell-save">
            Save
          </button>
          <button
            type="button"
            disabled={busy}
            onClick={onImportCliMtls}
            data-testid="openshell-import-cli-mtls"
          >
            Import from local config
          </button>
          <button
            type="button"
            disabled={busy || !mtls?.complete}
            onClick={onClearMtls}
            data-testid="openshell-clear-mtls"
          >
            Clear mTLS
          </button>
        </div>
      </form>

      {providers}

      {profiles}

      <aside className="workspace-webhook-hint" data-testid="openshell-cockpit-hint">
        <h3>Host setup</h3>
        <p className="dim">
          Role checklist: compute driver → gateway (mTLS) → providers → profiles
          (image/policy). Details in <code>docs/agents.md</code> and{" "}
          <code>docs/sandbox.md</code>.
        </p>
      </aside>
    </section>
  );
}

export function OpenShellPanel() {
  const [status, setStatus] = useState<OpenShellStatus | null>(null);
  const [gatewayEndpoint, setGatewayEndpoint] = useState("");
  const [caPem, setCaPem] = useState("");
  const [clientCertPem, setClientCertPem] = useState("");
  const [clientKeyPem, setClientKeyPem] = useState("");
  const [mtls, setMtls] = useState<OpenShellSettings["mtls"]>();
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [savedHint, setSavedHint] = useState<string | null>(null);

  const applySaved = useCallback((cfg: OpenShellSettings, st?: OpenShellStatus) => {
    setGatewayEndpoint(cfg.gateway_endpoint ?? st?.gateway_endpoint ?? "");
    setMtls(cfg.mtls ?? st?.mtls);
    setCaPem("");
    setClientCertPem("");
    setClientKeyPem("");
  }, []);

  const refresh = useCallback(() => {
    setBusy(true);
    return Promise.all([api.getOpenShellStatus(), api.getOpenShell()])
      .then(([st, cfg]: [OpenShellStatus, OpenShellSettings]) => {
        setStatus(st);
        applySaved(cfg, st);
        setError(null);
      })
      .catch((e) => setError(String(e)))
      .finally(() => {
        setBusy(false);
        setLoading(false);
      });
  }, [applySaved]);

  useEffect(() => {
    refresh();
  }, [refresh]);

  const put = (body: OpenShellSettings, hint: string) => {
    setBusy(true);
    setError(null);
    setSavedHint(null);
    api
      .putOpenShell(body)
      .then((saved) => {
        applySaved(saved);
        setSavedHint(hint);
        return api.getOpenShellStatus();
      })
      .then((st) => setStatus(st))
      .catch((e) => setError(String(e)))
      .finally(() => setBusy(false));
  };

  return (
    <OpenShellPanelView
      status={status}
      gatewayEndpoint={gatewayEndpoint}
      caPem={caPem}
      clientCertPem={clientCertPem}
      clientKeyPem={clientKeyPem}
      mtls={mtls}
      busy={busy || loading}
      error={error}
      savedHint={savedHint}
      onGatewayEndpointChange={(next) => {
        setSavedHint(null);
        setGatewayEndpoint(next);
      }}
      onCaPemChange={(next) => {
        setSavedHint(null);
        setCaPem(next);
      }}
      onClientCertPemChange={(next) => {
        setSavedHint(null);
        setClientCertPem(next);
      }}
      onClientKeyPemChange={(next) => {
        setSavedHint(null);
        setClientKeyPem(next);
      }}
      onRefresh={() => {
        setSavedHint(null);
        refresh();
      }}
      onSave={() => {
        const body: OpenShellSettings = {
          gateway_endpoint: gatewayEndpoint.trim() || null,
        };
        if (caPem.trim()) body.ca_pem = caPem;
        if (clientCertPem.trim()) body.client_cert_pem = clientCertPem;
        if (clientKeyPem.trim()) body.client_key_pem = clientKeyPem;
        put(body, "Saved. mTLS PEMs are sealed in the board database.");
      }}
      onImportCliMtls={() => {
        put(
          {
            gateway_endpoint: gatewayEndpoint.trim() || null,
            import_openshell_cli_mtls: true,
          },
          "Imported mTLS from local OpenShell config and sealed it.",
        );
      }}
      onClearMtls={() => {
        put(
          {
            gateway_endpoint: gatewayEndpoint.trim() || null,
            clear_mtls: true,
          },
          "Cleared sealed mTLS material.",
        );
      }}
      providers={<OpenShellProvidersPanel gatewayHealthy={!!status?.healthy} />}
      profiles={<SandboxesPanel />}
    />
  );
}
