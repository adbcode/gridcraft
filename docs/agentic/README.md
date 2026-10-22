# Agentic spreadsheet foundation

> Last reviewed: 2026-10-22 · Last updated: 2026-10-22 · Change: major (service baseline)

The product turns business records and explicit remembered rules into an editable workbook.
This fork contains the Rust Agentic interface, live workbook bridge, deterministic business
contracts and a Rust HTTP service with PostgreSQL storage and provider adapters. The editor
still runs its labelled local demo. The service is a baseline for connecting that editor to
durable model-driven work; an end-to-end hosted agent is not complete.

## Architecture decision

Build the agentic layer in this project using its current Rust/egui stack. Classic and
Agentic are layouts of the same mounted `SheetApp` and engine `Session`: showing or hiding
the conversation panel must preserve the workbook and its native undo history. The open
editor remains authoritative for workbook content.

`apps/gridcraft-server` serves compiled WASM assets and the API from one origin, keeping
provider credentials and database access on the backend. The target is Aiven Runtime with
the user's existing Aiven PostgreSQL instance for records, rules, query evidence and workbook
files. The first version uses one fixed mocked workspace, without login. Lovable supplies
an external landing page and is outside this project's scope. No TypeScript shell or iframe
is required; an optional embedding host must retain one iframe and use protocol 2.

| Boundary | Responsibility | Foundation here |
|---|---|---|
| GridCraft native/WASM | Manual editing, formulas, native history, live workbook | Existing Rust/egui editor and engine session |
| Agentic interface | Mode toggle, typed requests, transcript, action results | Rust/egui panel in this application; deterministic offline intents |
| Rust adapter | Bounded inspection, one rectangular write, native undo, snapshot receipts | `crates/engine/src/agent_bridge.rs` |
| Business query | Typed sales, structured enabled rules, fixed periods, provenance | `crates/engine/src/review_workflow.rs` offline reference |
| Optional embedding host | Integrate the same live web editor into another application | Version 2 same-origin bridge; no separate shell required |
| Rust HTTP service | Configuration, one-origin hosting, bounded storage API | Implemented in `apps/gridcraft-server`; health and capability status are explicit |
| PostgreSQL | Typed sales, explicit rules, immutable query evidence, versioned workbook bytes | Executable migration and transactional storage; no durable run tables yet |
| OpenAI | Chat Completions interpretation and tool selection | Configurable HTTP adapter and normalized tool messages; orchestration and UI connection remain |
| ElevenLabs | STT and brief TTS of verified results | Bounded HTTP adapters; speech routes and microphone UI remain |
| Aiven Runtime | Host the Rust service and web assets | Dockerfile and runtime environment configuration; deployment is a separate validation gate |

The current conversation panel recognizes supported deterministic demo intents. It does not
call a model, interpret arbitrary requests or query business systems. Its sales records are
the fixed synthetic fixture; remembered rules and conversation state last only for the
current application session. Unsupported requests remain visibly unsupported. Future
model tools must reuse the validated engine/business contracts and match these
fixture results. Keep user content and database text as tool data; never elevate it into
agent system instructions.

The service has a separate durable path: supported rules are stored in PostgreSQL and
actually used by comparison queries. Bounded typed imports update source records and their
revision. Each completed query stores its exact input records, enabled rules, result and
provenance. Later data corrections change a new query without changing earlier evidence.
Origin labels distinguish synthetic seeding from imported rows; they do not independently
prove that imported rows came from a real business system.

## Maintain, add and retire

- Maintain the same-session editor, bounded mutation checks, native undo, snapshot identity,
  deterministic calculations and honest local-demo interface.
- Add the trusted Rust service, PostgreSQL persistence, OpenAI Chat Completions and ElevenLabs
  adapters, secure environment configuration, Make tasks and Aiven container packaging.
- Retire the Anthropic-first requirement and the proposed `schema.sql`. The executable
  [server migrations](../../apps/gridcraft-server/migrations/) are the schema authority.
  Responses and Anthropic support remain optional future adapters; CLI regeneration,
  chart polish and a separate product shell are not baseline work.

## Repository analysis and baseline

Analysis started from GridCraft commit `8ae67c5534f3d7f145423a57206cf5fd08fe9221`.
The existing foundation is pinned at `8b89f0e663ca31cd62a98b948fc4f7f0ccee8710` on
`feat/agentic-foundation`. Service work branches from it as `feat/aiven-service-baseline`.
Record the final reviewed service commit and built artifact digest before deployment or
distribution. Keep `Cargo.lock` and the pinned `craft-libs` dependency.

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
- [Operations](operations.md): private configuration, local commands and Aiven setup.
- [Service API](service-api.md): implemented routes, payloads and persistence behavior.
- [PostgreSQL migrations](../../apps/gridcraft-server/migrations/): executable schema.
- [Validation](validation.md): exact checks and remaining limitations.

For the service baseline, run `make env`, fill in the private configuration, then use
`make check-config`, `make build` and `make run`. Database migration and synthetic seeding
are separate explicit commands. Adding provider keys validates configuration only; it does
not switch the panel out of local-demo mode or perform live inference.

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
| 2026-10-22 | major | Rust service, executable PostgreSQL storage, OpenAI-first adapters, deployment tooling and explicit end-to-end gaps |
| 2026-10-21 | major | Existing-app Rust/egui Agentic interface, live-session foundation, deterministic business/recovery contracts and service handoff plan |
