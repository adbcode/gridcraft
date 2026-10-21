# Parent bridge protocol, version 2

> Last reviewed: 2026-10-21 · Last updated: 2026-10-21 · Change: major

Mount `gridcraft/?host=parent` in a same-origin iframe. Keep that DOM node mounted when
changing modes. Check both `event.origin === location.origin` and
`event.source === iframe.contentWindow` in the product; GridCraft likewise checks the exact
parent window and origin. Provider credentials belong only in the backend.

This is a versioned change to the old open/save bridge. Hosts must adopt identifiers and
save acknowledgements; a legacy `gridcraft:open` without them is rejected. There is no
fallback that silently marks transferred bytes persisted.

## Connection and load

Listen before loading the iframe. On `gridcraft:ready`, require `protocol: 2` and retain
`session_id` (fresh cryptographic UUID for this mounted editor). A parent that missed ready
may send `{ "type": "gridcraft:hello" }`. Ready means the listener exists, not that a workbook
has loaded. The session ID prevents replay into a remounted editor; it is not authentication.

Send a plain postMessage object, with `bytes` an ArrayBuffer or Uint8Array:

```json
{"type":"gridcraft:open","id":"open-1","session_id":"<ready session>","name":"review.xlsx","bytes":"<binary, not a JSON string>"}
```

Wait for `gridcraft:opened` with the same `id`, `ok: true` and
`result: { opened, context }`. A parse failure returns `gridcraft:open-error`, `ok: false`,
`error: { code, message }`; it does not authorize editing the intended new workbook. A
previously open workbook remains available if the new file fails. Invalid and encrypted
files return errors; programmatic calls do not open password/file dialogs.

A successful load identifies the active document through `result.context.identity`:

```json
{"document_id":1,"sheet_index":0,"sheet_name":"Sheet1","change_token":1}
```

## Requests

```json
{"type":"gridcraft:request","id":"inspect-1","session_id":"<ready session>","method":"inspect","params":{"expected":{"document_id":1,"sheet_index":0,"sheet_name":"Sheet1","change_token":1},"range":"A1:C3"}}
```

Responses echo `id` and `session_id`, use `type: "gridcraft:response"`, and contain either
`{ ok: true, result }` or `{ ok: false, error: { code, message } }`. Do not infer success from
an HTTP response or a model's text. Use the actual bridge result and readback.

| Method | Params | Meaning |
|---|---|---|
| `context` | `{}` | Identity, selection, active cell, dirty, ready, busy, calculation mode, undo availability |
| `inspect` | `{expected, range}` | Cells, formulas, cached calculated values and context from the active sheet |
| `set_values` | `{expected, range, values}` | One exact rectangular `range.setValues`; values are JSON scalar cells, formulas start with `=` |
| `undo` | `{expected, range}` | One native `edit.undo`; read back the requested range |
| `snapshot.export` | `{expected}` | Capture/export immutable XLSX; result has snapshot ID and identity, top-level `bytes` holds binary |
| `snapshot.ack` | `{snapshot_id, storage_version}` | Acknowledge durable storage of those exact bytes; storage version is an opaque nonempty string |

For writes use an exact rectangle, e.g. `range: "A1:B2"`, `values: [[1,2],[3,"=SUM(A1:B1)"]]`.
The envelope ID is also the adapter's mutation request ID; callers need not supply it again.
The adapter accepts no arbitrary engine command, filesystem path, URL or dialog request.

Tokens are intentionally conservative. The adapter retains the observed workbook Arc and
compares it, document identity, selection and edit mode each frame. Manual edits, explicit
recalculation, sheet changes, renames and view-state snapshots can invalidate a token even
where the engine revision does not change. Inspect/context again after a conflict; never
silently substitute a fresh token for an old proposed edit. Sheet index and exact name
are identities only within the token, not permanent database keys. The token includes the
whole workbook rather than only the requested cells, so unrelated edits can also conflict.

Mutations run synchronously after the frame's manual UI input, with no await between check,
write and readback. An unfinished cell/name/text-box edit or dialog gives `busy`; it is not
committed or discarded by an agent. The UI continues to accept edits while the backend thinks.
Only native last-operation undo is offered; it may undo the user's most recent edit.

A successful write reports `applied: true` and `readback`. If a write succeeded but readback
could not be returned, `applied: true` with `verification_error` is an incomplete verification,
not a rollback. Do not speak a completion message until results have been verified. Returned
formula errors are values to inspect, not proof of correct business results. Inspection
reports the calculation mode; `set_values` rejects manual calculation (`manual_calculation`)
until automatic calculation is enabled, so stale caches cannot be presented as verified edits.

## Retry and bounds

Keep IDs stable when retrying the same request. Stateful requests (including failures) are
remembered for the mounted editor lifetime. An exact repeat returns the original result;
reusing its ID for different input is rejected. A stale/error retry needs a **new** ID after
reinspection. IDs and tokens must also be retained by the backend's durable tool records.
A browser reload ends this in-memory guarantee; never automatically repeat an uncertain
mutation into a new session.

Bounds: 1,000 cells per read/write; 64 KiB adapter JSON; 16 KiB per input cell;
256 KiB transport JSON; 16 MiB incoming files/host exports; eight queued requests;
64 lifetime native mutation receipts, 128 lifetime stateful transport receipts;
four explicit exported byte payloads cached for retries; four pending snapshots.
Ledgers refuse overflow instead of evicting duplicate protection. These are foundation
limits, not large-workbook production targets. Save/export before reaching session limits;
download recovery remains a product integration requirement. Reopening a file does not
reset the receipt ledger, but remounting does and changes the session ID.

## Persistence

Both explicit snapshot export and user-initiated XLSX Save capture the exact workbook at
export time. User Save emits `gridcraft:save` with `snapshot_id`, `identity`, `name`,
`session_id`, and binary `bytes`. The UI's export callback now receives the document itself,
so it never guesses which document was exported at the end of a frame.

1. Store bytes, snapshot ID, workbook/session identity, content hash and a separately allocated
   database version in one acknowledged database transaction.
2. Only after commit, send `snapshot.ack` for that snapshot and storage version.
3. GridCraft updates the saved baseline to the captured workbook, not the current workbook.
   Edits during upload stay dirty. A stale/out-of-order acknowledgement cannot replace a
   newer saved baseline. Duplicate acknowledgements do not mutate the workbook again.
4. On failure, retain pending bytes for retry and show unsaved state. Never acknowledge an
   HTTP timeout as success. Do not mark a snapshot saved merely because postMessage worked.

`gridcraft:dirty` reports aggregate unsaved status. Non-XLSX exports emit `gridcraft:export`
and never mark a workbook saved. GridCraft trusts the same-origin host's storage assertion;
this protocol cannot prove a database write itself.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-21 | major | Protocol 2, correlated loading, narrow mutation adapter and acknowledged snapshots |
