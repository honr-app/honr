//! The Billing v2 tree, straight out of the design doc's wireframe and
//! commitment-line diagram. Seeding the exact cards from §2 means the running
//! board can be read side by side with the document it came from.

use crate::model::{EscalationOption, ItemId, Origin, State};
use crate::store::Board;

/// Create a leaf and walk it to Ready.
fn leaf(
    b: &Board,
    parent: ItemId,
    title: &str,
    intent: &str,
    dod: &str,
    capability: Option<&str>,
) -> ItemId {
    let id = b
        .create(
            Some(parent),
            title,
            intent,
            Some(dod.to_string()),
            Origin::Human,
            false,
            capability.map(str::to_string),
        )
        .id;
    let _ = b.transition(id, State::Shaping, "planner", None);
    let _ = b.transition(id, State::Ready, "planner", None);
    id
}

/// Create a container: named, contracted, and not claimable.
fn container(b: &Board, parent: Option<ItemId>, title: &str, intent: &str, above: bool) -> ItemId {
    let id = b.create(parent, title, intent, None, Origin::Human, above, None).id;
    let _ = b.transition(id, State::Shaping, "human", None);
    id
}

fn run(b: &Board, id: ItemId, agent: &str, model: &str, progress: f32, cost_cents: u64) {
    let _ = b.claim(id, agent, Some(model.into()), 45);
    let _ = b.heartbeat(id, agent, progress, cost_cents, 45);
}

pub fn billing_v2(b: &Board) {
    // ---- above the line: human-approved, quarterly cadence -----------------

    let vision = container(
        b,
        None,
        "Air-gapped billing",
        "Ship a billing platform customers can run air-gapped.",
        true,
    );

    let billing = container(
        b,
        Some(vision),
        "Billing v2",
        "Replace the legacy invoicing path before the Q3 release.",
        true,
    );
    b.set_budget(billing, 12_000); // $120

    // Committed to, deliberately unelaborated. A name and a contract and
    // nothing else is the correct state for work four months out.
    let _metering = container(
        b,
        Some(billing),
        "Metering",
        "Usage metering accurate enough to bill from, with no per-customer reconciliation.",
        true,
    );

    // Named only — the Q4 project the vision also carries.
    let _migration = container(
        b,
        Some(vision),
        "Migration tooling",
        "Move existing customers off the legacy path without a maintenance window.",
        true,
    );

    let payments = container(
        b,
        Some(billing),
        "Payment provider",
        "All Stripe interaction behind one swappable adapter.",
        true,
    );
    let invoicing = container(
        b,
        Some(billing),
        "Invoicing",
        "Every invoice reproducible from ledger state alone.",
        true,
    );

    // ---- below the line: agent-owned, hourly churn -------------------------

    let retry = container(
        b,
        Some(payments),
        "Retry handling",
        "No duplicate charges under network partition.",
        false,
    );
    let webhooks = container(
        b,
        Some(payments),
        "Webhook ingest",
        "Every Stripe event processed exactly once, verifiably.",
        false,
    );

    // READY
    let stripe_retry = leaf(
        b,
        retry,
        "Stripe retry",
        "Idempotent retry with exponential backoff on the charge endpoint.",
        "Integration tests green against Stripe test mode; no duplicate charge under induced partition.",
        Some("any"),
    );
    b.seed_backdate(stripe_retry, 40 * 60);

    // RUNNING
    let webhook_sig = leaf(
        b,
        webhooks,
        "Webhook signature",
        "Verify Stripe signatures before any event is acted on.",
        "Forged signature rejected; replayed event is a no-op.",
        Some("any"),
    );
    run(b, webhook_sig, "agent-2", "opus", 0.62, 210);
    b.seed_backdate(webhook_sig, 12 * 60);

    let tax_rules = leaf(
        b,
        invoicing,
        "Tax rules",
        "Apply jurisdiction tax rules at invoice generation, not at render.",
        "Golden-file tests pass for the five launch jurisdictions.",
        Some("any"),
    );
    run(b, tax_rules, "agent-3", "codex", 0.18, 80);
    b.seed_backdate(tax_rules, 3 * 60);

    // NEEDS YOU — an agent is stopped and burning nothing, waiting on you.
    let refund = leaf(
        b,
        payments,
        "Refund policy",
        "Decide how partial refunds interact with proration before implementing.",
        "Refund path matches the documented policy; both cases covered by tests.",
        Some("any"),
    );
    run(b, refund, "agent-4", "opus", 0.30, 140);
    let _ = b.escalate(
        refund,
        "agent-4",
        "Partial refunds and proration overlap. Which wins when a mid-cycle downgrade is refunded?".into(),
        vec![
            EscalationOption {
                label: "Refund takes precedence".into(),
                detail: "Reverse the charge in full, then re-prorate from the downgrade date. \
                         Simpler to reason about; one extra ledger entry per refund."
                    .into(),
            },
            EscalationOption {
                label: "Proration takes precedence".into(),
                detail: "Refund only the unused prorated remainder. Matches the legacy path's \
                         behaviour, so no customer-visible change."
                    .into(),
            },
        ],
        1,
    );
    b.seed_backdate(refund, 18 * 60);

    let prod_migration = leaf(
        b,
        invoicing,
        "Prod DB migration",
        "Backfill the invoice_line ledger table on production.",
        "Backfill completes with zero rows orphaned; rollback script verified on a restore.",
        Some("any"),
    );
    run(b, prod_migration, "agent-5", "opus", 0.90, 95);
    let _ = b.escalate(
        prod_migration,
        "agent-5",
        "The backfill is ready and touches production data. This is irreversible — approve?".into(),
        vec![
            EscalationOption {
                label: "Approve and run now".into(),
                detail: "Estimated 6 minutes, table locked for ~40s.".into(),
            },
            EscalationOption {
                label: "Hold for the maintenance window".into(),
                detail: "Thursday 02:00 UTC. Blocks three downstream cards until then.".into(),
            },
        ],
        0,
    );
    b.seed_backdate(prod_migration, 7 * 60);

    // VERIFY
    let proration = leaf(
        b,
        invoicing,
        "Proration calc",
        "Mid-cycle plan changes bill the difference, never the full period.",
        "Property test: sum of prorated charges equals the full-period price for any split.",
        Some("any"),
    );
    run(b, proration, "agent-6", "opus", 1.0, 190);
    let _ = b.report(proration, "agent-6", 96, 12, vec!["lint".into(), "types".into(), "tests".into()]);

    // REVIEW — finished and safe; can wait until this evening.
    let invoice_pdf = leaf(
        b,
        invoicing,
        "Invoice PDF",
        "Render an invoice to PDF deterministically, offline.",
        "Byte-identical output across two runs; no network access during render.",
        Some("any"),
    );
    run(b, invoice_pdf, "agent-7", "opus", 1.0, 260);
    let _ = b.report(invoice_pdf, "agent-7", 412, 38, vec!["lint".into(), "types".into(), "tests".into()]);
    let _ = b.settle_gates(invoice_pdf, true, "3 gates passed");

    let seat_upgrade = leaf(
        b,
        invoicing,
        "Seat upgrade",
        "Adding seats mid-cycle charges immediately and prorated.",
        "Seat delta invoiced within one billing tick; idempotent under retry.",
        Some("any"),
    );
    run(b, seat_upgrade, "agent-1", "opus", 1.0, 70);
    let _ = b.report(seat_upgrade, "agent-1", 88, 4, vec!["lint".into(), "types".into(), "tests".into()]);
    let _ = b.settle_gates(seat_upgrade, true, "3 gates passed");

    // READY, but blocked — the dependency is visible on the card face.
    let dunning = leaf(
        b,
        invoicing,
        "Dunning emails",
        "Escalating retry emails on failed payment, stopping on success.",
        "Sequence halts on payment; no duplicate sends under retry.",
        Some("writer"),
    );
    b.set_blocked_by(dunning, vec![invoice_pdf]);

    // A deeper Ready pool so the queue has depth and staleness to show.
    for (title, intent, dod, cap) in [
        (
            "Card decline codes",
            "Map Stripe decline codes to actionable customer-facing copy.",
            "Every code in the launch set maps to exactly one message.",
            "any",
        ),
        (
            "Idempotency keys",
            "Every mutating Stripe call carries a stable idempotency key.",
            "Replayed request returns the original result, no second charge.",
            "any",
        ),
        (
            "Adapter interface",
            "Extract the provider interface so Stripe is swappable.",
            "A no-op stub provider passes the same contract tests.",
            "any",
        ),
        (
            "Credit notes",
            "Issue a credit note instead of a refund where policy requires.",
            "Credit note appears on the next invoice and reconciles to zero.",
            "any",
        ),
        (
            "Invoice numbering",
            "Gap-free sequential invoice numbers per customer.",
            "No gaps and no duplicates under concurrent generation.",
            "any",
        ),
        (
            "Currency rounding",
            "Money is Decimal end to end, rounded once at render.",
            "No float appears in any money path; rounding test suite green.",
            "any",
        ),
        (
            "Receipt copy",
            "Plain-language receipt wording for the five launch locales.",
            "Copy reviewed and under the length budget in every locale.",
            "writer",
        ),
    ] {
        let parent = if cap == "writer" { invoicing } else { payments };
        let id = leaf(b, parent, title, intent, dod, Some(cap));
        b.seed_backdate(id, 5 * 60);
    }

    // DONE — already merged this morning.
    for (title, intent, dod, added, removed) in [
        ("Charge endpoint", "Single entry point for creating a charge.", "Contract tests green.", 130, 22),
        ("Customer sync", "Mirror customer records into the billing store.", "Sync is idempotent.", 74, 9),
        ("Ledger schema", "Append-only ledger table with the invoice_line shape.", "Migration applies and rolls back.", 210, 0),
        ("Webhook queue", "Durable queue in front of webhook processing.", "No event lost across a restart.", 158, 31),
        ("Plan catalog", "Load plan definitions from config, not code.", "Catalog round-trips through config.", 91, 44),
    ] {
        let id = leaf(b, payments, title, intent, dod, Some("any"));
        run(b, id, "agent-1", "opus", 1.0, 120);
        let _ = b.report(id, "agent-1", added, removed, vec!["lint".into(), "tests".into()]);
        let _ = b.settle_gates(id, true, "gates passed");
        let _ = b.approve_review(id);
    }

    // A pinned constraint that already bit someone once.
    let _ = b.pin(
        billing,
        "Money is Decimal, never float — including in test fixtures.".into(),
    );
    let _ = b.pin(
        vision,
        "Air-gapped: no phone-home telemetry, no runtime dependency on an external endpoint.".into(),
    );

    b.story(billing, "Seeded Billing v2 from the Q3 plan. 7 agents on the pool.".into());
}
