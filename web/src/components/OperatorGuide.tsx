const MCP_URL = "http://127.0.0.1:8080/mcp";

const CURSOR_MCP_JSON = `{
  "mcpServers": {
    "honr": {
      "type": "http",
      "url": "${MCP_URL}",
      "auth": { "CLIENT_ID": "honr-cursor", "scopes": ["mcp"] }
    }
  }
}`;

const CLAUDE_MCP_ADD = `claude mcp add --transport http honr ${MCP_URL}`;

/**
 * Reusable operator onboarding — MCP connect, OpenShell + sandbox setup,
 * then the first Project loop. Cursor / Claude snippets are secondary examples.
 * Embed from Board empty state or Help; keep chrome (hero, nav) outside.
 */
export function OperatorGuide() {
  return (
    <div className="operator-guide" data-testid="operator-guide">
      <section
        className="operator-guide-section"
        aria-labelledby="operator-guide-mcp-title"
        data-testid="operator-guide-mcp"
      >
        <h2 id="operator-guide-mcp-title">Connect MCP</h2>
        <p className="dim">
          Drive Projects and Tasks from any MCP client. `/mcp` is the operator
          seat: operator tools only (no worker verbs). honr must already be
          listening before you add the server.
        </p>
        <ol className="operator-guide-steps" data-testid="operator-guide-mcp-steps">
          <li>
            Start honr so it is listening (API + MCP on port 8080 by default).
          </li>
          <li>
            Point your client at the Streamable HTTP endpoint:
            <pre
              className="operator-guide-snippet"
              data-testid="operator-guide-mcp-url"
            >
              {MCP_URL}
            </pre>
          </li>
          <li>
            Transport is <strong>HTTP / Streamable HTTP</strong> (not stdio).
          </li>
          <li>
            Add an MCP server named <code>honr</code> at that URL.
          </li>
          <li>
            After local admin exists, authenticate via MCP OAuth (browser login /
            consent — same admin or GitHub allowlist as the board). In Cursor:
            Tools &amp; MCP → Authenticate / Connect.
          </li>
          <li>Enable or reload the server if your client requires it.</li>
        </ol>

        <aside
          className="operator-guide-examples"
          data-testid="operator-guide-client-examples"
        >
          <h3>Client examples</h3>
          <p className="dim">
            Optional — same endpoint and server name in any MCP client that
            speaks Streamable HTTP.
          </p>
          <p className="operator-guide-example-label">
            Cursor — <code>.cursor/mcp.json</code>
          </p>
          <pre
            className="operator-guide-snippet"
            data-testid="operator-guide-cursor-snippet"
          >
            {CURSOR_MCP_JSON}
          </pre>
          <p className="operator-guide-example-label">Claude Code</p>
          <pre
            className="operator-guide-snippet"
            data-testid="operator-guide-claude-snippet"
          >
            {CLAUDE_MCP_ADD}
          </pre>
        </aside>
      </section>

      <section
        className="operator-guide-section"
        aria-labelledby="operator-guide-openshell-title"
        data-testid="operator-guide-openshell"
      >
        <h2 id="operator-guide-openshell-title">OpenShell + sandbox</h2>
        <p className="dim">
          Before the first Project loop, configure the OpenShell gateway,
          providers, a sandbox spec, and enable agents. honr does not discover
          host credentials — paste them in Settings.
        </p>
        <ol
          className="operator-guide-steps"
          data-testid="operator-guide-openshell-steps"
        >
          <li>
            <a
              className="operator-guide-link"
              href="/settings/openshell/connectivity"
            >
              Settings → OpenShell → Connectivity
            </a>
            {" "}
            — gateway endpoint and mTLS PEMs (CA, client cert, client key).
            Refresh status until Healthy.
          </li>
          <li>
            <a
              className="operator-guide-link"
              href="/settings/openshell/providers"
            >
              Settings → OpenShell → Providers
            </a>
            {" "}
            — model providers; Sync applies them to the gateway. For GitHub{" "}
            <code>GH_TOKEN</code>, use{" "}
            <a className="operator-guide-link" href="/settings/github-app">
              Settings → GitHub App
            </a>
            , not the providers band.
          </li>
          <li>
            <a
              className="operator-guide-link"
              href="/settings/openshell/profiles"
            >
              Settings → OpenShell → Sandbox specs
            </a>
            {" "}
            — default profile / sandbox image runs will use.
          </li>
          <li>
            Enable agents via{" "}
            <a className="operator-guide-link" href="/settings/agent-runtime">
              Settings → Agent runtime
            </a>
            {" "}
            or <code>honr.yaml</code> (
            <code>execution.agents.enabled</code>), then restart if you changed
            the file.
          </li>
        </ol>
      </section>

      <section
        className="operator-guide-section"
        aria-labelledby="operator-guide-loop-title"
        data-testid="operator-guide-loop"
      >
        <h2 id="operator-guide-loop-title">First Project loop</h2>
        <ol className="operator-guide-steps" data-testid="operator-guide-loop-steps">
          <li>
            <code>create_project</code> with required{" "}
            <code>clone_repo</code> (<code>owner/name</code>) — auto-seeds a
            claimable <strong>Initial plan</strong> Task stamped with that
            planning clone target.
          </li>
          <li>
            <code>dispatch</code> the Initial plan — the agent clones{" "}
            <code>clone_repo</code> and writes <code>plan.json</code>. Each
            proposed task names its clone target in intent/DoD.
          </li>
          <li>
            <strong>Approve</strong> — creates sibling Tasks under the Project.
          </li>
          <li>
            <code>dispatch</code> each Backlog Task (or turn on Project auto
            mode).
          </li>
        </ol>
        <p className="dim" data-testid="operator-guide-idle-note">
          Agents stay idle until you enable them in Settings and dispatch. Name
          the repo to clone in each Task&apos;s intent/DoD. After a report, card{" "}
          <code>pull_request</code> drives resume remotes.
        </p>
      </section>
    </div>
  );
}
