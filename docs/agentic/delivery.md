# Product integration and delivery

> Last reviewed: 2026-10-21 · Last updated: 2026-10-21 · Change: major

## Extend the existing Rust application

The base interface belongs in this repository and uses Rust/egui. Classic and Agentic modes
keep the same `SheetApp`, workbook and native undo history; the mode toggle only changes
layout. The panel routes bounded actions through the live-session adapter and existing engine
commands. The business and resume contracts stay in `crates/engine`; UI rendering stays in
`crates/ui-egui`. No separate product repository or TypeScript implementation is required.

The current panel is an offline demonstration with a bounded set of typed intents and
buttons. It uses the documented synthetic sales fixture, not live business records. Its
conversation and explicit supported rules are held only in the current session. It must
identify these limits and reject unsupported requests rather than simulate model reasoning,
provider success or durable memory. The eventual Anthropic loop replaces this intent routing
while reusing inspection, validation, query semantics, provenance and action receipts.

Prefer Rust provider/database adapters in this workspace when service access exists. A web
deployment requires a trusted server for secrets and durable operations; add a Rust service
crate or a separate deployment component as needed. A different stack remains an option only
if a concrete integration requires it. Native and browser UIs should share the same typed
workflow contract. The HTTP routes below are a proposed service boundary, not implemented
endpoints or a requirement to move the interface out of this application.

Serve the browser application and API from one origin. If an external embedding host is
later added, preserve its iframe identity. Keep provider credentials and database connections
inaccessible to browser bundles. Aiven PostgreSQL will store acknowledged snapshots, enabled
rules and durable run records; the live editor remains authoritative for unsaved content.
Development memory must never silently substitute for configured production storage.

## HTTP contracts to implement

Runtime-validate all boundaries. Limit request size, number of rounds, number of tools, query
rows, speech duration, tool-result bytes and total run time. Authorize every workbook/run/rule
by authenticated owner; never accept an arbitrary client-supplied owner ID. The SQL schema is
a starting storage contract, not an authentication implementation.

| Endpoint | Input/behavior | Durable result |
|---|---|---|
| `GET /api/health/live` | Process alive | No provider query |
| `GET /api/health/ready` | Required configured services usable; bounded `SELECT 1` | No claim of inference or speech readiness without a real probe |
| `POST /api/runs/start` | Client idempotency key, workbook/session ID, user text, inspected identity | Run ID, server conversation, next outstanding tool request or final outcome |
| `POST /api/runs/:id/continue` | Session ID, exact outstanding tool-use ID, verified bridge result | Consumes once; identical retry returns same next response |
| `GET /api/runs/:id` | Read run and completed action history after interruption | Final-message outcome separate from mutation outcome |
| `GET /api/rules` | Enabled rules for owner, bounded list | Original wording and supported structured rule |
| `POST /api/rules` | Explicit remember intent and supported structure | Rule ID; unsupported intent returns a visible error |
| `POST /api/workbooks/:id/snapshots` | Snapshot ID, browser identity, expected DB version, XLSX bytes, content hash | Commit-acknowledged database version; exact retry returns prior receipt |
| `GET /api/workbooks/:id/snapshots/latest` | Owner/workbook | Bytes plus version; open must acknowledge before tools resume |
| `POST /api/speech/transcribe` | Bounded microphone upload | Editable transcript; never execute until the user submits it |
| `POST /api/speech/synthesize` | Short verified completion text | Audio; TTS failure does not undo completed edits |

Persist a request/result receipt **before** acknowledging its continuation. Use transactions
and uniqueness constraints for the owner + idempotency key, run + tool-use ID, and workbook +
browser session + snapshot ID. Two concurrent continuations must not both advance the model. A process crash
between storing a tool result and requesting the provider is resumable from that stored
result. A provider call may still be repeated after network uncertainty; no repeated provider
response may bypass browser mutation receipts. Do not promise globally exactly-once external
API calls.

Retain the complete Anthropic assistant content blocks and match each tool result to its
`tool_use_id`. Initially dispatch one browser mutation at a time. Reject unknown, duplicate,
out-of-order, wrong-run and wrong-session results; do not let a model choose arbitrary engine
commands or SQL. The pure Rust run-state reference exercises correlation and retry behavior,
but does not implement an HTTP server or durable transactions.

## Tool contracts

| Tool | Execution | Bounded responsibility |
|---|---|---|
| `query_sales_comparison` | Backend | Fixed typed periods/metric/currency/status/rules; return aggregates and provenance |
| `query_sales_detail` | Backend | Descriptive contributing records for a category; not unsupported causal conclusions |
| `read_review_rules` | Backend | Enabled supported structured rules and original wording |
| `save_review_rule` | Backend | Explicit supported remember intent, no opaque ineffective rule text |
| `inspect_workbook` | Live editor | Context and bounded range read |
| `write_summary` | Live editor | Exact rectangle, expected identity; one native operation, verified readback |
| `undo_last_operation` | Live editor | One native undo, current identity, exact readback range |

When connected, Anthropic interprets intent and selects tools. The trusted backend owns
conversation/run state and carries out that agent loop; ElevenLabs only provides transcription
and synthesized speech. The current offline panel uses deterministic routing in place of
Anthropic. Eventually typed and submitted voice transcripts enter the same `/start` workflow.
Never describe an aggregate decline as a cause; show the
comparison and offer bounded detail. Persist applied rule IDs and record counts with the run.

## External services

The following documentation was checked on 2026-10-21; no provider or deployment calls were made.

- Aiven Runtime deploys GitHub-sourced applications; new commits require an explicit redeploy.
  Connect the actual project/repository and verify the supported manifest before deployment.
  [Deployment documentation](https://aiven.io/docs/products/runtime/deploy-apps).
- Anthropic client tool results must match the emitted tool-use IDs. Preserve all content
  blocks and report tool failures as errors, separate from already completed actions.
  [Tool lifecycle](https://platform.claude.com/docs/en/agents-and-tools/tool-use/handle-tool-calls).
- ElevenLabs supports HTTP transcription and speech generation. Application WebSockets are
  unnecessary for push-to-talk. [Transcription](https://elevenlabs.io/docs/api-reference/speech-to-text/convert),
  [speech generation](https://elevenlabs.io/docs/api-reference/text-to-speech/convert).

Future server configuration should have explicit `DATABASE_URL`, trusted PostgreSQL CA/TLS
settings, `ANTHROPIC_API_KEY`, `ANTHROPIC_MODEL`, `ELEVENLABS_API_KEY`,
`ELEVENLABS_STT_MODEL`, `ELEVENLABS_TTS_MODEL`, `ELEVENLABS_VOICE_ID`, `PORT`, and
`PUBLIC_ORIGIN`. Choose available models/voices when credentials exist; there are no
hard-coded guesses or fabricated healthy adapters here. Do not commit secrets or read them
to prove this offline foundation.

## Delivery gates

| Stage | Work | Exit criterion |
|---|---|---|
| Foundation/interface | Rust/egui panel in this project, mode toggle, typed offline intents, live bridge and fixture | Same session/history survives toggle; manual/programmatic edits, formulas, export and undo verified; offline limits visible |
| Browser validation | Build the existing web app; test the panel and optional parent bridge | Browser interaction and screenshots verified; correlated open before bridge tools; reviewed commit/artifact pinned |
| Storage/deployment | Local PostgreSQL first, migrations, one-origin container, then Aiven | Transaction/retry/ownership tests; public URL and real PostgreSQL query |
| Typed agent | Anthropic adapter, durable runs, fixed sales tools | Correct first slice using actual Aiven records and enabled rule; visible verified results |
| Recovery | Snapshot upload/ack/reload, interrupted continuations, concurrent edits | Duplicate calls do not repeat edits; incomplete runs preserve successful action receipts |
| Voice | Push-to-talk, editable transcript, brief TTS | Same tested workflow from speech; provider failure does not fabricate success |
| Optional polish | Charts, reduced ribbon, extra query shapes | Core gates stay green; no new remount or history loss |

Basic recovery is part of every stage. Before provider integration, add persistence behind
the existing session-only rule/run interfaces and validate it against local PostgreSQL.
The foundation builds on the recorded base; that base commit does not pin the completed
interface or bridge. Record the reviewed final commit and artifact digest
before deployment. Charts, CLI regeneration,
realtime audio and arbitrary business data ingestion are deferred. If a bridge attempt fails,
time-box diagnosis before choosing a CLI-only product; don't build both engines concurrently.
No hour estimate is treated as reliable before live deployment and provider validation.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-21 | major | Existing-app Rust/egui interface, optional trusted service boundary, provider/storage contracts and ordered live acceptance gates |
