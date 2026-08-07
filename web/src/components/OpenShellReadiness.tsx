import { useEffect, useState } from "react";
import { api } from "../api.js";
import type { OpenShellStatus, SandboxProfilesOut } from "../types.js";

/** Fail-closed: healthy gateway + complete mTLS only. */
export function gatewayMtlsReady(status: OpenShellStatus | null | undefined): boolean {
  if (!status) return false;
  if (status.not_configured) return false;
  if (!status.healthy) return false;
  if (!status.mtls?.complete) return false;
  return true;
}

/** Fail-closed: a configured default sandbox profile id. */
export function sandboxSpecReady(
  out: SandboxProfilesOut | null | undefined,
): boolean {
  if (!out) return false;
  const id = out.default_sandbox_profile_id;
  return typeof id === "string" && id.length > 0;
}

export type ReadinessRow = {
  /** True only when known-ready; unknown/error/incomplete → false. */
  ready: boolean;
  /** True while the first fetch is in flight (still shown as not-ready). */
  checking?: boolean;
  /** Short status detail (summary, profile name, etc.). */
  detail?: string | null;
};

/**
 * Presentational Welcome readiness strip — gateway + sandbox checks with
 * Settings CTAs. Export for UI tests; no fetch here.
 */
export function OpenShellReadinessStripView({
  gateway,
  sandbox,
}: {
  gateway: ReadinessRow;
  sandbox: ReadinessRow;
}) {
  return (
    <section
      className="openshell-readiness"
      aria-labelledby="openshell-readiness-title"
      data-testid="openshell-readiness"
    >
      <header className="openshell-readiness-head">
        <h2 id="openshell-readiness-title">OpenShell readiness</h2>
        <p className="dim openshell-readiness-lede">
          Live checks from the board APIs. Not ready means incomplete or
          unhealthy — fix in Settings before the first Project loop.
        </p>
      </header>
      <ul className="openshell-readiness-list">
        <ReadinessItem
          testId="openshell-readiness-gateway"
          label="Gateway / mTLS"
          row={gateway}
          href="/settings/openshell/connectivity"
          cta="Settings → Connectivity"
        />
        <ReadinessItem
          testId="openshell-readiness-sandbox"
          label="Sandbox spec"
          row={sandbox}
          href="/settings/openshell/profiles"
          cta="Settings → Sandbox specs"
        />
      </ul>
    </section>
  );
}

function ReadinessItem({
  testId,
  label,
  row,
  href,
  cta,
}: {
  testId: string;
  label: string;
  row: ReadinessRow;
  href: string;
  cta: string;
}) {
  const statusLabel = row.checking
    ? "Checking…"
    : row.ready
      ? "Ready"
      : "Not ready";
  const statusClass = row.checking
    ? "dim"
    : row.ready
      ? "openshell-health-ok"
      : "openshell-health-bad";

  return (
    <li
      className="openshell-readiness-item"
      data-testid={testId}
      data-ready={row.ready ? "true" : "false"}
      data-checking={row.checking ? "true" : "false"}
    >
      <div className="openshell-readiness-item-main">
        <span className="openshell-readiness-label">{label}</span>
        <strong className={statusClass} data-testid={`${testId}-status`}>
          {statusLabel}
        </strong>
        {row.detail && !row.checking && (
          <span className="dim openshell-readiness-detail">{row.detail}</span>
        )}
      </div>
      <a
        className="operator-guide-link openshell-readiness-cta"
        href={href}
        data-testid={`${testId}-cta`}
      >
        {cta}
      </a>
    </li>
  );
}

const checkingRow = (): ReadinessRow => ({ ready: false, checking: true });

/**
 * Live strip for the empty Welcome board — reads existing board APIs only.
 * Fail closed on errors; never invent credentials or host discovery.
 */
export function OpenShellReadinessStrip() {
  const [gateway, setGateway] = useState<ReadinessRow>(checkingRow);
  const [sandbox, setSandbox] = useState<ReadinessRow>(checkingRow);

  useEffect(() => {
    let cancelled = false;

    const load = async () => {
      const [st, profiles] = await Promise.all([
        api.getOpenShellStatus().catch(() => null),
        api.listSandboxProfiles().catch(() => null),
      ]);
      if (cancelled) return;

      const gwReady = gatewayMtlsReady(st);
      setGateway({
        ready: gwReady,
        checking: false,
        detail: st?.summary?.split("\n")[0] ?? (st ? null : "Could not read status"),
      });

      const sandReady = sandboxSpecReady(profiles);
      let sandDetail: string | null = null;
      if (!profiles) {
        sandDetail = "Could not read sandbox profiles";
      } else if (sandReady) {
        const id = profiles.default_sandbox_profile_id!;
        const name = profiles.profiles.find((p) => p.id === id)?.name;
        sandDetail = name ? `Default: ${name}` : `Default: ${id}`;
      } else {
        sandDetail = "No default sandbox profile";
      }
      setSandbox({ ready: sandReady, checking: false, detail: sandDetail });
    };

    load();
    return () => {
      cancelled = true;
    };
  }, []);

  return (
    <OpenShellReadinessStripView gateway={gateway} sandbox={sandbox} />
  );
}
