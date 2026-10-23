# Model Context Protocol

GridCraft's MCP server runs a headless `Session` in-process and communicates over stdio using
newline-delimited JSON-RPC 2.0. It does not connect to a desktop app or expose screenshot capture.

Start the server from a workbook or a blank session:

```sh
gridcraft-cli mcp
gridcraft-cli mcp --in workbook.xlsx
gridcraft-cli mcp --sample sales
```

Configure an MCP client to launch `gridcraft-cli mcp` as a stdio process. The CLI can also run
without MCP for local workbook operations; see [`cli.md`](cli.md).

## Tools and resources

The tool catalog includes common cell and range operations, formula evaluation, workbook
inspection, generic command execution, and filesystem-backed import/export tools. The generic
`execute_command` operation delegates to the engine command catalog and headless capability policy.
Commands that only open an editor, dialog, or external URL are not accepted. Parameterized engine
operations remain available even if their interactive menu entry can open a dialog.

MCP resources expose workbook context, the filtered command catalog, and worksheet functions.
For the full capability map and the distinction between MCP stdio and the browser host protocol,
see [`agentic-core.md`](agentic-core.md).

Unlike the browser bridge, MCP tools can use filesystem paths for file operations. MCP does not
transport `ArrayBuffer` payloads and is not protocol-3 browser bridge compatible.
