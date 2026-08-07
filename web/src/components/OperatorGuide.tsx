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
 * Reusable operator onboarding — Quickstart (first Project loop), MCP connect,
 * and OpenShell + sandbox setup. Cursor / Claude snippets are secondary examples.
 * Embed from Board empty state or Help; keep chrome (hero, nav) outside.
 */
export function OperatorGuide() {
  return (
    <div className="operator-guide" data-testid="operator-guide">
      <section
        className="operator-guide-section"
        aria-labelledby="operator-guide-quickstart-title"
        data-testid="operator-guide-quickstart"
      >
        <h2 id="operator-guide-quickstart-title">Quickstart</h2>
        <p className="dim">
          The in-app first loop: create a Project, plan, Approve, then dispatch.
          Agents stay idle until you enable them and dispatch.
        </p>
        <ol
          className="operator-guide-steps"
          data-testid="operator-guide-quickstart-steps"
        >
          <li>
            Create a Project with required <code>clone_repo</code> (
            <code>owner/name</code>) — via the board or{" "}
            <code>create_project</code>. honr auto-seeds a claimable{" "}
            <strong>Initial plan</strong> Task stamped with that planning clone
            target.
          </li>
          <li>
            <code>dispatch</code> the Initial plan — the agent clones{" "}
            <code>clone_repo</code> and writes <code>plan.json</code>. Each
            proposed task names its clone target in intent/DoD.
          </li>
          <li>
            <strong>Approve</strong> — creates sibling Tasks under the Project
            (never merges).
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

      <section
        className="operator-guide-section"
        aria-labelledby="operator-guide-mcp-title"
        data-testid="operator-guide-mcp"
      >
        <h2 id="operator-guide-mcp-title">Connect MCP</h2>
        <p className="dim">
          Drive Projects and Tasks from any MCP client.{" "}
          <code>/mcp</code> is the <strong>operator seat</strong>: shape
          Projects, triage, dispatch, park, steer, approve — operator tools
          only. Worker verbs (<code>claim</code>, <code>heartbeat</code>,{" "}
          <code>report</code>, …) are not on this seat. honr must already be
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
            Transport is <strong>Streamable HTTP</strong> (not stdio).
          </li>
          <li>
            Add an MCP server named <code>honr</code> at that URL.
          </li>
          <li>
            After local admin exists, authenticate via MCP OAuth (browser login /
            consent — same admin or GitHub allowlist as the board).
          </li>
        </ol>
        <p className="dim" data-testid="operator-guide-mcp-empty-tools">
          Tokens survive a honr restart. If the tools list stays empty, reload
          the client.
        </p>

        <aside
          className="operator-guide-examples"
          data-testid="operator-guide-client-examples"
        >
          <h3>Client examples</h3>
          <p className="dim">
            Optional — same Streamable HTTP endpoint and server name{" "}
            <code>honr</code> in any MCP client.
          </p>
          <p className="operator-guide-example-label">
            Cursor — <code>.cursor/mcp.json</code>, then Tools &amp; MCP →
            Authenticate / Connect (or <code>agent mcp login honr</code>)
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
          Before agents can run the Quickstart loop, configure the OpenShell
          gateway, providers, a sandbox spec, and enable agents. honr does not
          discover host credentials — paste them in Settings.
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
            — configure providers (including shipped type{" "}
            <code>github-app</code> / <code>GH_TOKEN</code>). Sync applies them
            to the gateway. Provider types lists the shipped{" "}
            <code>github-app</code> profile next to <code>cursor-agent</code>{" "}
            and <code>antigravity</code>.
          </li>
          <li>
            <a
              className="operator-guide-link"
              href="/settings/openshell/policies"
            >
              Settings → OpenShell → Policies
            </a>
            {" "}
            — named OpenShell allow-list YAML (filesystem / network). Sandbox
            specs reference a policy by id.
          </li>
          <li>
            <a
              className="operator-guide-link"
              href="/settings/openshell/profiles"
            >
              Settings → OpenShell → Sandbox specs
            </a>
            {" "}
            — image, resources, engine, and which policy + providers attach on
            create.
          </li>
          <li>
            Tune concurrency / timeouts under{" "}
            <a className="operator-guide-link" href="/settings/agent-runtime">
              Settings → Agent runtime
            </a>
            {" "}
            if needed (dispatch starts with the process).
          </li>
        </ol>
      </section>
    </div>
  );
}
