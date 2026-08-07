# Invariants

Properties honr will not trade away as the surface grows, and why each one is
load-bearing. If a change would break one of these, the change is wrong.

## One state machine

Every mutation — UI, MCP, supervisor — goes through `Board` in `src/store.rs`.
Legal transitions live in `src/machine.rs`. No transport holds state-machine
logic.

*Why:* the board has several faces and they must not drift. A rule encoded in
`api.rs` is a rule the MCP seat does not have, and the first time those disagree
you have two products.

## The worker is material, not a participant

The card agent gets no network path to honr. The supervisor calls `claim` /
`heartbeat` / `report` on its behalf.

*Why:* an agent that could reach honr's MCP could approve its own review. The
containment is what makes the review boundary real rather than advisory.

## Liveness is observed, never self-reported

The supervisor parses the agent's output stream. There is no timer-based
keepalive.

*Why:* a keepalive asserts liveness without evidence. The moment a heartbeat can
fire while the agent is wedged, the signal stops meaning anything — and a wedged
agent holding a valid lease is exactly the case the lease exists to catch.

## Merging is human

Approving in honr surfaces the pull request. It never merges.

*Why:* it is the one irreversible step, and it is the step where taste matters
most. A card that passes every gate can still be building the wrong thing.

## Feature branches are writable; the default branch is human-gated

Agents push `honr/card-*` and open PRs. A repository ruleset keeps the default
branch owner-only.

*Why:* defence in depth for the rule above. The boundary should hold even if
honr has a bug.

## Everything in the sandbox stack fails as a hang

Denied egress, a missing credential, a wedged relay — all of it presents as
silence, never as an error. Every exec carries a deadline, and silence is
treated as failure.

*Why:* this is an observation about the stack rather than a choice, but it
shapes the code so thoroughly that it belongs here. It is why `openshell.rs`
looks the way it does, and why "it is taking a while" should be read as "it has
already failed."

## Conventions that follow from these

**Comments explain why, not what.** A comment that restates the line below it is
noise.

**Write the current contract, not the archaeology.** Docs, UI copy, MCP
descriptions, and briefings should make sense to someone who never saw the
previous design. Bug-history notes that justify a still-present invariant are
fine; teaching the product by arguing with its past is not.

**Tests name the failure they prevent**, not the function they call.
`machine.rs` holds the lifecycle invariants; other modules test what breaks
silently — argv shape, shell quoting, config validation.

## Working on honr

```bash
cargo test
cargo clippy --all-targets -- -D warnings
```

Both must be clean, and both run `--offline` inside a sandbox.

Stage deliberately. `git add -A` has committed unintended local state here
before — prefer explicit paths.

### Building these docs

```bash
make docs             # mdbook build → target/mdbook
make docs-serve       # http://localhost:3000
```

Screenshots are **not** committed. CI captures them from a real board against
the fixture in `web/ui-fixture.mjs` and drops them into `docs/images/` before
mdBook runs, so a local `make docs` builds without them. To see them locally:

```bash
npm --prefix web run shots     # → web/shots/
cp web/shots/*.png docs/images/
```

CI publishes `target/mdbook` to
[`honr-app/honr-app.github.io`](https://github.com/honr-app/honr-app.github.io)
via a write deploy key (`PAGES_DEPLOY_KEY`). The org's **Deploy keys** setting
must stay enabled.
