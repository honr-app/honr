/**
 * Lightweight operator guide — MCP connect + first Project loop.
 * Not the full empty-state onboarding chrome (#279); just a reachable Help surface.
 */
export function Help() {
  return (
    <div className="help-page" data-testid="help-page">
      <header className="board-hero">
        <h1>Operator help</h1>
        <p className="board-lede">
          Drive the board from chat via MCP, or use the UI. Name the repo to
          clone in each Task&apos;s intent/DoD. After a report, card{" "}
          <code>pull_request</code> drives resume remotes.
        </p>
      </header>

      <section className="help-section">
        <h2>Connect MCP</h2>
        <p className="dim">
          honr serves Streamable HTTP MCP at{" "}
          <code>http://127.0.0.1:8080/mcp</code>. Add that URL as an HTTP MCP
          server in your client, then enable the honr server.
        </p>
      </section>

      <section className="help-section">
        <h2>First Project loop</h2>
        <ol className="help-steps">
          <li>
            <code>create_project</code> — auto-seeds a claimable Initial plan
            Task.
          </li>
          <li>
            <code>dispatch</code> the Initial plan — agent writes{" "}
            <code>plan.json</code> only (no docs PR). Each proposed task names
            its clone target in intent/DoD.
          </li>
          <li>
            <strong>Approve</strong> — creates sibling Tasks under the Project.
          </li>
          <li>
            <code>dispatch</code> each Backlog Task (or turn on Project auto
            mode).
          </li>
        </ol>
        <p className="dim" style={{ marginTop: 12 }}>
          Empty board copy points here when there are no Projects yet.
        </p>
      </section>
    </div>
  );
}
