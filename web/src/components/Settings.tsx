import { useState } from "react";

type SettingsSection = "sandboxes" | "general";

const SECTIONS: { id: SettingsSection; label: string; stub?: boolean }[] = [
  { id: "sandboxes", label: "Sandboxes" },
  { id: "general", label: "General", stub: true },
];

/**
 * Settings shell — thin scaffolding so Sandboxes can land without turning the
 * board into a config surface. Other sections stay stubs until wired.
 */
export function Settings() {
  const [section, setSection] = useState<SettingsSection>("sandboxes");

  return (
    <div className="settings" data-testid="settings">
      <header className="settings-hero">
        <h1>Settings</h1>
        <p className="settings-lede">
          Control-plane preferences. Sandboxes is the first real panel; other
          sections are placeholders for now.
        </p>
      </header>

      <div className="settings-body">
        <nav className="settings-nav" aria-label="Settings sections">
          {SECTIONS.map((s) => (
            <button
              key={s.id}
              type="button"
              className={`settings-nav-btn ${section === s.id ? "active" : ""}`}
              aria-current={section === s.id ? "page" : undefined}
              onClick={() => setSection(s.id)}
              data-testid={`settings-nav-${s.id}`}
            >
              {s.label}
              {s.stub && <span className="dim settings-stub-tag">soon</span>}
            </button>
          ))}
        </nav>

        <div className="settings-panel" data-testid={`settings-panel-${section}`}>
          {section === "sandboxes" ? <SandboxesPlaceholder /> : <GeneralStub />}
        </div>
      </div>
    </div>
  );
}

function SandboxesPlaceholder() {
  return (
    <section aria-labelledby="sandboxes-title">
      <h2 id="sandboxes-title">Sandboxes</h2>
      <p className="dim">
        Named sandbox profiles, the global default, and Project-level overrides
        will live here. Wiring lands in a follow-up card — this panel is the
        shell only.
      </p>
      <div className="settings-placeholder" data-testid="sandboxes-placeholder">
        <p>No profiles yet.</p>
        <p className="dim">List / create / set default will appear once the API is ready.</p>
      </div>
    </section>
  );
}

function GeneralStub() {
  return (
    <section aria-labelledby="general-title">
      <h2 id="general-title">General</h2>
      <p className="dim">
        Stub section — reserved for preferences that are not sandbox-related.
        Nothing to configure here yet.
      </p>
      <div className="settings-placeholder" data-testid="general-stub">
        <p className="dim">Coming soon.</p>
      </div>
    </section>
  );
}
