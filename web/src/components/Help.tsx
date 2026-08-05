import { OperatorGuide } from "./OperatorGuide.js";

/**
 * Help surface — same OperatorGuide content as empty-state onboarding.
 * Hero chrome stays here; MCP + first-loop copy lives in OperatorGuide.
 */
export function Help() {
  return (
    <div className="help-page" data-testid="help-page">
      <header className="board-hero">
        <h1>Operator help</h1>
        <p className="board-lede">
          Connect an MCP client, then run the first Project loop. Name the
          clone target in each Task&apos;s intent/DoD.
        </p>
      </header>

      <OperatorGuide />
    </div>
  );
}
