# Model Context Protocol

`gridcraft-cli mcp` exposes an in-process, headless GridCraft engine to MCP clients over stdio:
newline-delimited JSON-RPC 2.0. Protocol versions `2025-06-18`, `2025-03-26`, and `2024-11-05` are
supported. Logs are written to stderr.

Start with a blank workbook, an existing file, or a sample:

```sh
gridcraft-cli mcp
gridcraft-cli mcp --in workbook.xlsx
gridcraft-cli mcp --sample sales
```

## Client configuration

Claude Code can register the stdio server directly:

```sh
claude mcp add gridcraft -- gridcraft-cli mcp
```

For `.mcp.json`, Claude Desktop, and other clients that accept MCP server JSON configuration:

```json
{
  "mcpServers": {
    "gridcraft": {
      "command": "gridcraft-cli",
      "args": ["mcp"]
    }
  }
}
```

To start with an existing workbook, set `"args": ["mcp", "--in", "/path/to/book.xlsx"]`.
The CLI also supports local workbook operations without MCP; see [`cli.md`](cli.md).

## Tools

| Tool | Arguments | Purpose |
|---|---|---|
| `execute_command` | `command`, `params?` | Run a supported headless engine command. |
| `list_commands` | `search?` | Discover supported command ids, labels, ribbon paths, shortcuts, and parameter docs. |
| `inspect_workbook` | — | Inspect sheets, used ranges, tables, charts, names, selection, and history. |
| `read_range` | `range?`, `sheet?`, `formulas?`, `formatted?` | Read rows of values (defaults to the used range). |
| `write_range` | `range`, `values` (2-D) | Write values or formulas; strings beginning with `=` are formulas. |
| `set_cell` / `get_cell` | `cell`, `input` / `cell` | Enter one cell or read its value, formula, display text, and style. |
| `evaluate_formula` | `formula`, `cell?` | Evaluate a formula without changing the sheet. |
| `select` | `range`, `active?` | Set the selection. |
| `format_range` | `range`, formatting options | Apply supported font, color, alignment, number-format, wrapping, and border options. |
| `insert_chart` | `range`, `type`, `title?`, `subtype?`, `at?` | Insert a chart. |
| `create_table` | `range`, `style?`, `header?`, `name?` | Create a table. |
| `sort_range` | `keys`, `range?`, `header?` | Sort a range; keys may be `"B"`, `"B desc"`, or `{ "column": "B", "order": "desc" }`. |
| `filter` | `column`, `range?`, `values?`, `custom?`, `top?`, `blanks?`, `clear?` | Apply or clear a filter. |
| `open_workbook` / `save_workbook` / `new_workbook` | `path` / `path?` / `sample?` | Open, save, or create local workbooks. |
| `undo` / `redo` | `steps?` | Undo or redo workbook changes. |
| `list_functions` | `search?`, `category?` | Discover worksheet functions and signatures. |

`execute_command` uses the headless capability policy, not every UI action in the engine catalog.
Pure editor/dialog entry points and external-URL actions are excluded or rejected; parameterized
engine operations remain available when they have a meaningful headless implementation. Use
`list_commands` to discover what the current session supports.

Tool results are text blocks containing JSON. Failures set `isError: true`; unknown tools,
missing required arguments, and wrong argument types are JSON-RPC `-32602` errors, malformed JSON
is `-32700`, and unknown methods are `-32601`.

## Resources

- `gridcraft://workbook` — inspection of the active workbook.
- `gridcraft://commands` — the supported command catalog and parameter docs.
- `gridcraft://functions` — worksheet functions.

`prompts/list` returns an empty list.

## Example session

```text
→ {"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}
→ {"jsonrpc":"2.0","method":"notifications/initialized"}
→ {"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"write_range","arguments":{"range":"A1","values":[["Item","Cost"],["Rent",1200],["Food",400],["Total","=SUM(B2:B3)"]]}}}
→ {"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"read_range","arguments":{"range":"B4"}}}
← {"jsonrpc":"2.0","id":3,"result":{"content":[{"type":"text","text":"{\n  \"range\": \"B4\", ... \"values\": [[1600.0]]\n}"}],"isError":false}}
```

## Embedding

`gridcraft-mcp` is a library. Create a `Server` with `Headless`, then call
`Server::serve(stdin, stdout)` or `Server::handle_line(line) -> Option<String>`. A host can also
implement the `Backend` trait (`call(method, params)`) to connect another headless engine session.
This is stdio JSON-RPC, separate from the browser's protocol-3 same-origin bridge; MCP file tools
use filesystem paths and do not transfer `ArrayBuffer` payloads. See
[`agentic-core.md`](agentic-core.md) for the capability map and browser protocol.
