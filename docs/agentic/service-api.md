# Service baseline API

> Last reviewed: 2026-10-22 · Last updated: 2026-10-22 · Change: major

The Rust server serves the built GridCraft editor and the following HTTP API from one
origin. It selects one mocked workspace from server configuration. There is no login or
per-visitor isolation: everyone who can reach this deployment shares that workspace.
Workspace IDs in request payloads are rejected. This is a demonstration boundary, not
multi-user authentication. Keep the demo's data safe to share publicly.

The editor's current Agentic panel still runs its clearly labelled local fixture workflow.
These HTTP endpoints are the persistence foundation for the future connected panel and
durable agent loop. An API save does not itself mark the browser document saved.

| Method and path | Contract |
|---|---|
| `GET /api/health/live` | Process liveness; no service calls |
| `GET /api/health/ready` | PostgreSQL connection, required schema and web index; 503 when unavailable |
| `GET /api/status` | Configured capabilities and explicit offline/UI/mock-user status; no secrets or provider probes |
| `GET /api/rules` | Bounded saved rules, including original wording and enabled state |
| `POST /api/rules` | `{ "id": "exclude-tests", "instruction": "Remember to exclude test orders" }`; exact replay succeeds, conflicting wording returns 409 |
| `POST /api/sales/import` | `{ "records": [Sale, ...] }`; atomic upsert of at most 1,000 records, at most 10,000 stored per workspace |
| `POST /api/sales/compare` | `{ "periods": ComparisonPeriods }`; returns review, editable `sheet_values` and `evidence_url` |
| `GET /api/queries/{query_uuid}` | Immutable result, source revision/time, exact input rows, source origins and rule context |
| `GET /api/workbooks` | Bounded workbook list with latest database version |
| `PUT /api/workbooks/{workbook_uuid}` | `{ "title": "Weekly sales" }`; idempotent creation, changed title under the same ID conflicts |
| `POST /api/workbooks/{workbook_uuid}/snapshots` | Multipart `metadata` JSON and `file` bytes; atomic commit receipt |
| `GET /api/files?q=` | Searchable library cards: workbook ID, title, `origin` (`mock` for the three seeded mock workbooks), latest version and SHA-256 |
| `GET /api/files/{workbook_uuid}/versions/{version}/preview` | Bounded first rows of each sheet of that exact version |
| `GET /api/workbooks/{workbook_uuid}/snapshots/{version}` | Exact stored bytes of that version |
| `POST /api/runs/start` | `RunStart` (request ID, workbook session, prompt, versioned attachments, bridge context); same request ID and body replays, different body returns 409 |
| `POST /api/runs/{run_uuid}/continue` | `RunContinue` for the one outstanding tool call; identical retry replays without a second receipt, a different outcome or tool call ID returns 409 |
| `GET /api/runs/{run_uuid}` | `RunView`; a `running` run idle for more than 90 s is reported `interrupted` |
| `POST /api/runs/{run_uuid}/resume` | Continues an interrupted run from its persisted conversation |
| `POST /api/speech/transcribe` | Raw audio body (`audio/webm`, `audio/ogg`, `audio/wav`, `audio/mpeg`, `audio/mp4`; at most 10 MiB) → `{ "text", "language_code" }`, editable text only; never starts a run |
| `POST /api/speech/synthesize` | `{ "text": "..." }`, at most 600 bytes of a verified summary → `audio/mpeg` |
| `GET /api/workbooks/{workbook_uuid}/snapshots/latest` | Saved bytes; `x-gridcraft-storage-version` and `x-gridcraft-sha256` headers identify the returned snapshot |

JSON requests are capped at 1 MiB. Snapshot bytes are capped at 16 MiB, metadata at 16 KiB,
and the multipart envelope at 16 MiB + 64 KiB. Requests have a 30-second response deadline.
Timeout or connection loss after a database commit is an uncertain acknowledgement: retry
the identical snapshot request to recover its receipt. No route executes arbitrary SQL or
arbitrary engine commands. Browser requests from a different origin are rejected.

## Data and evidence

Use `make db-seed-demo` explicitly to load the 12 synthetic records. Startup and migration
never seed data or enable a remembered rule. The import schema is the engine's `Sale`:

```json
{
  "records": [{
    "id": "public-demo-order-1",
    "category": "Jackets",
    "timestamp_utc": 1791763200,
    "amount_minor": 6000,
    "refunded_minor": 0,
    "currency": "EUR",
    "status": "paid",
    "is_test": false
  }]
}
```

Import is an explicit upsert by record ID, with the last accepted changed input becoming
the current source record. An identical repeat does not advance the source revision.
Invalid records or a capacity violation roll back the whole batch. Imports are not a
scheduled connector, automatic refresh or proof of a real business source. Evidence labels
each record `synthetic_demo` or `imported`; identical imports preserve its existing label.
There is no record deletion or rule editing endpoint in this baseline.

The fixed comparison request is:

```json
{
  "periods": {
    "previous": { "start_utc": 1791158400, "end_exclusive_utc": 1791763200 },
    "current": { "start_utc": 1791763200, "end_exclusive_utc": 1792368000 }
  }
}
```

The calculation uses paid orders, EUR cents, full consecutive Monday-to-Monday UTC weeks,
and gross minus recorded refunds attributed to the original order date. The engine applies
the supported structured rules to a bounded set of database rows; the model does not write
SQL. Source rows, source revision and rule context come from one repeatable-read transaction.
Queries persist both included and excluded input rows so the reported source count and
filtering can be explained. Later imports or rule changes never rewrite old query evidence.
`queried_at_utc` describes when that database snapshot was queried, not upstream freshness.

For the demo, compare without the rule (current total 65,500 cents), save the exclusion
rule, then compare again (15,500 cents). Change a known source record through import and
compare again: the answer and source revision must change while the earlier evidence stays
intact. The [fixture](sales-fixture.md) defines the exact baseline answers. These deterministic
checks are evidence of data correctness; they do not measure model reasoning or user time saved.

## Snapshot receipt

Capture bytes and identity together using the live editor's snapshot API. Register a stable
workbook UUID. The multipart `metadata` field is:

```json
{
  "export_id": "export-1",
  "workbook_session_id": "browser-session-1",
  "identity": { "document_id": 1, "sheet_index": 0, "sheet_name": "Sheet1", "change_token": 1 },
  "expected_version": 0
}
```

`expected_version: 0` means the first saved version. The service calculates SHA-256 from
the actual bytes and serializes saves by locking the workbook row. The receipt contains
`workbook_id`, `export_id`, `workbook_session_id`, `identity`, `storage_version`, `sha256`
and `byte_length`. It is returned only after the transaction commits.

An exact retry of the same session/export ID recovers the original receipt, even after a
newer version is saved. Reusing that ID with different bytes, identity or expected version
returns 409. Two different saves against the same expected version cannot both succeed.
Bytes are stored opaquely; a successful download is not an acknowledgement that the editor
opened them. Forward the exact successful receipt to the existing browser snapshot ack
contract; later edits must remain dirty. Reopening requires the bridge's correlated load
acknowledgement before agent actions resume. This client wiring remains to be implemented.

## Provider boundary

`providers.rs` supplies configurable OpenAI Chat Completions and ElevenLabs HTTP adapters.
They validate bounded messages and tool-result IDs, preserve assistant tool-call messages,
use deadlines, and disable redirects and automatic retries. Provider errors expose only a
safe classification/status, never credentials, URLs or raw response bodies. They make no
network request during server startup or `check-config`.

There is deliberately no public generic completion endpoint. Durable `/api/runs/start`,
`/continue`, recovery and speech routes are the next stage, as specified in
[delivery.md](delivery.md). Typed input and submitted voice transcripts will use the same
run workflow; transcribing speech alone will not execute a workbook action.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-22 | major | Runnable Rust service, durable source/evidence/rule/snapshot APIs and explicit integration boundaries |
