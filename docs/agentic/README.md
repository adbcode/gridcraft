# Agentic spreadsheet workspace

> Last reviewed: 2026-10-22 · Last updated: 2026-10-22 · Change: major (connected workspace)

GridCraft turns business records and explicit working rules into an editable workbook.
Classic and Agentic are layouts of one mounted Rust `SheetApp` and live engine `Session`.
The browser connects to the Rust server from the same origin. Native builds retain the
provider-free local demonstration; connected storage and voice run in the browser shell.

Agentic mode has a slim filename/save header, Files, Rules, native Undo and Classic controls,
a floating composer, and one response card with expandable sources and completed actions.
It retains direct cell editing, sheet tabs, selection, drafts and history across mode changes.
Previews need Apply or Discard; errors remain visible until acknowledged. Agentic mode hides
the ribbon, persistent formula bar and auxiliary panes.

## Workbooks and sources

Without explicit files/samples or recovered bytes, startup generates **Weekly Sales.xlsx**
with GridCraft's writer and opens it through the ordinary loader in Classic mode. Review,
Orders, and Sources and Checks contain the twelve original synthetic records and completed
UTC weeks. Revenue starts at **EUR 150 previous / EUR 655 current**, without an exclusion
rule. Editable formulas, EUR amounts and fractional percentage growth are preserved by XLSX
round-trip. The pure business contracts continue to calculate in integer cents.

`seed-demo` explicitly initializes the twelve backend records and three versioned XLSX
library files: Weekly Sales, Quarterly Forecast and Inventory. It never enables a rule or
overwrites a saved file. Local standalone file cards are visibly labelled local mock sources;
service cards refer to actual stored versions. Attachments bind workbook ID and version.
The model inspects live cells for questions about the editor, and exact stored versions for
attachments. Backend sales queries explicitly name `workspace_sales_records`, a separate
dataset. None of these ingestion paths independently verifies business provenance.

## Connected workflow

The Rust backend owns model conversations, tool authorization, checkpoints and recovery.
OpenAI Chat Completions is the first adapter, with configurable endpoint/model and normalized
messages. ElevenLabs supplies transcription and synthesis; it does not control workbooks.
Typed input and submitted editable transcripts use the same request path.

The model can request current context, bounded inspection, one rectangular write with
readback, native undo, durable rule retrieval/persistence, typed sales comparisons, immutable
query evidence, versioned file inspection and exact snapshot saving. No unrestricted engine
execution, formatting/chart/table tools or alternate workbook controller is exposed.

Each mutation retains the inspected document/sheet/change identity and a stable retry ID.
It fails if manual edits, selection, sheet, calculation or document identity changed, or an
unfinished editor interaction exists. Writes and model-requested undo pause for user review.
Only actual completed effects enter action counts. Native Undo means the last operation,
which may be a manual edit; it does not undo an entire conversation.

Durable runs persist complete assistant messages, correlated results and outstanding editor
tools in PostgreSQL. Stable server-tool IDs recover uncertain query commits. Provider errors
retain prior completed effects. Snapshot export, upload, committed receipt and exact editor
acknowledgement are separate steps; edits after export remain dirty.

The browser keeps a bounded same-tab recovery snapshot and outstanding requests/receipts in
session storage. Reload opens recovered bytes before the default. It creates a new editor
identity and never replays uncertain old mutations into it. Recorded results can be delivered
without executing their tools again; unknown effects require inspection and a new request.
Reload restores workbook content, not native undo history. Unfinished cell drafts are never
implicitly committed for recovery. Workbooks above the browser recovery limit need explicit
cloud saving or download.

## Voice

Microphone starts a bounded recording (60 seconds / 10 MB); Finish transcribes into the
editable composer and never submits it automatically. Cancel stops tracks and suppresses
transcription/submission. Send enters the same harness as typing. Read outcome aloud fetches
a short deterministic outcome based on persisted completed actions, then offers play/pause.
Continuous ElevenLabs Agents integration is deferred pending live provider/HTTPS validation
against these same contracts.

## Operations and evidence

Use `make env`, private backend configuration, `make check-config`, `make build`, and
`make run`. Migration and synthetic seeding are explicit operations. Process environment
wins over dotenv. No credentials enter browser assets. The server selects one mocked
workspace without login; client-selected owners are rejected.

- [Workspace behavior and recovery](workspace.md)
- [Delivery and live gates](delivery.md)
- [Service API](service-api.md) and [operations](operations.md)
- [Bridge contracts](bridge-protocol.md), [sales fixture](sales-fixture.md)
- [Observed validation](validation.md)

The implementation starts from service baseline `2bfd1f9cb9a876bd4773d0147d52f927ad106baf`
on `feat/agentic-workspace`. It does not deploy or switch the existing Aiven baseline.
Shared `../../craftrules` and ignored local `plan/` remained unavailable; repository guidance
and committed contracts were used. No sibling code or vendor artwork was copied.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-22 | major | Connected workspace, generated files, durable runs, exact saving and browser speech |
| 2026-10-22 | major | Rust service and PostgreSQL/provider baseline |
| 2026-10-21 | major | Same-session local agent foundation |
