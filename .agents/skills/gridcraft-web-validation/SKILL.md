---
name: gridcraft-web-validation
description: Build and validate GridCraft native and WASM changes, inspect the Rust Agentic interface, and exercise the real same-origin parent bridge. Use for UI, host transport or cross-platform verification; includes conditional toolchain troubleshooting.
---

# GridCraft native and browser validation

Run from the repository root unless noted. Read `AGENTS.md` and
[validation.md](../../../docs/agentic/validation.md) for prior evidence, then verify current
tools and source. Prior passing tests, temporary binaries and screenshots are not evidence
for changed code. This workflow runs locally; it does not authorize deployment or live calls.

## Build and select checks

Use the existing workspace and locked dependencies. `cargo xtask ci` is the commit gate and
stops on its first failure; do not call skipped stages passed. For iteration, select relevant
engine/UI tests, native `cargo check -p gridcraft --locked`, and `cargo xtask wasm`. UI tests
that do not use a renderer can pass even when the real browser rendering fails.

For the web app, the normal workflow is `trunk build --release` or `trunk serve` from
`apps/gridcraft-web`. Its configuration writes `dist/web`, uses relative asset URLs and
serves on localhost port 8771. Confirm the current config before relying on paths/ports.
Use [the local-toolchain reference](references/local-toolchain.md) only if missing targets,
Trunk/linker tooling, cache permissions or GPU access block the normal path.

Keep unrelated lint failures visible and unchanged unless fixing them is in scope. A
command-local diagnostic allowance is not a green strict CI result. Do not install global
tooling or persist relaxed lint/compiler settings merely to make a local check pass.

## Inspect the actual interface

Run the Rust UI in a browser and inspect both preview and applied states. Native offscreen
rendering is also available; choose a temporary destination:

```sh
cargo run -p gridcraft-ui-egui --example snapshot -- \
  --size 1440x1000 --scale 1 \
  --cmd 'ui.agent.toggle={"on":true}' \
  --cmd 'ui.agent.remember={}' \
  --cmd 'ui.agent.compare={}' /tmp/gridcraft-agentic-preview.png
```

Add `--cmd 'ui.agent.apply={}'` before the destination to inspect the written sheet. Native
font discovery can hide missing browser glyphs: inspect browser text too. Prefer supported
text or the existing code-drawn icon system; do not import vendor fonts or artwork. Check
summary headers and provenance at default column widths without adding unrelated undo steps.

In the actual UI: select Agentic, remember the test-order exclusion, compare, inspect the
overwrite target and provenance, apply, switch Classic/Agentic, and undo. Confirm values and
formulas, a persistent conversation/rule, and the same live workbook/history. Also check that
a manual edit invalidates a prepared preview. The local demo must visibly identify its limits.

## Exercise the parent bridge when it changes

Serve a temporary same-origin parent and the real compiled editor with `?host=parent`.
Use [bridge-protocol.md](../../../docs/agentic/bridge-protocol.md) as the contract; never mock
the engine for transport acceptance. Temporary browser-driver code belongs outside the
Rust product repository. Browser tools can drive the loaded page without committing a JS app.

Check these observable outcomes:

1. Hello returns the mounted session; ready alone cannot authorize workbook actions.
2. Open returns correlated success or error for the intended workbook.
3. An actual keyboard cell edit is visible through inspection; an unfinished draft yields busy.
4. A formula write returns its calculated value; native undo restores the prior value.
5. Exact retry replays once, different input under the same ID is rejected, and stale writes fail.
6. Export yields XLSX bytes; acknowledging after a later edit leaves that edit unsaved.
7. Layout toggles retain the iframe/session; unsupported operations do not open dialogs.

For malformed XLSX coverage, use a truncated ZIP with a ZIP signature. Plain text renamed
`.xlsx` may be legitimately recognized as CSV by the existing loader. Test origin/source
and wrong-session rejection when changing handshake/authentication logic.

Serve WASM as `application/wasm`; see
[packaging/web/README.md](../../../packaging/web/README.md) for hosting details. A local HTTP
preview does not prove public HTTPS, microphone behavior, provider availability or database
recovery. Record the exact boundaries in validation notes.

Record the PID/port of any preview server you start. Stop it when requested or when it is no
longer needed, and verify the listener is gone. Target that process rather than broadly
killing Python/Node services. Leave a requested running preview with its URL and stop method.
