---
name: gridcraft-agentic-workflow
description: Extend or validate GridCraft's agentic panel, live workbook tools, sales rules and continuation or snapshot recovery. Use for changes to those contracts and service integration, not unrelated spreadsheet parity work.
---

# GridCraft agentic workflow

Work in the current Rust application. Read the repository's `AGENTS.md`, then the relevant
contract in [docs/agentic](../../../docs/agentic/README.md). Paths in commands below are
relative to the repository root. Follow the user's task scope; this skill does not authorize
deployment, provider calls, publication or changes to unrelated services.

## Choose the boundary before editing

- Panel and conversation actions: `crates/ui-egui/src/agent.rs`, dispatched through `app.run`.
- Live identity, read/write/undo and captured snapshots: `crates/engine/src/agent_bridge.rs`.
- Query/rule/run state: `crates/engine/src/review_workflow.rs`.
- Optional embedding transport: `apps/gridcraft-web/src/host.rs` and
  [bridge protocol](../../../docs/agentic/bridge-protocol.md).
- New persistence/provider work: read [delivery.md](../../../docs/agentic/delivery.md) and
  [schema.sql](../../../docs/agentic/schema.sql). Verify what is implemented now; the original
  foundation supplied contracts, not a server, database, model or speech client.

Keep one mounted `SheetApp`/`Session` across modes. Keep one `BridgeState` per caller's
editor lifetime so tokens and retry receipts survive requests. If an embedding host is
used, preserve its iframe and validate both origin and source window. `ready` is construction;
only correlated `opened` acknowledges the intended load.

## Preserve live-edit invariants

Capture the explicit document/sheet/change identity for a proposed change. Do not replace a
stale proposal's token with the newest token and silently apply it. Manual edits may continue
while a provider is thinking; check again immediately before synchronous application.
`DocState::revision` alone misses selection, sheet changes and explicit recalculation.

Refuse mutation while the UI has an unfinished interaction (`host_busy`) and preserve the
draft. Keep programmatic calls dialog-free and bounded. Use one native command for a small
rectangular summary; multiple commands need explicit history grouping before promising batch
undo. The current undo policy is the last native operation, including a newer manual edit.

Match stable request IDs to exact input and replay stored outcomes before current-state
checks. Reusing an ID with different input is an error. Never evict receipts in a way that
lets an uncertain retry repeat a mutation. A reload ends the current in-memory guarantee;
durable recovery must reconcile the old session instead of replaying blindly into a new one.

Read back affected cells after applying. A successful mutation followed by failed verification
or provider timeout remains a successful mutation with an incomplete outcome. Preserve its
receipt and recovery path; do not announce rollback or verified completion without evidence.

Capture immutable workbook bytes and identity together. Acknowledge the exact snapshot only
after persistence succeeds; later edits must stay dirty. Test reordered and duplicate acks,
document switching and closed documents. Database versions are separate from editor tokens.

## Keep business results reproducible

Read [sales-fixture.md](../../../docs/agentic/sales-fixture.md) for periods, monetary units,
refund attribution, qualifying statuses, zero-baseline growth and exact expected results.
Do not compare a partial week with a complete week or introduce silent currency conversion.
Store original rule wording plus validated structure; retrieve enabled rules and apply them
in the query. Unsupported instructions must remain visibly unsupported. Treat external labels
as literal cell text, not formulas. Preserve query provenance with the summary.

For continuations, match run, workbook session and outstanding tool-use ID. Keep completed
actions when the final provider response fails. New durable adapters need transaction and
concurrent-retry tests; the pure in-memory state machine does not prove those behaviors.

## Verify the changed behavior

Run the focused suites that cover the changed boundaries:

```sh
cargo test -p gridcraft-engine agent_bridge::tests --lib
cargo test -p gridcraft-engine review_workflow::tests --lib
cargo test -p gridcraft-ui-egui --test agent_workspace --test host_snapshots --test file_menu
```

Meaningful regression cases include manual-edit conflicts, formula readback, unchanged
history on mode switch, one-step undo, exact retries, wrong identity, pending drafts and an
old snapshot ack after a new edit. Extend those tests when changing their contracts rather
than adding tests that merely match implementation strings.

For UI or parent-transport changes, use
[gridcraft-web-validation](../gridcraft-web-validation/SKILL.md) to exercise the actual app.
Update contract/fixture/validation docs with observed results and separate offline evidence
from live storage, provider and deployment evidence.
