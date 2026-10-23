# GridCraft agentic-core roadmap

## Current scope

This branch extracts the spreadsheet engine and its non-UI agent operations from baseline
`8ae67c5534f3d7f145423a57206cf5fd08fe9221`. The goal is to preserve useful workbook capabilities
while removing the classical native frontend, its native packaging/release flows, and its
UI-dependent control surface.

The ten engine/domain crates remain, including calculation, XLSX, charts, and PDF. The supported
interfaces are the headless CLI/MCP server and a Rust/WebAssembly browser shell with a version-3
same-origin host bridge. There is no connected LLM, agent provider, or server-side backend.

## Extraction checklist

- [x] Keep engine/domain crates and capability-based headless command discovery.
- [x] Keep in-process MCP and CLI operations; remove TCP `Remote`, `--connect`, `send`, and
  screenshot control.
- [x] Keep bounded browser inspect, mutation, undo, snapshot export/acknowledgement, and
  request-receipt handling.
- [x] Remove the native desktop UI, application, and native packaging workflows.
- [x] Keep the web host boundary explicit: byte-based imports/exports and shell-managed workbook
  lifecycle.
- [ ] Complete focused engine/MCP/CLI/web tests and the local `cargo xtask ci` gate.
- [ ] Review the built browser shell visually and resolve any remaining integration defects.

## Next

1. Validate protocol-3 command discovery, conflict handling, binary imports/exports, and receipt
   replay in the browser host.
2. Improve workbook capability coverage based on agent workflows, without reintroducing native UI
   requirements.
3. Keep the architectural and capability maps current as commands gain explicit parameterized
   operations.

## Reference

- [`docs/agentic-core.md`](docs/agentic-core.md): preserved and removed surfaces, capability policy,
  browser protocol, and MCP distinction.
- [`docs/architecture.md`](docs/architecture.md): current workspace structure and data flow.
- [`docs/mcp.md`](docs/mcp.md): MCP tools and stdio protocol.
