# Agentic core

## Baseline and scope

This extraction starts from baseline commit
`8ae67c5534f3d7f145423a57206cf5fd08fe9221`. The repository keeps the spreadsheet domain and engine
while removing the classical native desktop interface and behavior that depended on it.

The retained engine/domain crates are `core`, `numfmt`, `formula`, `functions`, `model`, `calc`,
`xlsx`, `chart`, `pdf`, and `engine`. Their capabilities include workbook modeling, formulas and
recalculation, cell and range operations, formatting, tables, PivotTables, charts, print settings,
PDF output, and spreadsheet import/export. This is a capability map, not a claim of Excel parity.

The supported surfaces are:

- `gridcraft-cli` for local headless operations.
- `gridcraft-mcp` for in-process MCP over stdio.
- `gridcraft-web` for a browser preview and host-mediated command interface.

No LLM, agent provider, inference service, or remote execution backend is connected by this
repository. The MCP and browser protocols expose tools and commands to a client; they do not make
agent decisions.

## Capability policy

`engine::agent_command_specs`, `engine::agent_commands`, and `engine::is_agent_command` provide a
shared policy for headless command discovery and execution. Pure editor or external-interaction
entry points are not advertised or accepted by headless execution. This includes beginning a cell
edit, opening the function picker, and following an external hyperlink. Explicitly parameterized
operations remain usable when they have a meaningful engine implementation, even if a menu button
would otherwise open a dialog. Examples include formatting, Solver/Analysis ToolPak operations,
and unhide operations.

`Session::execute` remains UI-inclusive. `Session::execute_headless` reports unresolved `Dialog`,
`EditCell`, and `OpenUrl` requests as errors rather than returning a misleading success; informational
`Message` requests do not turn a successful operation into a failure. `file.options` returns
calculation settings without opening a dialog.

The browser bridge offers bounded `context`, `commands`, `inspect`, `set_values`, `undo`, and
generic `execute` operations, along with snapshot capture and acknowledgement. Mutations use
document/sheet/change-token identity checks and a bounded receipt ledger: an exact request retry
replays its original outcome, while reusing an id with a changed payload is rejected. Execution is
one command at a time; it does not promise rollback across arbitrary multi-action requests. A
successful command remains reported as applied if subsequent context observation fails. Generic
execute results and receipts, plus command discovery results, are bounded by the existing 256 KiB
serialized-response limit.

Browser discovery and execution additionally exclude shell-managed lifecycle and filesystem-only
operations and reject top-level `path` parameters. The host owns open/new/close lifecycle and
transfers imports/exports as bytes. MCP retains its filesystem-based operations and domain tools.

## Removed surfaces

- `crates/ui-egui` and `apps/gridcraft`: the classical native UI and application.
- Native desktop packaging directories and release workflows.
- The TCP `Remote` control backend and desktop screenshot MCP tool.
- CLI `mcp --connect` and the `send` control-channel subcommand.
- Generated UI keytip tooling and keytip-specific tests.
- Native-only UI dependencies no longer required by the retained workspace.

Engine features such as charts, XLSX, calculation, PDF, and filesystem-backed MCP operations are
not removed by this extraction.

## Browser host protocol 3

### Handshake and envelopes

Each browser mount generates a random `session_id`. The parent host sends
`{"type":"gridcraft:hello"}` to the iframe; the browser replies with `gridcraft:ready`, protocol
version 3, that browser-generated `session_id`, its capabilities, and current context. The host
must use the returned session id rather than choose one. The listener requires
`event.origin === window.location.origin` and `event.source === window.parent`; opaque origins are
rejected. Send requests to the iframe with that origin as `targetOrigin`.

A request envelope has this shape:

```json
{
  "type": "gridcraft:request",
  "session_id": "<session_id from gridcraft:ready>",
  "id": "host-17",
  "method": "execute",
  "params": {
    "request_id": "host-17",
    "expected": {
      "document_id": 1,
      "sheet_index": 0,
      "sheet_name": "Sheet1",
      "change_token": 4
    },
    "command": "home.bold",
    "params": {"range": "A1:C1", "on": true}
  }
}
```

Use the latest `identity` from `gridcraft:ready`, `context`, or a mutation response for operations
that accept `expected`. A successful response returns the current context identity for the next
operation. For `execute`, `set_values`, and `undo`, an inner `request_id` is optional; if supplied,
it must equal the outer `id`. The runtime fills it from the envelope id.

All replies use `type: "gridcraft:response"` (not the request's type), the same session id and id,
then either `ok: true` with a result, or `ok: false` with an error:

```json
{
  "type": "gridcraft:response",
  "session_id": "<session_id from gridcraft:ready>",
  "id": "host-17",
  "ok": true,
  "result": {
    "request_id": "host-17",
    "command_result": {"on": true},
    "context": {
      "identity": {
        "document_id": 1,
        "sheet_index": 0,
        "sheet_name": "Sheet1",
        "change_token": 5
      }
    }
  }
}
```

An id must contain 1–128 ASCII letters, digits, `-`, `_`, `.`, or `:`. The `local-` prefix is
reserved for requests created inside the browser shell; host ids must not use it.

### Commands, open, and snapshots

`commands` takes `{}` and returns the browser-filtered command catalog. It is capped at 256 KiB;
an oversized catalog returns a `limit` error. Generic `execute` accepts one engine command. If it
already ran but its result is too large, the reply remains successful and includes
`applied: true`, `response_omitted: true`, an actionable message to use a smaller range or export,
and the current `context` (or `verification_error` when context observation failed). It does not
claim rollback. An exact retry returns that same compact reply.

The bridge checks the serialized response with a bounded JSON writer rather than creating an
additional serialized copy. The engine may already have allocated the command result before this
transport cap is checked; the cap is not a new calculation or workbook-memory budget.

An import replaces the active workbook through `open`; the host owns workbook lifecycle, and the
bridge does not expose separate `new` or `close` methods. Pass the current identity, an explicit
discard choice, and the file bytes in the transferable `bytes` field:

After reading `session_id` and the latest context from `gridcraft:ready`, pass file bytes as a
transferable `ArrayBuffer`:

```js
const request = {
  type: "gridcraft:request",
  session_id: sessionId,
  id: "open-18",
  method: "open",
  params: {
    expected: currentContext.identity,
    name: "Book.xlsx",
    discardChanges: false,
    csvAsText: false,
  },
  bytes: fileBytes,
};
iframe.contentWindow.postMessage(request, iframeOrigin, [fileBytes]);
```

`password` and `csvAsText` are optional. `discardChanges: true` is required to replace a dirty
workbook after the host has explicitly confirmed that choice.

Capture and persist a snapshot using the latest identity. `snapshot.export` returns metadata and
the XLSX bytes in the response's separate transferable `bytes` field; its `snapshot_id` is the
export request's `id`:

```json
{
  "type": "gridcraft:request",
  "session_id": "<session_id from gridcraft:ready>",
  "id": "save-19",
  "method": "snapshot.export",
  "params": {
    "expected": {"document_id": 1, "sheet_index": 0, "sheet_name": "Sheet1", "change_token": 5}
  }
}
```

After the host has durably saved those bytes, acknowledge the capture with a non-empty storage
version supplied by the host:

```json
{
  "type": "gridcraft:request",
  "session_id": "<session_id from gridcraft:ready>",
  "id": "save-19-ack",
  "method": "snapshot.ack",
  "params": {"snapshot_id": "save-19", "storage_version": "durable-revision-42"}
}
```

`storage_version` must be a non-empty string of at most 256 bytes. The bridge requires it as
evidence from the caller that saving completed; it does not independently verify the host's storage.
The acknowledgement reports whether the live workbook
still matches the captured snapshot. For recovery, persist and acknowledge a snapshot, remount the
browser shell, then import the saved bytes with `open` using the new mount's context identity.

### Limits and replay

- JSON request metadata is limited to 64 KiB and binary files to 16 MiB.
- The engine bridge retains up to 64 distinct `execute`, `set_values`, and `undo` receipts per
  mounted runtime. Its snapshot ledger retains up to 64 snapshot ids and at most 4 unacknowledged
  workbook snapshots.
- The browser transport retains up to 128 receipts for non-query requests per mount. The mount
  also permits at most 4 binary exports over its lifetime.
- For an admitted non-query request, an exact id and identical payload/bytes replay the original
  success or failure. Reusing that id with changed content is rejected. Read-only `context`,
  `commands`, and `inspect` requests are not receipt-cached.
- Persist and remount before exhausting receipts or export allowances. A remount creates a new
  session id and resets in-memory ledgers.

Protocol 3 intentionally breaks compatibility with the older control bridge; it is not a
drop-in-compatible transport. It is also distinct from native MCP's newline-delimited JSON-RPC
stdio protocol, which has no transferred binary payload.

## Verification baseline

Before extraction changes, a locked `gridcraft-web` WASM compile check passed at the baseline
commit. That compile check did not exercise the browser runtime or render the interface.
