# GridCraft agentic spreadsheet core

GridCraft is a Rust spreadsheet engine and set of agent-facing interfaces. This branch keeps the
workbook model, calculation and file-format capabilities while removing the classical native
desktop application and its UI-only interaction layer.

The workspace retains ten engine/domain crates (`core`, `numfmt`, `formula`, `functions`, `model`,
`calc`, `xlsx`, `chart`, `pdf`, and `engine`), plus the CLI, headless MCP server, browser shell, and
workspace tooling. See [`docs/architecture.md`](docs/architecture.md) for the crate map and
[`docs/agentic-core.md`](docs/agentic-core.md) for the capability and removal map.

## Agent interfaces

- **CLI:** `gridcraft-cli` provides spreadsheet operations and can start an in-process MCP server.
- **MCP:** newline-delimited JSON-RPC 2.0 over stdio. The server is headless; it retains generic
  engine operations and filesystem-backed file tools, but has no desktop connection or screenshot
  tool. See [`docs/mcp.md`](docs/mcp.md).
- **Browser:** `apps/gridcraft-web` is a Rust/WebAssembly shell with a formatted-value preview,
  command console, and same-origin host bridge. Its version-3 protocol transfers file bytes and
  keeps browser lifecycle operations in the host. The host is not an LLM or inference service.

The command catalog remains broad, including formulas, formatting, charts, PivotTables, analysis,
printing/PDF, and workbook import/export. Headless discovery and execution exclude commands that
only open an editor or require an external interactive action; explicit parameters remain
available for commands that also have dialog fallbacks.

## Build and test

```sh
cargo test --locked -p gridcraft-engine
cargo test --locked -p gridcraft-mcp
cargo test --locked -p gridcraft-cli
cargo xtask ci
```

To build the browser shell, install Trunk and the `wasm32-unknown-unknown` Rust target, then run:

```sh
cd apps/gridcraft-web
trunk build --release
```

The static site is written to `dist/web`; see [`packaging/web/README.md`](packaging/web/README.md)
for hosting guidance.

## Project status

This is an agentic-core extraction from baseline `8ae67c5534f3d7f145423a57206cf5fd08fe9221`.
It is not a release of the former desktop application and does not claim Excel parity. The current
scope and known interface limits are documented in [`ROADMAP.md`](ROADMAP.md).

## License and credits

GridCraft is dual-licensed under MIT or Apache-2.0. See [`LICENSE`](LICENSE), [`ATTRIBUTION.md`](ATTRIBUTION.md),
and the contributor guidance in [`CONTRIBUTING.md`](CONTRIBUTING.md).
