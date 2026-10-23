# Architecture

GridCraft's workspace is centered on an in-process spreadsheet engine. Commands mutate or query a
`Session`; the CLI and headless MCP server invoke supported commands without a graphical frontend.
The browser shell uses a bounded bridge around the same session and transfers files as bytes.

## Workspace layers

| Layer | Crates | Responsibility |
|---|---|---|
| L0 | `core`, `numfmt`, `pdf` | Cell/range values, number formats, and PDF output |
| L1 | `formula`, `functions` | Formula parsing and worksheet functions |
| L2 | `model`, `calc`, `xlsx` | Workbook model, recalculation, and spreadsheet file formats |
| L3 | `chart` | Toolkit-free chart layout and rendering primitives |
| L5 | `engine` | Session, command catalog, workbook operations, history, import/export |
| L6 | `mcp` | Headless MCP tools/resources over stdio |
| L7 | `apps/gridcraft-cli`, `apps/gridcraft-web` | CLI and browser/WASM shell |
| — | `xtask` | CI, asset, layer, WASM, parity, and workspace-version tools |

The root workspace explicitly lists these packages; the default members omit the web target so
ordinary native development does not build it by default. `cargo xtask wasm` checks the engine
layers and browser package for `wasm32-unknown-unknown`.

## Command flow

Commands are registered in `crates/engine/src/cmd/*`. A shared engine capability policy filters
pure editor/dialog entry points for headless discovery and execution. Commands with a useful
parameterized operation remain available even if their interactive UI also offers a dialog.

`Session::execute` retains UI-inclusive behavior for clients that handle `UiRequest`s.
`Session::execute_headless` rejects unresolved dialogs, cell-editor requests, and external URL
opening instead of silently reporting success; informational messages are not treated as failures.
`file.options` is a calculation-settings query and does not request a dialog.

## Browser boundary

`apps/gridcraft-web` contains a transport-neutral `Runtime` and a Rust/WebAssembly DOM shell. The
runtime delegates bounded command operations to `engine::agent_bridge::BridgeState`. The host
manages workbook lifecycle, transfers imported/exported bytes, and acknowledges snapshots; the
browser command surface rejects filesystem `path` parameters. Protocol details and examples are in
[`agentic-core.md`](agentic-core.md).

## MCP boundary

The MCP server runs in-process and communicates using newline-delimited JSON-RPC 2.0 on stdio. It
retains filesystem-oriented tools and domain operations; it does not connect to the browser or a
desktop process. MCP stdio and the browser's binary version-3 host protocol are separate interfaces.

## Known limits

- Headless execution is a single command invocation, not an atomic multi-command transaction.
- The browser preview is intentionally read-only; editing happens through the command console and
  host-mediated operations.
- Import/export and model capabilities do not imply full Excel fidelity or parity.

## Revision history

| Change | Summary |
|---|---|
| Agentic-core extraction | Documented the current engine, CLI/MCP, and browser architecture |
