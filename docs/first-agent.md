# Your first agent

The shortest path from a board with agents off to one real sandboxed run that
opens a pull request.

> **This spends real money and opens pull requests.** Work through it once,
> deliberately, on a repo you do not mind receiving a small PR against.

## Start in the product

On an empty board, **Welcome to honr** embeds the same operator guide as
**Help** (nav → Help). That guide is the named first-run path:

1. **Connect MCP**
2. **OpenShell + sandbox** — Connectivity, Providers, Sandbox specs, then
   agents enabled
3. **First Project loop**

Work the checklist there; deep links land on Settings → OpenShell and Agent
runtime. This page is the prose companion: the same order, with checks and the
host-side pieces the UI does not run for you.

Every step has a check. Do not move on until the check passes — see
[why](troubleshooting.md#everything-fails-as-a-hang) below: in this stack a
half-finished step does not error, it hangs.

## What you are assembling

Four things have to be true on the host. The tools named are examples, not the
only stack.

| # | Role | Concretely |
|---|---|---|
| 1 | Something that runs containers | podman, Colima, or Docker |
| 2 | The OpenShell gateway | holds sandboxes, network policy, credentials |
| 3 | Model + GitHub credentials | as OpenShell *providers*, never baked into an image |
| 4 | A sandbox image | with whatever toolchain the work needs |

honr itself holds none of those credentials. It talks to the gateway over gRPC
and the gateway injects secrets on egress, so nothing sensitive enters the
sandbox.

## 1. A compute driver

OpenShell's gateway needs a working Docker-compatible API. How you provide it is
your choice:

| Driver | Typical setup |
|---|---|
| **podman** | `podman machine start` |
| **Colima** | `colima start`, then point the gateway at `unix://$HOME/.colima/default/docker.sock` |
| **Docker Desktop / engine** | Make sure the daemon is up and the gateway can reach its socket |

`DOCKER_HOST` and friends belong to the **gateway process**, not to honr
Settings.

**Check:**

```bash
docker info        # must succeed
```

The driver can stop on its own — the podman machine especially. honr classifies
that as infrastructure rather than the card failing, so it will not burn a
card's retry budget, but it cannot prevent the outage.

## 2. The OpenShell gateway

Start it however your install expects (Homebrew service, systemd, …). honr does
not spawn an `openshell` CLI for board traffic: `src/openshell.rs` talks to the
gateway in-process over gRPC with client certificates.

**Check:**

```bash
openshell status   # expect Connected + Authenticated
```

Then tell honr how to reach it, in **Settings → OpenShell → Connectivity**
(Welcome/Help deep-links here):

- **Gateway endpoint** — often `https://127.0.0.1:17670`. Deliberately not
  honr's `8080`; your install may differ.
- **mTLS PEMs** — CA, client cert, client key. Paste them in. They are sealed
  into the board database with `~/.config/honr/master.key`, and the API never
  hands private key material back. honr will not go looking for them on disk:
  it assumes nothing about what is on the host, so configuration is uploaded
  rather than discovered.

**Settings** (stored on the board) is the live source of truth for gateway
endpoint and sealed PEMs — same split as [Configuration](configuration.md).

**Check:** hit **Refresh status** in Settings. You want **Healthy**.

## 3. Providers

**Settings → OpenShell → Providers** holds the desired provider list, with
credentials sealed. That list is the source of truth; **Sync** applies it to the
gateway. Providers marked **attach** are passed when a sandbox is created.

For inference, point OpenShell's local router at your model:

```bash
openshell provider create --name vertex --type google-vertex-ai --from-gcloud-adc \
  --config VERTEX_AI_PROJECT_ID=<project> --config VERTEX_AI_REGION=global
openshell inference set --provider vertex --model claude-sonnet-4-6@default
```

Agents then reach models at `https://inference.local` and the gateway swaps in
the real credential on the way out. Details, including the one environment
variable that will silently break this: [Sandbox](sandbox.md).

For GitHub, use **Settings → GitHub App**, not the providers band. Installation
tokens sync into the OpenShell `github` provider as `GH_TOKEN` on their own.

**Check:** Sync reports success and the providers you expect are listed on the
gateway.

## 4. A sandbox image

Build or pull whatever your sandbox spec's image field names. For honr's own
Rust toolchain image:

```bash
make sandbox
# or: podman build -f sandbox/Containerfile -t honr-sandbox:latest .
# Docker: CONTAINER_ENGINE=docker make sandbox
```

From the **repo root**, not `sandbox/` — `Cargo.lock` and
`web/package-lock.json` have to be in build context. The image pre-warms cargo
and npm caches so crates.io never has to be reachable from a sandbox.

Then confirm the board's **default** sandbox spec in
**Settings → OpenShell → Sandbox specs** (Welcome/Help deep-links here). Specs
live on the board; [Configuration](configuration.md#sandbox-specs) and
[Sandbox](sandbox.md) cover resolution and policy.

**Check:** `podman image ls | grep honr-sandbox`, and the default spec's image
matches what you built.

## 5. Turn agents on

Enable agents in **Settings → Agent runtime**, or set the process boot gate in
`honr.yaml`:

```yaml
# honr.yaml
execution:
  agents:
    enabled: true
```

Board Settings is the live source of truth for Agent runtime once saved
([Configuration](configuration.md)). `honr.yaml` `agents.enabled` is the
startup gate (and the seed for a fresh board's runtime toggle). If the process
started with agents disabled, **restart honr** after enabling so the dispatch
loop starts.

> Put `enabled: false` back before you commit YAML. It has been committed as
> `true` once already, swept in by `git add -A`, which makes a fresh clone spend
> money on startup.

**Check:** the startup log no longer says
`execution.agents.enabled = false; board runs with no executor`.

## 6. Run one card

This is the **First Project loop** section of Welcome/Help:

1. Create a Project pointed at your repo (`clone_repo` as `owner/name`).
2. **Start** its Initial plan card.
3. Watch it move Backlog → Running. The card shows its sandbox name.
4. It lands in **Review** with a proposed breakdown. Read it, edit it, Approve.
5. **Start** one of the resulting Tasks.
6. It opens a pull request and lands in Review.
7. Merge on GitHub. The card moves to Done.

Keep `max_concurrent` at 1 until you have watched this work end to end.

If something takes longer than it should, it has already failed —
[Troubleshooting](troubleshooting.md#everything-fails-as-a-hang) is the next
page you want. Denied egress, missing credentials, and wedged relays present as
silence; treat hangs as failure, not as "give it more time."

## Next

- [Workflow](workflow.md) — steering cards day to day
- [Configuration](configuration.md) — sandbox specs, engines, timeouts
- [Sandbox](sandbox.md) — what actually happens inside a run
- [Cockpit](cockpit.md) — a durable terminal seat with operator reach
