# GridCraft — instructions for agents

GridCraft is a clean-room Rust spreadsheet engine with headless CLI/MCP interfaces and a browser shell via WASM. This branch is an agentic-core extraction, not the former native desktop application. Siblings with the same conventions: `../designcraft`, `../photocraft`, `../vectorcraft`, `../pdfcraft`, `../filmcraft`, `../lightcraft`, `../effectcraft`.

Standards and learnings shared across the crafting apps live in `../../craftrules` (checked out next to the craft apps, or `storytold/craftrules`). Read its `AGENTS.md` at the start of a session, follow its standards, and contribute reusable learnings back there. Never code: repos don't share code.

## Start every session here
1. Read `plan/STATUS.md` (current milestone, next task), then the task in `plan/execution-plan.md` and the relevant `plan/architecture.md` section. Behaviour reference: `plan/excel/*.md` (`observed-ui.md` holds measured observations of the running app).
2. Follow the autonomous operation protocol (`plan/execution-plan.md` §7). Don't stop to ask unless §7 lists the decision as the user's.

`plan/` is gitignored (local only).

## Assets and iconography — absolute rule
**No Microsoft, Adobe, Avid, Autodesk (or any other vendor's) iconography, images, artwork, fonts, templates, themes or sample files — ever.** This is the most important rule in this repository; breaking it is a serious failure.
- Every asset in the repo is original work by GridCraft contributors, or public domain / CC0, or open source / Creative Commons that allows redistribution (or licensed open source by the contributor who created it).
- Every asset file has a row in [`ATTRIBUTION.md`](ATTRIBUTION.md) (author, source, licence). `cargo xtask assets` enforces it. Adding an asset without a row is a bug.
- Prefer original, code-generated art for any future interface; table and cell style palettes and sample workbooks are defined in code.
- Never copy Office's icons, ribbon artwork, theme XML, table-style definitions, templates, clip art or wording beyond feature names. Excel feature *names* (menu labels, function names) are fine.
- Screenshots of Microsoft software are never committed (they live only in `plan/excel/screenshots/`).
- The ArtCraft brand files in `docs/brand/` are ArtCraft trademarks under `docs/brand/LICENSE-brand.txt`, the one exception.

## Never crash
People trust GridCraft with their numbers; a crash loses their work. **This outranks feature work.** Standard: [`craftrules/standards/never-crash.md`](https://github.com/storytold/craftrules/blob/main/standards/never-crash.md).
- **No panics in non-test code:** no `unwrap()`, `expect()`, `panic!`, `unreachable!`, `todo!`, `unimplemented!`; no `unsafe` (`unsafe_code = "forbid"`).
- **Errors are `Result<T, E>`** through the crate's error type and `?`.
- **Input-derived numbers are hostile** (files, formulas, commands, MCP/CLI/browser bridge params): `get()` instead of indexing, checked/saturating arithmetic, cap input-sized allocations (huge ranges, `REPT`, `SEQUENCE`, whole-column references).
- **Bound recursion** (formula nesting, dependency chains, names referring to names).
- **Last-resort guard:** `Session::execute` catches escaped panics and restores the workbook. It's a safety net, not a licence.
- **Prove it:** every crash fix lands with a regression test (`tests::every_command_survives_empty_params` fuzzes every command; `crates/engine/tests/command_sequence_fuzz.rs` fuzzes random command sequences against undo/redo, XLSX round-trip and full-recalc invariants; soak it with `GRIDCRAFT_FUZZ_SEEDS`/`GRIDCRAFT_FUZZ_STEPS`, replay with `GRIDCRAFT_FUZZ_SEED`).
- Crates carry `#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]`.

## Non-negotiables
- **Clean-room.** Microsoft Excel is installed on the dev machine and may be *observed* black-box: run it, use its UI with synthetic workbooks, take screenshots (by window id) stored only under `plan/excel/screenshots/`. Never read, disassemble or copy anything inside the Office bundle, never commit files produced by Excel, never copy GPL/AGPL/LGPL code (LibreOffice, Gnumeric…). Function behaviour comes from public documentation and observation.
- **Fonts live in [`storytold/craft-fonts`](https://github.com/storytold/craft-fonts)**, never in this repo. They are an optional external build input via `CRAFT_FONTS_DIR`; without it, use system fonts. Never vendor or commit font files here. Standard: craftrules `standards/fonts.md`.
- **Test corpora** (real Excel-authored files) live in a separate corpus repo, fetched pinned and sha256-verified (craftrules `standards/test-corpora.md`). Never commit large binary fixtures; build test workbooks in code.
- **Everything is a command.** Workbook behavior lives in `crates/engine/src/cmd/*` (id, label, parameters, enablement, execution) + tests. Headless command discovery uses the shared engine agent-capability policy; pure editor/dialog and external URL actions are excluded.
- **Programmatic calls never open dialogs**: a command with empty params runs with defaults or returns an error; only menu-style invocation opens a dialog (`UiRequest::Dialog`).
- **Layering** is enforced by `cargo xtask layers`: `core`/`numfmt`/`pdf` (L0) → `formula`/`functions` (L1) → `model`/`calc`/`xlsx` (L2) → `chart` (L3) → `engine` (L5) → `mcp` (L6) → CLI/web apps (L7).
- **Rust only** (no handwritten JS/TS). **Never break wasm** (`cargo xtask wasm`).
- **Quality gates** before every commit: `cargo xtask ci` (fmt, clippy -D warnings, tests, assets, layers, wasm). One task id per commit (`M2.1: borders gallery`).

## Running the agentic interfaces
- Headless CLI: `cargo run -p gridcraft-cli -- eval '=SUM(1,2,3)'` or `cargo run -p gridcraft-cli -- mcp`.
- Browser shell: build with `trunk build --release` from `apps/gridcraft-web`; see `docs/agentic-core.md` for protocol 3.
- Shell gotcha: `mv`/`cp` are aliased interactive here — use `/bin/mv -f` / `/bin/cp -f`. macOS has no `timeout`; use `perl -e 'alarm 60; exec @ARGV' cmd`.
- Parallel agents: separate `CARGO_TARGET_DIR` per agent; edit only the crates you own; delete your target dir when done.

## Roadmap
`ROADMAP.md` summarizes this branch's extraction scope. Keep it and `docs/agentic-core.md` aligned with actual supported interfaces and capabilities. `cargo xtask parity` regenerates `docs/parity-checklist.md` from the engine command catalog.
