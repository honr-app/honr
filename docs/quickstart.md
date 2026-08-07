# Quickstart

Get the board running on your machine in a few minutes, with agents off. No
Docker, no gateway, no credentials, no spend.

Agents being off by default is deliberate: the control plane should run on a
laptop that only needs to show and shape work. When you are ready for sandboxed
runs, start from the empty-board **Welcome** guide (or **Help**) — OpenShell +
sandbox before the Project loop — then the prose checklist on
[Your first agent](first-agent.md).

**You need:** a current Rust stable toolchain, and a recent Node.js if you want
to build the UI.

## 1. Run it

```bash
git clone https://github.com/honr-app/honr.git
cd honr
cargo run
```

That serves the API, SSE, MCP, and the built UI on
<http://127.0.0.1:8080>. `HONR_PORT` overrides the port.

If `web/dist` does not exist yet, build the UI once:

```bash
npm --prefix web install && npm --prefix web run build
```

## 2. Create your admin

The first time you open the board it asks you to create an admin account. Until
you do, the API refuses everything — there is no anonymous mode.

Pick any username and password; it is stored locally, in your board database.

## 3. Make something

The board starts empty, so make it not be. Create a **Project**, give it an
intent, and point it at a repository (`owner/name` — this is the repo the
planning agent will clone).

honr seeds a claimable **Initial plan** Task under it automatically. You now
have a Project, a Task, and a board that looks like the one in the
[Tour](tour.md) — minus anything running, because agents are off.

Click into the card. The detail drawer shows **why this exists** (the chain up
to its Project), its definition of done, and the Proposed Tasks section that an
agent would fill in.

Move it around. Nothing will claim it, nothing will spend money, and you cannot
break anything that a restart does not fix.

## 4. Connect a chat client (optional)

You can drive the board from Cursor or Claude Code over MCP instead of the UI.
honr must already be listening.

`/mcp` is the **operator surface**: shape Projects, triage, dispatch, park,
steer, approve. Worker verbs (`claim`, `heartbeat`, `report`, …) are not there —
those belong to the supervisor.

**Cursor** — project config is already in [`.cursor/mcp.json`](https://github.com/honr-app/honr/blob/main/.cursor/mcp.json):

```json
{
  "mcpServers": {
    "honr": {
      "type": "http",
      "url": "http://127.0.0.1:8080/mcp",
      "auth": { "CLIENT_ID": "honr-cursor", "scopes": ["mcp"] }
    }
  }
}
```

```bash
agent mcp login honr
```

**Claude Code:**

```bash
claude mcp add --transport http honr http://localhost:8080/mcp
```

Either way a browser opens for login and consent, using the same account you
just created. Tokens survive a honr restart, so you will not be logging in
repeatedly.

If the tools list stays empty, reload the client.

## Developing on it

```bash
make dev                  # watchexec rebuilds and restarts on Rust changes
make dev-ui               # Vite on :5173, proxying to :8080
```

`make dev` needs [`watchexec`](https://crates.io/crates/watchexec-cli)
(`brew install watchexec` or `cargo install watchexec-cli`).

## Next

- **[Your first agent](first-agent.md)** — Welcome/Help OpenShell onboarding, then one sandboxed run
- [Workflow](workflow.md) — the day-to-day loop
- [Configuration](configuration.md) — database URL, environment, Settings
