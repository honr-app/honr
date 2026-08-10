import { OpenShellReadinessStrip } from "./OpenShellReadiness.js";
import { OperatorGuide } from "./OperatorGuide.js";

/**
 * Help surface — Welcome hero + OpenShell readiness + OperatorGuide
 * (Create Project stays on the Board).
 */
export function Help() {
  return (
    <div className="help-page" data-testid="help-page">
      <header className="board-hero">
        <h1>Welcome to honr</h1>
        <p className="board-lede">
          Create a Project, approve its plan, then dispatch work. Setup steps
          are below.
        </p>
      </header>

      <div className="board-empty" data-testid="help-welcome">
        <OpenShellReadinessStrip />
        <OperatorGuide />
      </div>
    </div>
  );
}
