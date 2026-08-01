# Architectural Evaluation & Design Plans: Building `honr` on `beads` (`bd`)

> **Executive Summary**
> This document evaluates **`gastownhall/beads`** (`bd`) as a foundation for `honr`'s task management, graph dependencies, and multi-agent execution model. It analyzes `beads`'s architecture, identifies the pain points in `honr`'s current task model, and proposes three architectural design plans for building on or integrating `beads`.

---

## 1. Deep-Dive Analysis of `gastownhall/beads` (`bd`)

`beads` (`bd`) is an open-source, distributed graph issue tracker designed specifically for AI coding agents and multi-agent coordination.

### Key Architectural Strengths of `beads`:
1. **Dolt-Powered Storage Engine:**
   - Uses **Dolt** (a version-controlled SQL database) running embedded (`.beads/embeddeddolt/`) or server-mode.
   - Supports cell-level merging, native branching, and cross-repo data sync via `bd dolt push` / `bd dolt pull` on git remotes (`refs/dolt/data`).
2. **Directed Acyclic Graph (DAG) Task Model:**
   - Replaces flat/strict hierarchies with graph relations: `blocks`, `parent-child`, `relates-to`, `duplicates`, `supersedes`, and `replies-to`.
   - `bd ready` automatically calculates the graph frontier and returns only unblocked, actionable tasks (`ready`).
3. **Collision-Free Hash IDs:**
   - Hash-based IDs (e.g., `bd-a1b2`, `bd-a1b2.1`) eliminate sequence collision issues when multiple agents create or split tasks concurrently across branches.
4. **Agent Context & Memory Primitives:**
   - `bd prime` generates system prompt context summaries for LLM agents.
   - `bd remember` provides semantic project memory ("insight decay") that is injected into future agent prompts without cluttering `MEMORY.md`.
5. **Zero-Git & Stealth Modes:**
   - Supports `BEADS_DIR` and `--stealth` mode (`no-git-ops: true`), allowing local evaluation in temporary/sandbox directories without polluting project git logs.

---

## 2. Current Pain Points in `honr`'s Task Engine

| Area | Current `honr` Implementation | Pain Point / Limitation |
| :--- | :--- | :--- |
| **Storage Engine** | Single `honr.json` file on host with `RwLock` | No multi-process or multi-branch concurrency. External tools cannot query or update tasks safely. |
| **Hierarchy & Graph** | Fixed ladder (`Vision → Project → Epic → Story → Task`) + `blocked_by` array | Strict tree height rules create confusion (e.g. Epics appearing on execution boards). Lack of flexible graph edges (`relates-to`, `supersedes`). |
| **Sandbox Context** | Static `$HONR_BRIEFING` string passed via CLI flag at container start | Agents inside OpenShell sandboxes cannot query task context dynamically or update progress during long execution turns. |
| **Agent Memory** | Stateslint logs and verdict files (`.honr/*.json`) | No persistent memory across card attempts or agent turns. Learned insights are lost when a sandbox is destroyed. |
| **Concurrency & IDs** | Integer sequential IDs (`#12`, `#13`) | Susceptible to ID collision when multiple sandboxed agents attempt to split cards concurrently on divergent git branches. |

---

## 3. Comparison Matrix: `honr` vs. `beads`

| Dimension | `honr` Current State | `beads` Engine | Benefit of Convergence |
| :--- | :--- | :--- | :--- |
| **Database** | In-memory `Board` + `honr.json` | Embedded Dolt (Version-Controlled SQL) | Branchable, mergeable task history across host and sandboxes |
| **Task Queue** | Custom `list_ready` filtering | Native `bd ready` graph query | Mathematically sound DAG dependency resolution |
| **IDs** | Auto-incrementing u32 (`12`) | Content-hashed (`bd-a1b2`) | Zero-collision task splitting in parallel agent sandboxes |
| **Agent Interface** | Initial Briefing bash variable | `bd prime`, `bd show`, `bd update` | Dynamic interactive task management inside sandboxes |
| **Agent Memory** | None (ephemeral) | `bd remember` semantic decay | Cross-run insight accumulation across agent fleet |

---

## 4. Proposed Architectural Design Plans

Below are three proposed design plans for integrating `beads` into `honr`, ranging from full engine replacement to a hybrid adapter model.

```mermaid
graph TD
    subgraph Plan A: Full Beads Backend Integration
        A1["honr Control Plane (Web UI + Supervisor)"] --> A2["beads CLI / Dolt DB Engine"]
        A2 --> A3["OpenShell Agent Sandbox (bd inside)"]
    end

    subgraph Plan B: Dual-Layer Adapter Integration
        B1["honr Control Plane"] --> B2["honr BoardStore Adapter"]
        B2 --> B3["beads JSON / bd ready"]
        B2 --> B4["honr.json fallback"]
    end

    subgraph Plan C: Native Synthesis (Rust Beads Graph)
        C1["honr Control Plane"] --> C2["Rust Graph Engine (DAG + Hash IDs)"]
        C2 --> C3["beads-compatible JSONL export"]
    end
```

---

### Design Plan A: `honr-beads` (Full Engine Replacement) — *Recommended*

#### Overview
Replace `honr`'s internal `honr.json` storage with **Embedded Dolt via `beads` (`bd`)**. `honr` becomes the visual control plane, supervisor, and OpenShell sandbox orchestrator built on top of `beads`.

#### Architecture Details:
1. **Host Control Plane:**
   - `src/store.rs` wraps `bd` CLI calls or talks directly to Dolt SQL via `tokio::process::Command` / MySQL protocol.
   - `bd ready --json` powers `list_ready()` in `src/supervisor.rs`.
   - `bd update <id> --claim` handles atomic task leasing.
2. **Sandbox Environment:**
   - Mount `.beads/` or pass `BEADS_DIR` / Dolt remote credentials into OpenShell sandboxes.
   - Pre-install `bd` binary in `sandbox/Containerfile`.
   - Agents inside OpenShell execute `bd ready`, `bd show`, `bd close`, and `bd remember` natively during execution turns.
3. **Web UI Alignment:**
   - Web UI queries `honr` API, which maps `bd` graph nodes into goal swimlanes and task cards.
   - Hash IDs (`bd-a1b2`) display as crisp tags on cards.

#### Key Advantages:
- **Zero ID collisions:** Agents can split tasks inside sandboxes without coordinate locking.
- **Agent Empowerment:** Agents interact with the task database directly rather than relying solely on file verdict protocols.
- **Git Branch Merging:** Task state changes made on feature branches merge naturally when PRs are merged via Dolt git remote sync (`refs/dolt/data`).

---

### Design Plan B: `beads-adapter` (Modular Storage Abstraction)

#### Overview
Introduce a `TaskStore` trait in `src/store.rs` with dual backends: `MemoryJsonStore` (existing `honr.json`) and `BeadsStore` (`bd` CLI wrapper).

```rust
#[async_trait]
pub trait TaskStore: Send + Sync {
    async fn list_ready(&self) -> Result<Vec<WorkItem>, String>;
    async fn claim(&self, id: &str, agent_id: &str) -> Result<ClaimGrant, String>;
    async fn complete(&self, id: &str, pr_url: &str) -> Result<(), String>;
    async fn remember(&self, insight: &str) -> Result<(), String>;
}
```

#### Key Advantages:
- Backward-compatible: Keeps `honr` runnable on zero-dependency systems without requiring `bd` or `dolt`.
- Allows incremental migration and side-by-side benchmarking.

---

### Design Plan C: `honr-native` (Synthesis of `beads` Design Concepts)

#### Overview
Keep `honr`'s native Rust implementation, but adopt `beads`'s core design principles into `honr`:
1. **Hash-based IDs (`honr-a1b2`):** Replace auto-incrementing integers with short SHA hash prefixes.
2. **Directed Graph (`petgraph`):** Replace tree level constraints with a DAG graph structure (`petgraph` crate in Rust).
3. **`bd remember` Equivalent (`honr pin` / `honr memory`):** Implement persistent project memory injection in briefings.
4. **Beads JSON Interchange:** Support import/export from `.beads/issues.jsonl`.

#### Key Advantages:
- Single compiled Rust binary with no external process dependencies (`bd` or Dolt server).
- Tailored specifically to `honr`'s supervisor loop and Web UI.

---

## 5. Recommendation & Proposed Evaluation Roadmap

### Recommendation: **Plan A (`honr-beads`)** — adopted
Building on top of `beads` leverages Dolt's multi-agent versioning, eliminates ID collision bugs, and gives sandboxed agents a standardized tool (`bd`) to inspect and update tasks.

**Work model locked with Plan A:** top level is a **Project** (beads `epic`); claimable work is **flat Tasks** under it (`--parent`); Task↔Task links use `blocks` / `relates-to`. Vision / Epic / Story ladder is retired. See [`plan_a_honr_beads_design.md`](plan_a_honr_beads_design.md).

### Evaluation Roadmap (Phased Plan):

```carousel
### Phase 1: Prototype Evaluation Project
* Create a dedicated evaluation repository (`honr-beads-eval`).
* Install `beads` (`bd`) and run `bd init --stealth` in temporary project directories.
* Test `bd ready`, `bd create`, `bd dep add`, and `bd remember` in multi-agent scenario scripts.
<!-- slide -->
### Phase 2: OpenShell Sandbox Integration
* Add `bd` binary to `sandbox/Containerfile`.
* Test passing `BEADS_DIR` or socket mounting into Landlock container sandboxes.
* Benchmark agent tool calls (`bd ready --json`, `bd remember`) inside `agy` / `claude` execution runs.
<!-- slide -->
### Phase 3: Control Plane Bridge
* Implement `BeadsStore` in `src/store.rs` wrapping `bd --json` output.
* Connect Web UI goal swimlanes to `beads` DAG nodes.
* Validate end-to-end task splitting, decision escalations, and PR creation.
```

---

## 6. Next Steps for Feedback

1. **Which design plan do you prefer?**
   - **Plan A (Recommended):** Full `beads` engine replacement (Dolt graph + `bd` in sandboxes).
   - **Plan B:** Modular adapter layer supporting both `honr.json` and `beads`.
   - **Plan C:** Synthesize `beads` graph/hash concepts directly into Rust natively.
2. Should we initialize a dedicated research directory / evaluation sandbox to test `beads` CLI workflows interactively?
