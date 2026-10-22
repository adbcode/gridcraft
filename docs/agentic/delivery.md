# Workspace delivery and remaining live gates

> Last reviewed: 2026-10-22 · Last updated: 2026-10-22 · Change: major

The first product slice compares two completed UTC weeks, explicitly remembers a supported
exclusion, writes and reads back an editable summary, retains provenance, and permits native
last-operation undo. The connected browser and Rust backend now implement that path.

| Checkpoint | Implementation | Validation boundary |
|---|---|---|
| Interface/default file | Same-session layout; generated original XLSX; editable formulas/EUR/percentages | Engine round-trip, UI tests and compiled browser |
| Library/harness | Search/open/attach exact versions; bounded Chat Completions tools; explicit backend dataset | Local HTTP mock model and real disposable PostgreSQL |
| Persistence/recovery | Durable run checkpoints/receipts; immutable evidence; exact snapshot acknowledgement; same-tab reload | Transaction/retry tests and browser acceptance |
| Voice | Backend STT/TTS; bounded microphone capture; editable transcript; cancel and playback | Provider adapter mocks and fake browser microphone; live providers remain a gate |

## Data-driven acceptance

The documented twelve-row synthetic fixture starts at **15000 / 65500 cents**. Explicitly
remembering the test-order exclusion changes the current total to **15500 cents** and current
qualifying rows to three. Persisted rules must survive a new service connection. Changing
`c-jackets.refunded_minor` from 1000 to 2000 changes a new answer to **14500 cents** without
rewriting the earlier 15500-cent evidence, records, rule context or source revision.

The workbook tool payload uses EUR (150 / 155), while database business contracts use integer
cents. Retain periods, metric, query/rule IDs, source count and qualifying row counts. Backend
sales are a separate named dataset, not a substitute for inspection of the live editor or
an attached file. Aggregates and drill-down describe contributors, not causes.

## Recovery contract

Persist the assistant turn before executing its queued tools. Server effects use stable IDs;
editor effects return correlated receipts. Exact retries replay results before fresh context
checks, and altered retries conflict. A provider failure after a write retains that write's
receipt and action count. Resume continues the same run rather than repeating its effects.
A reloaded editor gets a fresh identity: uncertain old mutations need inspection/new intent.
Recorded results and exact snapshot uploads can be retried without re-executing workbook edits.

Browser recovery is same-tab session storage, bounded to 3.5 MB of XLSX bytes. It waits for
unfinished editor interactions, so an uncommitted draft is not automatically saved. Restoring
bytes resets native history; only the currently mounted session promises native undo. A cloud
version conflict preserves local content and the original retry payload; resolving competing
cloud edits needs an explicit new version/copy workflow in a later iteration.

## Next live gates

1. Review/commit/push this isolated branch; the user then selects it in Aiven. No current
   deployment, original checkout or in-progress baseline image build is changed by this work.
2. Build the one-origin image and test Aiven PostgreSQL TLS/migrations/readiness using the
   selected branch. Local database success is not deployed evidence.
3. With backend-only real credentials, check model availability, an actual multi-step run,
   provider failures, live ElevenLabs transcription and synthesis, and HTTPS microphone rules.
4. Validate public-host browser recovery, broader platforms, and the full repository CI gate.
   Keep unrelated baseline failures visible rather than weakening repository lint policy.
5. Assess continuous ElevenLabs Agents against the existing bounded tools after push-to-talk
   passes. Do not introduce another workbook controller.

Authentication, multiple workspaces, arbitrary spreadsheet/MCP tools and continuous voice
remain outside this acceptance slice. Lovable remains an external landing page.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-22 | major | Delivered connected checkpoints; isolated local validation and remaining Aiven/provider gates |
