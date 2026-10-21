# Agentic spreadsheet foundation

> Last reviewed: 2026-10-21 · Last updated: 2026-10-21 · Change: major (fork foundation)

The product turns business records and explicit remembered rules into an editable workbook.
This fork adds the base Agentic interface to the existing Rust application, alongside the
live workbook bridge and executable offline contracts. It does not yet supply a hosted
product, an Anthropic client, speech, or database persistence.

## Architecture decision

Build the agentic layer in this project using its current Rust/egui stack. Classic and
Agentic are layouts of the same mounted `SheetApp` and engine `Session`: showing or hiding
the conversation panel must preserve the workbook and its native undo history. The open
editor remains authoritative for workbook content.

Add provider and persistence adapters in Rust when the services become available. Introduce
an additional service only where credentials, durable state or deployment require it; a
TypeScript shell, another repository and an iframe are not prerequisites. If an external
host embeds the web editor, the version 2 bridge is available and that host must retain one
iframe across mode changes. Browser builds must keep provider secrets on a trusted backend.

| Boundary | Responsibility | Foundation here |
|---|---|---|
| GridCraft native/WASM | Manual editing, formulas, native history, live workbook | Existing Rust/egui editor and engine session |
| Agentic interface | Mode toggle, typed requests, transcript, action results | Rust/egui panel in this application; deterministic offline intents |
| Rust adapter | Bounded inspection, one rectangular write, native undo, snapshot receipts | `crates/engine/src/agent_bridge.rs` |
| Business query | Typed sales, structured enabled rules, fixed periods, provenance | `crates/engine/src/review_workflow.rs` offline reference |
| Optional embedding host | Integrate the same live web editor into another application | Version 2 same-origin bridge; no separate shell required |
| Service adapters/backend | Anthropic loop, validated tools, durable continuations | Offline state-machine reference and proposed SQL schema; network adapters deferred |
| Aiven PostgreSQL | Sales, explicit rules, snapshots, runs, tool receipts | Proposed migration; not applied |
| ElevenLabs | Push-to-talk STT and brief TTS of verified results | Interface contract; not called |

The current conversation panel recognizes supported deterministic demo intents. It does not
call a model, interpret arbitrary requests or query business systems. Its sales records are
the fixed synthetic fixture; remembered rules and conversation state last only for the
current application session. Unsupported requests remain visibly unsupported. Future
Anthropic tools should reuse the validated engine/business contracts and must match these
fixture results. Keep user content and database text as tool data; never elevate it into
agent system instructions.

## Repository analysis and baseline

Analysis started from GridCraft commit `8ae67c5534f3d7f145423a57206cf5fd08fe9221`.
The foundation builds on that base; the base pin is **not** a pin of the completed
foundation. Record the reviewed fork commit and built artifact digest before
deployment or distribution. Keep `Cargo.lock` and the pinned `craft-libs` dependency.

The sibling `../artcraft` was inspected at
`800217ed04d10fc12706c28e6e91ae2a6b286092`. It is a Tauri/React AI image/video desktop
client, with services in a different repository, not a spreadsheet backend. Useful patterns
are long-lived listeners, typed background results and per-operation recovery. Its deliberate
editor remounting on tab changes is unsuitable here. No sibling code or assets were copied.

The ignored `plan/` files and shared `craftrules` checkout were absent. The advertised
`storytold/craftrules` AGENTS URL also returned 404 during verification. The committed
`docs/architecture.md`, repository AGENTS instructions and supplied product plan guided this
work. Restore the shared guidance before making future cross-project standards changes.

## Start here

- [Bridge protocol](bridge-protocol.md): messages, identity, limits, save acknowledgements.
- [Deterministic sales example](sales-fixture.md): metric, exact answers and source rules.
- [Service integration and delivery](delivery.md): next implementation steps and live gates.
- [Proposed PostgreSQL schema](schema.sql): storage contract, not an applied migration.
- [Validation](validation.md): exact checks and remaining limitations.

Run the provider-free example (the destination must not already exist):

```sh
cargo run -p gridcraft-engine --example agentic_review -- /tmp/review.xlsx
```

It writes a synthetic XLSX, verifies live formula results and duplicate protection, and
undoes the live operation; the exported file remains available for editing. Its save
acknowledgement is for a real local file write, not PostgreSQL.

The example starts with the fixture's test-order exclusion rule enabled, produces one
rectangular edit, verifies calculated values, exports a snapshot and allows one native undo.
Snapshot regression tests also verify that edits made after export remain dirty when that
snapshot is acknowledged. Do not promise undo of an entire conversation: manual edits share
the same history.

For the base interface, launch the existing application with `cargo run -p gridcraft` and
switch to Agentic mode. The panel and spreadsheet share the same session. The panel starts
with **no saved rules**. First submit “Remember to exclude test orders”, then “compare sales”
or use the matching buttons. Review the fixed-period results, provenance and the stated
`A1:E…` overwrite target before applying the prepared change. The panel's undo action undoes
the last native operation, including a manual edit if that was more recent. Rules disappear
when the application session ends. Service availability is stated in the interface; no
successful provider response is simulated. Bounded category drill-down is available in the
workflow API; a drill-down conversation interface remains future work.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-21 | major | Existing-app Rust/egui Agentic interface, live-session foundation, deterministic business/recovery contracts and service handoff plan |
