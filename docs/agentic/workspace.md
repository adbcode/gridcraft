# Workspace contracts and recovery

> Last reviewed: 2026-10-22 · Last updated: 2026-10-22 · Change: major

`crates/engine/src/workspace.rs` contains shared run, attachment, snapshot and speech
contracts plus the original demo XLSX generator. `apps/gridcraft-server/src/runs.rs` owns
orchestration; `apps/gridcraft-web/src/workspace_client.rs` owns same-origin delivery and
executes allowed tools through the mounted editor's persistent `BridgeState`.

## File lifecycle

On ordinary browser startup, an explicit sample wins over same-tab recovery. With neither,
the generated Weekly Sales XLSX is opened with `file.open`. Parent-host mode waits for its
correlated parent load. Native command-line files/samples win over the default. Local picker
and drop opens use the existing loader and do not silently replace unfinished interactions.

Mock library seeding is explicit, idempotent and never overwrites an existing snapshot. File
cards preview actual stored content. Open and Attach refer to an exact workbook/version;
attached contents are bounded source data, not instructions. The library currently lists the
first 50 stored workbooks; searching filters that loaded set. Local standalone cards are
labelled local mock sources. Document attachments support XLSX only.

The open editor and `workspace_sales_records` are separate authorities. A question about
live cells uses inspect_range; an attachment query uses its exact stored version; a sales
query uses the named backend dataset and retains immutable evidence. Synthetic and imported
are ingestion labels, not independently verified real business provenance.

## Run lifecycle

A StartRun carries a stable client-generated UUID, unique mounted editor session, prompt,
context and exact attachment versions. An identical start recovers the same run; altered
input under that ID conflicts. The backend persists the assistant message and queued tools
before execution, then checkpoints every bounded step (24 maximum). A row lock serializes
advances; duplicate provider call IDs are rejected before another tool executes.

Server comparisons use stable run/tool-derived query IDs. After an uncertain commit, retry
retrieves the original immutable evidence. Remembering a supported rule also checks the
original user request, not only model arguments. Workbook tools return correlated pending
dispatches; only context/inspection execute automatically. Writes and undo pause for review.
Save requires the explicitly observed Identity and exact captured bytes.

ToolContinuation echoes the editor session and outstanding call ID. An exact result retry
is accepted; a changed result conflicts. Completed actions remain if final provider text
fails. Bridge failures count as zero effects, and applied-with-verification-error remains an
applied effect with incomplete verification. Native Undo affects the latest native operation,
including manual edits. No unrestricted engine, chart, table or formatting tools are exposed.

## Snapshot and reload lifecycle

Save captures immutable workbook Arc, editor Identity and export UUID before encoding bytes.
The upload includes expected storage version and mounted editor session. The database commits
bytes and a receipt atomically. The browser checks workbook/export/session/identity, SHA-256,
length and allocated version, then acknowledges the exact capture. Later edits remain dirty;
older acknowledgements never lower the saved baseline or storage version. A failed upload
retains the exact request for retry. An expected-version conflict requires explicit resolution;
it never overwrites newer cloud data or discards local edits.

Session storage holds bounded same-tab recovery bytes, cloud binding, outstanding start,
continuation and save payloads. Recovery waits for a finished/cancelled cell interaction and
never implicitly commits a draft. XLSX above 3.5 MB needs explicit save/download. Storage
quota/unavailability is visible. Reload opens recovered bytes before the default and allocates
a fresh mounted editor identity. It restores content without native undo history. Old pending
mutations are inspectable, never automatically re-executed; a recorded receipt may be delivered
without redoing its effect. A recovered snapshot receipt does not mark new-session edits clean.

## Voice lifecycle

Recording uses browser MediaRecorder in Rust, with a 60-second/10-MB bound and track cleanup.
Finish sends raw supported audio to the backend ElevenLabs adapter. The transcript enters the
editable composer and requires Send; any concurrent composer draft is preserved. Cancel
invalidates the recording generation, stops tracks and suppresses transcription/submission.
Submitted transcripts and typing use the same StartRun path. TTS accepts a run ID, derives a
brief outcome from persisted effects/readback, and returns MP3 for explicit playback/pause.
Provider configuration alone is not proof of connectivity. Continuous voice agents are deferred.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-22 | major | Same-session workspace, file/run/storage/speech contracts and bounded recovery |
