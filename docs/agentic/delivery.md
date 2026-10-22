# Product integration and delivery

> Last reviewed: 2026-10-22 · Last updated: 2026-10-22 · Change: major

The product must turn changing business data and explicitly remembered rules into a correct,
editable workbook. Its first complete slice is: retrieve the saved test-order exclusion,
compare two defined periods, inspect the live sheet, apply and verify a summary, explain its
sources, and allow native undo. A convincing demonstration also changes a source record,
produces a changed answer, and shows why the earlier answer was correct for its earlier data.

The initial customer hypothesis is a small retail or ecommerce operator preparing a weekly
category-sales review in a spreadsheet. The value to test is a correct, editable review that
retains their explicit working rules and explains changes using source records. For the
required pitch, recruit a few operators or their accountants to try that single journey;
record completion time and correction count against their current process before claiming
time savings. The landing page and pitch slide are separate deliverables, not this baseline.

## Current scope and decisions

Keep the existing Rust/egui application and live engine session. Classic and Agentic are
layouts of the same `SheetApp`; toggling must retain workbook content, pending previews and
native history. The local panel's deterministic intent routing remains visibly a demo until
its service workflow is connected. Panel rules currently last only for that editor session.

The trusted Rust server in `apps/gridcraft-server` serves web assets and HTTP APIs from one
origin. Aiven Runtime is the hosting target, and the already-created Aiven PostgreSQL service
stores data and files. The baseline uses a fixed server-selected mock workspace without login.
Lovable is a separate landing page outside this repository's scope. OpenAI Chat Completions
is the first required model API; Responses and Anthropic adapters remain optional.
ElevenLabs handles transcription and synthesis through ordinary HTTP calls.

Maintain the editor, bridge, deterministic query semantics and recovery protections. Add the
service boundary and connect it incrementally. Retire the old Anthropic-first assumption,
the separate TypeScript-shell prerequisite and the duplicate draft SQL schema. The executable
[server migrations](../../apps/gridcraft-server/migrations/) now define implemented storage.
Do not add speculative run tables or claim browser integration merely because adapters exist.

## Implemented baseline and remaining boundaries

| Area | Implemented | Remaining |
|---|---|---|
| Editor | Local Agentic panel, live inspection/write/verify/native undo, persistent mode toggle | HTTP client and durable workflow integration |
| Service | Rust HTTP server, static hosting, liveness/readiness/status, bounded same-origin API | Aiven deployment validation and product connection |
| Data | PostgreSQL migration; explicit synthetic seed; bounded typed sales import and comparison; saved supported rules | Real business-source mapping and end-to-end data-driven demonstration |
| Evidence | Stored query timestamp/revision, exact rows, ingestion origins, rules, result and provenance | Display and drill-down from the connected conversation |
| Files | Versioned XLSX bytes, exact-retry snapshot receipts, expected-version conflicts | Browser upload/acknowledgement/reload coordination |
| Models | OpenAI Chat Completions adapter, configurable endpoint/model, correlated normalized tool messages | Durable multi-step loop, persisted conversation and browser tool dispatch |
| Voice | ElevenLabs STT/TTS HTTP adapters | Speech routes, push-to-talk, editable transcript and playback |
| Operations | Private dotenv configuration, Make tasks, one-origin Dockerfile | Successful container/deployed checks recorded separately |

See [the service API](service-api.md) for implemented endpoints and [operations](operations.md)
for running them. Configuration and adapter construction do not call providers. Readiness
checks database/schema and web assets; it does not claim model or speech connectivity.
[Validation](validation.md) records actual tests, including their local/mocked/live boundaries.

## Prove that data and memory drive the result

Use the documented [synthetic fixture](sales-fixture.md) for deterministic acceptance first.
Keep periods, EUR cents, paid-order status and refund attribution fixed while changing one
factor at a time:

1. Seed the twelve original synthetic records explicitly. Without a saved exclusion rule,
   current-period revenue is **65500 cents**, versus **15000 cents** previously.
2. Explicitly save “Remember to exclude test orders.” The current total becomes **15500 cents**
   with three qualifying current records. Restart the service or create a fresh connection;
   retrieving and applying the saved rule must produce the same result.
3. In a controlled source correction, change `c-jackets.refunded_minor` from `1000` to `2000`.
   A new query with the saved rule must produce **14500 cents** current revenue and **5000 cents**
   for Jackets. Its source revision changes; an identical import retry does not change it again.
4. Retrieve the earlier query by its ID. Its **15500-cent** answer, original refund amount,
   source rows and rule context must remain unchanged. Explain the difference using that
   evidence, rather than reconstructing an old answer from today's mutable records.
5. Through the connected editor workflow, write the new comparison with live formulas and
   provenance, verify affected cells, and undo the last native operation. Editing the summary
   manually must remain possible throughout the same mounted workbook session.

Steps 1–4 exercise storage behavior; step 5 and model-led multi-step execution are still
integration work. Do not present API tests as a completed product demonstration. Record
expected and observed totals, included row counts, preserved rule/evidence IDs and whether
manual correction was required. This supplies a measurable comparison against the unfiltered
answer, not a claim of general accuracy from one fixture.

The import endpoint accepts typed records, not arbitrary SQL. `synthetic_demo` and `imported`
are ingestion labels; an imported synthetic row is still synthetic. Demonstrating real business
data requires a documented source/export and field mapping. Preserve that distinction in the
interface and the demonstration. Aggregates show the size of changes; contributing records
support descriptive drill-down, not a causal claim about customer demand.

## Durable typed agent loop to implement next

The Rust backend owns conversation state, outstanding tools, authorization and retry/recovery.
OpenAI interprets the request and selects bounded tools; it never receives unrestricted SQL or
engine execution. The existing `RunState` is an in-memory reference, not durable orchestration.

The planned routes below are **not implemented**:

| Endpoint | Responsibility |
|---|---|
| `POST /api/runs/start` | Accept client request ID, workbook/session identity and user text; save a run and its initial context |
| `POST /api/runs/:id/continue` | Consume the exact outstanding tool-call result once; replay an identical continuation |
| `GET /api/runs/:id` | Recover status, completed actions and outstanding work after interruption |
| `POST /api/speech/transcribe` | Return a bounded editable transcript; execution waits for explicit submission |
| `POST /api/speech/synthesize` | Speak a short verified result; synthesis failure cannot erase completed edits |

Store the request and result receipt before acknowledging a continuation. Add transactional
uniqueness for workspace/request ID and run/tool-call ID, with one outstanding browser mutation
at a time. Reject wrong-run, wrong-session, stale, unknown and out-of-order results. Concurrent
continuations must not both advance the run. Preserve the complete provider assistant message
and associate each tool result with its emitted tool-call ID; normalized adapter types retain
these details. Provider-specific wire formats belong in adapters, not workbook code.

Bound model rounds, tool count, query rows, payload sizes and total run time. A provider timeout
after successful edits is an incomplete conversation with recorded effects, not an unchanged
workbook. Resume from stored evidence and exact browser mutation receipts. A provider request
may repeat after network uncertainty; do not promise globally exactly-once external API calls.

| Tool | Execution | Responsibility |
|---|---|---|
| `query_sales_comparison` | Backend | Validated periods/metric/currency/status and saved rules; return aggregates and immutable evidence ID |
| `query_sales_detail` | Backend | Bounded contributing records from the relevant query evidence |
| `read_review_rules` | Backend | Retrieve enabled supported rules and original wording |
| `save_review_rule` | Backend | Persist an explicitly requested supported rule |
| `inspect_workbook` | Live editor | Read context and bounded ranges |
| `write_summary` | Live editor | Apply one validated rectangle under the inspected identity and verify readback |
| `undo_last_operation` | Live editor | Undo one native operation and inspect the affected range |

A prepared write must fail on a conflicting manual edit, wrong document or sheet. Preserve
unfinished cell drafts and report busy instead of replacing them. Do not refresh the expected
identity behind the user's back. Native undo may undo a more recent manual operation; it is
not conversation-wide rollback.

## Snapshot completion

The server can store files, but the browser still owns the open workbook. Complete this path:

```text
Export identified live snapshot
→ upload bytes with session/export ID and expected database version
→ commit and receive the exact snapshot receipt
→ acknowledge that snapshot in the editor
```

Do not mark the document saved on export or before commit. Later browser edits must remain
dirty after acknowledgement. Exact retries return the original receipt even after a later
save; conflicting bytes, identity or preconditions under the same export ID must fail.
Database snapshot versions, source-data revisions and live workbook change tokens are separate.
On reload, wait for correlated workbook-open success before allowing new agent tools.

## Delivery gates

| Stage | Exit criterion |
|---|---|
| Service/data baseline | Local storage, retry, version and scope checks recorded; fixture comparisons and immutable evidence correct |
| One-origin deployment | Reviewed image runs on Aiven; public editor, schema readiness and existing PostgreSQL reads/writes verified |
| Typed agent and recovery | Saved rule survives restart; model performs query → inspect → apply → verify; changed source changes the next answer; interruption retains receipts |
| Saved workbook recovery | Upload acknowledgement leaves later edits dirty; reload restores committed bytes and correct session gating |
| Voice | Editable transcript enters the same durable workflow; verified completion spoken truthfully |
| Optional polish | Charts, reduced ribbon and extra query shapes preserve the core guarantees |

Basic recovery belongs in every stage. Record the reviewed service commit and image digest
before deployment. Keep API keys in runtime configuration and out of build inputs. Use
[operations](operations.md) for Aiven's explicit redeploy behavior. Actual provider availability,
container deployment and hosted microphone behavior require their own validation; no fixed
hour estimate or local green test proves those gates.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-22 | major | OpenAI-first Rust service, real persistence/evidence baseline and measurable changing-data acceptance before durable agent and voice integration |
| 2026-10-21 | major | Existing-app Rust/egui interface, optional service boundary and ordered live acceptance gates |
