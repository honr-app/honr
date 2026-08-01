# Architectural Design Specification: Plan A (`honr-beads`)

> **Objective:** Integrate **`gastownhall/beads`** (`bd`) as the identity + graph
> store for honr, with a flattened work model: **Project** containers and **flat
> Tasks** linked by dependency edges — not a Vision→Epic→Story ladder.

---

## Work breakdown model (locked)

```mermaid
graph TD
  Project["Project beads epic"] -->|parent-child| PlanTask[Initial plan Task]
  Project -->|plan artifact| Artifact[Plan DAG]
  Artifact -->|Approve Plan| T1[Task A]
  Artifact -->|Approve Plan| T2[Task B]
  Artifact -->|Approve Plan| T3[Task C]
  Project -->|parent-child| T1
  Project -->|parent-child| T2
  Project -->|parent-child| T3
  T2 -->|"blocks"| T1
  T3 -->|"relates-to"| T2
```

| Concept | Beads | Honr |
|---|---|---|
| Project | `issue_type: epic`, root | Container, not claimable, Board swimlane header only |
| Plan | (honr artifact; optional beads note later) | Source of truth: Tasks + keys + deps + DoDs |
| Initial plan Task | `issue_type: task`, `--parent=<project>` | Seeded Ready Task; writes/refines the Plan |
| Task | `issue_type: task`, `--parent=<project>` | Only claimable Board unit |
| Ordering | `bd dep add` (`blocks`, `relates-to`) | `blocked_by` projection + ready filter |
| Split | Sibling tasks under same Project | Original Task → Done |

Lifecycle: create Project → seed Initial plan Task (Ready) → `propose_breakdown`
writes Plan artifact → human **Approve Plan** materializes Tasks to Ready.
The Project itself never enters Ready.

---

## System architecture

```mermaid
graph TD
  subgraph Host["Host control plane"]
    UI["Web UI"] <--> SSE["Axum API / SSE"]
    SSE <--> Store["Board src/store.rs"]
    Sup["Supervisor"] <--> Store
    Store <--> BD["BeadsClient src/beads.rs"]
  end

  subgraph Sandbox["OpenShell sandbox"]
    Agent["Agent"] <--> BDSandbox["bd + BEADS_DIR=/work/.beads"]
  end

  BD <--> Dolt[".beads/embeddeddolt"]
  Sup -->|"tar upload at start"| BDSandbox
```

### Storage split

- **Beads** — canonical hash ids, Project→Task parent edges, task↔task deps,
  open / in_progress / closed, `bd remember` / `bd prime`.
- **honr.json / in-process** — rich lifecycle (Shaping/Ready/Claimed/Running/…),
  lease, sandbox name, live cost, PR URL, escalations.

### Control plane (`BeadsClient`)

- `create_linked` — Project as epic, Task as task with `--parent`
- `dep_add` — `blocks` / `relates-to`
- `list_ready` / `claim` / `close` / `set_status`
- `schedule_beads_mirror` on Board after create / split

### Sandbox

- `bd` baked into `sandbox/Containerfile`
- Host `.beads` tarball uploaded; `BEADS_DIR=/work/.beads`
- Briefing: sibling split only; `bd show` / `bd remember` / `bd dep add`

### UI

- Home: Project cards with plan status (`no_plan` / `awaiting_approval` / `approved_vN`)
- Board: Project swimlane headers only; columns hold Tasks (never the Project)
- Project detail: **Approve Plan** (not “publish Project to Ready”)
- Cards prefer beads hash ids over sequential `#n`

---

## Lifecycle

```mermaid
sequenceDiagram
  actor Human
  participant UI as Web_UI
  participant Sup as Supervisor
  participant BD as beads
  participant OS as Sandbox

  Human->>UI: Create Project
  UI->>BD: bd create epic
  Note over UI: Seed Initial plan Task Ready
  Human->>UI: propose_breakdown Plan artifact
  Note over UI: No board Tasks yet — artifact only
  Human->>UI: Approve Plan
  UI->>BD: bd create task --parent + deps
  loop Dispatch
    Sup->>UI: list_ready Tasks
    Sup->>BD: bd update --claim
    Sup->>OS: sandbox + beads tarball
    OS->>BD: bd show / remember
    OS->>Sup: PR published
    Sup->>BD: bd close
  end
```

---

## Status

Phase 1 eval and this model cutover are landed in-tree:

- [x] Project + Task schema (`honr.yaml`)
- [x] Flat create / sibling split / MCP breakdown
- [x] Plan artifact + Initial plan seed Task + Approve Plan materialize
- [x] `BeadsClient` + dual-write mirror
- [x] Sandbox `BEADS_DIR` sync + briefing
- [x] UI Home / Board swimlanes / beads id display

Still open: full replacement of `honr.json` identity (integer `ItemId` remains
the board key; beads hash is mirrored and shown). Supervisor-run gates (#10)
unchanged and still highest value.
