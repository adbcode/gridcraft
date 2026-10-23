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
successful command remains reported as applied if subsequent context observation fails.

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

The browser host uses version-3 `postMessage` envelopes. A host request has this shape:

```json
{
  "type": "gridcraft:request",
  "session_id": "host-session",
  "id": "host-17",
  "method": "execute",
  "params": {
    "request_id": "mutation-17",
    "expected": {
      "document_id": 1,
      "sheet_index": 0,
      "sheet_name": "Sheet1",
      "change_token": 4
    },
    "command": "home.bold",
    "params": {"range": "A1:C1"}
  }
}
```

A successful response uses the same `type`, `session_id`, and `id`, with `ok: true` and the result:

```json
{
  "type": "gridcraft:response",
  "session_id": "host-session",
  "id": "host-17",
  "ok": true,
  "result": {
    "request_id": "mutation-17",
    "command_result": null,
    "context": {"identity": {"change_token": 5}}
  }
}
```

Failures return `ok: false` and an `error` string. The envelope may carry a separate `bytes` field
using an `ArrayBuffer`/transferable payload; imports use `open` with bytes and exports return bytes
from `snapshot.export`. This is binary transport rather than base64 in JSON. JSON requests are
limited to 64 KiB and files to 16 MiB. The host API includes `context`, `commands`, `inspect`,
`execute`, `set_values`, `undo`, `open`, `snapshot.export`, and `snapshot.ack`. Local request ids
use the reserved `local-` prefix.

Protocol 3 intentionally breaks compatibility with the older control bridge; it is not a
drop-in-compatible transport. It is also distinct from native MCP's newline-delimited JSON-RPC
stdio protocol, which has no transferred binary payload.

## Verification baseline

Before extraction changes, a locked `gridcraft-web` WASM compile check passed at the baseline
commit. That compile check did not exercise the browser runtime or render the interface.
