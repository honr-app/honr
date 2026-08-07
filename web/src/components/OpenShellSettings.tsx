import { useCallback, useEffect, useState, type ReactNode } from "react";
import { api } from "../api.js";
import type { OpenShellSettings, OpenShellStatus } from "../types.js";
import { OpenShellPoliciesPanel } from "./OpenShellPolicies.js";
import { OpenShellProvidersPanel } from "./OpenShellProviders.js";
import { OpenShellProviderTypesPanel } from "./OpenShellProviderTypes.js";
import { SandboxesPanel } from "./OpenShellProfiles.js";

export type OpenShellTab =
  | "connectivity"
  | "providers"
  | "provider-types"
  | "policies"
  | "profiles";

const TABS: { id: OpenShellTab; label: string }[] = [
  { id: "connectivity", label: "Connectivity" },
  { id: "providers", label: "Providers" },
  { id: "provider-types", label: "Provider types" },
  { id: "policies", label: "Policies" },
  { id: "profiles", label: "Sandbox specs" },
];

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
  activeTab: activeTabProp,
  onTabChange,
  onGatewayEndpointChange,
  onCaPemChange,
  onClientCertPemChange,
  onClientKeyPemChange,
  onRefresh,
  onSave,
  onClearMtls,
  providers,
  providerTypes,
  policies,
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
  /** Controlled tab (tests). Uncontrolled when omitted. */
  activeTab?: OpenShellTab;
  onTabChange?: (tab: OpenShellTab) => void;
  onGatewayEndpointChange: (next: string) => void;
  onCaPemChange: (next: string) => void;
  onClientCertPemChange: (next: string) => void;
  onClientKeyPemChange: (next: string) => void;
  onRefresh: () => void;
  onSave: () => void;
  onClearMtls: () => void;
  providers?: ReactNode;
  providerTypes?: ReactNode;
  policies?: ReactNode;
  profiles?: ReactNode;
}) {
  const [internalTab, setInternalTab] = useState<OpenShellTab>("connectivity");
  const tab = activeTabProp ?? internalTab;
  const setTab = (next: OpenShellTab) => {
    onTabChange?.(next);
    if (activeTabProp === undefined) setInternalTab(next);
  };

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
      <header className="openshell-hero">
        <h2 id="openshell-title">OpenShell</h2>
        <p className="dim openshell-hero-lead">
          Connect honr to your OpenShell gateway, then configure providers,
          policies, and sandbox specs. Each spec picks which providers and
          policy a run gets.
        </p>
        <div
          className="openshell-status-chip"
          data-testid="openshell-health"
          data-healthy={status?.healthy ? "true" : "false"}
        >
          <span className="dim">Gateway</span>
          <strong className={healthClass} data-testid="openshell-health-label">
            {healthLabel}
          </strong>
          <span className="dim">·</span>
          <span className="dim">mTLS</span>
          <strong data-testid="openshell-mtls-label">{mtlsLabel}</strong>
          <button
            type="button"
            disabled={busy}
            onClick={onRefresh}
            data-testid="openshell-refresh"
          >
            Refresh
          </button>
        </div>
      </header>

      {error && <div className="err">{error}</div>}
      {savedHint && (
        <p className="dim" data-testid="openshell-saved-hint">
          {savedHint}
        </p>
      )}

      <nav
        className="openshell-subnav"
        aria-label="OpenShell sections"
        data-testid="openshell-subnav"
      >
        {TABS.map((t) => (
          <button
            key={t.id}
            type="button"
            className={
              tab === t.id ? "openshell-subnav-btn active" : "openshell-subnav-btn"
            }
            aria-current={tab === t.id ? "page" : undefined}
            onClick={() => setTab(t.id)}
            data-testid={`openshell-tab-${t.id}`}
          >
            {t.label}
          </button>
        ))}
      </nav>

      {tab === "connectivity" && (
        <div
          className="openshell-pane openshell-connectivity"
          data-testid="openshell-connectivity"
          aria-labelledby="openshell-connectivity-title"
        >
          <div className="openshell-band-head">
            <h3 id="openshell-connectivity-title">Connectivity</h3>
            <p className="dim">
              Gateway URL and mTLS certificates. Paste them or import from the
              local OpenShell config dir. Private keys are stored encrypted and
              are not returned by the API.
            </p>
          </div>

          {status?.summary && (
            <pre
              className="openshell-health-summary"
              data-testid="openshell-health-summary"
            >
              {status.summary}
            </pre>
          )}

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
              <button
                type="submit"
                className="primary"
                disabled={busy}
                data-testid="openshell-save"
              >
                Save
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
        </div>
      )}

      {tab === "providers" && (
        <div className="openshell-pane" data-testid="openshell-providers-host">
          {providers}
        </div>
      )}

      {tab === "provider-types" && (
        <div
          className="openshell-pane"
          data-testid="openshell-provider-types-host"
        >
          {providerTypes}
        </div>
      )}

      {tab === "policies" && (
        <div className="openshell-pane" data-testid="openshell-policies-host">
          {policies}
        </div>
      )}

      {tab === "profiles" && (
        <div className="openshell-pane" data-testid="openshell-profiles-host">
          {profiles}
        </div>
      )}
    </section>
  );
}

export function OpenShellPanel({
  activeTab,
  onTabChange,
}: {
  activeTab?: OpenShellTab;
  onTabChange?: (tab: OpenShellTab) => void;
} = {}) {
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
  const [tab, setTab] = useState<OpenShellTab>("connectivity");

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
      activeTab={activeTab ?? tab}
      onTabChange={(next) => {
        onTabChange?.(next);
        if (activeTab === undefined) setTab(next);
      }}
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
      providerTypes={<OpenShellProviderTypesPanel />}
      policies={<OpenShellPoliciesPanel />}
      profiles={<SandboxesPanel />}
    />
  );
}
