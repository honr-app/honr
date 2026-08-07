import { OperatorGuide } from "./OperatorGuide.js";

/**
 * Help surface — same OperatorGuide content as empty-state onboarding.
 * Hero chrome stays here; Quickstart + MCP (and OpenShell setup) live in
 * OperatorGuide.
 */
export function Help() {
  return (
    <div className="help-page" data-testid="help-page">
      <header className="board-hero">
        <h1>Operator help</h1>
        <p className="board-lede">
          First Project loop, then connect MCP if you want to drive the board
          from a client. Name the clone target in each Task&apos;s intent/DoD.
        </p>
      </header>

      <OperatorGuide />
    </div>
  );
}
