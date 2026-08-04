/**
 * Lightweight operator guide — MCP connect + first Project loop with init_plan.
 * Not the full empty-state onboarding chrome (#279); just a reachable Help surface.
 */
export function Help() {
  return (
    <div className="help-page" data-testid="help-page">
      <header className="board-hero">
        <h1>Operator help</h1>
        <p className="board-lede">
          Drive the board from chat via MCP, or use the UI. Product remotes are
          Task-scoped — Projects are containers only.
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
            <code>create_project</code> — container only (no child Tasks).
          </li>
          <li>
            <code>init_plan</code> — requires{" "}
            <code>repo.upstream</code> (<code>owner/name</code>); optional{" "}
            <code>fork</code>, <code>base</code> (default <code>main</code>).
            Seeds the Initial plan Task with that binding.
          </li>
          <li>
            <code>dispatch</code> the Initial plan — agent writes{" "}
            <code>plan.json</code> + a plan/docs PR.
          </li>
          <li>
            <strong>Approve</strong> — sibling Tasks inherit the Initial plan
            Task repo unless a child overrides (multi-repo under one Project).
          </li>
          <li>
            <code>dispatch</code> each Backlog Task (or turn on Project auto
            mode).
          </li>
        </ol>
        <p className="dim" style={{ marginTop: 12 }}>
          In the UI: open a Project with no Initial plan → <strong>Start
          planning</strong> with the same repo fields. Empty board copy points
          here when there are no Projects yet.
        </p>
      </section>
    </div>
  );
}
