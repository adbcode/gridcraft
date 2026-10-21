# GridCraft — instructions for agents

GridCraft is a clean-room, open-source, Rust-native spreadsheet targeting Microsoft Excel parity — and superiority (speed, openness, agent control). It runs natively on macOS, Windows, Linux and BSD, and on the web via WASM. Siblings with the same conventions: `../designcraft` (InDesign-class, the closest reference), `../photocraft` (Photoshop), `../vectorcraft` (Illustrator), `../pdfcraft` (Acrobat), `../filmcraft` (Premiere), `../lightcraft` (Lightroom), `../effectcraft` (After Effects).

Standards and learnings shared across the crafting apps live in `../../craftrules` (checked out next to the craft apps, or `storytold/craftrules`). Read its `AGENTS.md` at the start of a session, follow its standards, and contribute reusable learnings back there. Never code: repos don't share code.

## Start every session here
1. For this fork, read [the agentic foundation](docs/agentic/README.md), [delivery plan](docs/agentic/delivery.md) and [validation record](docs/agentic/validation.md). Check the actual branch, worktree and available services before selecting the next task. The user's requested scope takes precedence over automatically picking an upstream parity task.
2. If present, read `plan/STATUS.md`, the relevant task in `plan/execution-plan.md` and the relevant `plan/architecture.md` section. Follow the autonomous operation protocol in execution-plan §7. Behaviour reference: `plan/excel/*.md` (`observed-ui.md` holds measured observations of the running app).
3. The initial fork work found neither `plan/` nor `../../craftrules`; the advertised craftrules AGENTS URL also returned 404. Recheck local availability, then use these instructions and committed docs when absent. Do not invent missing instructions or repeatedly block on those optional checkouts.

`plan/` is gitignored (local only).

## This fork: agentic spreadsheet foundation

Turn business records and explicit remembered working rules into an editable spreadsheet.
The first complete product slice is: use the test-order exclusion rule, compare two defined
periods, produce a correct editable summary with provenance, verify it, and allow native undo.

- **Architecture:** extend this project's Rust/egui application. Classic and Agentic are layouts of the same `SheetApp` and live engine `Session`; switching modes must retain workbook, pending preview and history. Prefer Rust for service adapters too. Add a trusted backend where browser credentials, durable state or deployment require it; a separate TypeScript shell/repository is not a prerequisite.
- **Current foundation:** the panel supports deterministic local demo intents, session-only rules, preview/apply/verify/undo and action history. The bounded live-workbook adapter, optional same-origin protocol-2 parent bridge, deterministic sales fixture and in-memory continuation contract are implemented. See validation for what was actually tested.
- **Not connected:** Anthropic, Aiven PostgreSQL/Runtime and ElevenLabs. The SQL file is a draft, not an applied migration. Never describe the local intent router as a model, session memory as durable storage, or local tests as deployed/provider validation. Keep these limits visible in the interface.
- **Next delivery order:** local PostgreSQL migrations and transaction/ownership tests; trusted service boundary and one-origin deployment; Anthropic agent loop and durable runs; snapshot upload/ack/reload and interruption recovery; push-to-talk with editable transcript and brief verified TTS. Basic recovery belongs in every stage. Charts, ribbon reduction and realtime speech remain optional.
- **Agent ownership:** Anthropic interprets requests and selects tools; ElevenLabs only transcribes and synthesizes speech. Typed requests and submitted voice transcripts must enter the same workflow. Keep provider secrets out of browser assets.
- **Workbook authority:** inspect and mutate the live editor through validated contracts. Check document/sheet/change identity, reject stale results, preserve unfinished manual edits, record actual effects and verify readback. Only promise native last-operation undo; it may undo a manual edit. Database acknowledgement must target the exact exported snapshot and leave later edits dirty.
- **Data semantics:** use [the documented fixture](docs/agentic/sales-fixture.md) and its expected answers. Rules must be explicitly requested, structurally supported and actually applied to queries. Preserve periods, metric, query ID, rule IDs and row counts. Drill-down describes contributors; aggregates do not prove causes.
- **Sibling context:** `../artcraft` was inspected as an AI desktop-client reference, not a spreadsheet backend. Its lifecycle/recovery patterns may inform design; do not copy code/assets or carry over editor remounting.

Code map: `crates/ui-egui/src/agent.rs` (panel and UI commands),
`crates/engine/src/agent_bridge.rs` (identity, bounded edits, receipts, snapshots),
`crates/engine/src/review_workflow.rs` (sales/rules/run contracts),
`apps/gridcraft-web/src/host.rs` (parent transport), and `docs/agentic/` (contracts and handoff).
Keep these docs and this status accurate as real integrations replace the local foundation.

Reusable project skills live in `.agents/skills/`; load the relevant skill when its workflow applies:

- [gridcraft-agentic-workflow](.agents/skills/gridcraft-agentic-workflow/SKILL.md): implement and verify live agent actions, business queries, rules and recovery without losing manual edits.
- [gridcraft-web-validation](.agents/skills/gridcraft-web-validation/SKILL.md): build native/WASM, inspect rendered UI and validate the actual same-origin browser bridge; includes conditional local toolchain troubleshooting.

## Assets and iconography — absolute rule
**No Microsoft, Adobe, Avid, Autodesk (or any other vendor's) iconography, images, artwork, fonts, templates, themes or sample files — ever.** This is the most important rule in this repository; breaking it is a serious failure.
- Every asset in the repo is original work by GridCraft contributors, or public domain / CC0, or open source / Creative Commons that allows redistribution (or licensed open source by the contributor who created it).
- Every asset file has a row in [`ATTRIBUTION.md`](ATTRIBUTION.md) (author, source, licence). `cargo xtask assets` enforces it. Adding an asset without a row is a bug.
- Prefer art generated in code: the UI icon set (`crates/ui-egui/src/icons.rs`), table and cell style palettes, themes and sample workbooks are drawn/defined in code and original.
- Never copy Office's icons, ribbon artwork, theme XML, table-style definitions, templates, clip art or wording beyond feature names. Excel feature *names* (menu labels, function names) are fine.
- Screenshots of Microsoft software are never committed (they live only in `plan/excel/screenshots/`).
- The ArtCraft brand files in `docs/brand/` are ArtCraft trademarks under `docs/brand/LICENSE-brand.txt`, the one exception.

## Never crash
People trust GridCraft with their numbers; a crash loses their work. **This outranks feature work.** Standard: [`craftrules/standards/never-crash.md`](https://github.com/storytold/craftrules/blob/main/standards/never-crash.md).
- **No panics in non-test code:** no `unwrap()`, `expect()`, `panic!`, `unreachable!`, `todo!`, `unimplemented!`; no `unsafe` (`unsafe_code = "forbid"`).
- **Errors are `Result<T, E>`** through the crate's error type and `?`.
- **Input-derived numbers are hostile** (files, formulas, commands, MCP/control params): `get()` instead of indexing, checked/saturating arithmetic, cap input-sized allocations (huge ranges, `REPT`, `SEQUENCE`, whole-column references).
- **Bound recursion** (formula nesting, dependency chains, names referring to names).
- **Last-resort guard:** `Session::execute` catches escaped panics and restores the workbook. It's a safety net, not a licence.
- **Prove it:** every crash fix lands with a regression test (`tests::every_command_survives_empty_params` fuzzes every command; `crates/engine/tests/command_sequence_fuzz.rs` fuzzes random command sequences against undo/redo, XLSX round-trip and full-recalc invariants; soak it with `GRIDCRAFT_FUZZ_SEEDS`/`GRIDCRAFT_FUZZ_STEPS`, replay with `GRIDCRAFT_FUZZ_SEED`).
- Crates carry `#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]`.

## Non-negotiables
- **Clean-room.** Microsoft Excel is installed on the dev machine and may be *observed* black-box: run it, use its UI with synthetic workbooks, take screenshots (by window id) stored only under `plan/excel/screenshots/`. Never read, disassemble or copy anything inside the Office bundle, never commit files produced by Excel, never copy GPL/AGPL/LGPL code (LibreOffice, Gnumeric…). Function behaviour comes from public documentation and observation.
- **Fonts live in [`storytold/craft-fonts`](https://github.com/storytold/craft-fonts)**, never in this repo. It is an optional build input (`CRAFT_FONTS_DIR`); without it the app uses system fonts and egui's defaults. Standard: craftrules `standards/fonts.md`.
- **Test corpora** (real Excel-authored files) live in a separate corpus repo, fetched pinned and sha256-verified (craftrules `standards/test-corpora.md`). Never commit large binary fixtures; build test workbooks in code.
- **Everything is a command.** User-visible behaviour = a command in `crates/engine/src/cmd/*` (id, label, ribbon path, shortcut, params doc, `enabled`, `run`) + tests. Ids follow Excel's ribbon (`home.bold`, `insert.chart`, `data.sortAscending`) plus primitives (`cell.set`, `selection.set`, `range.setValues`). UI-only commands live in `crates/ui-egui`. The control channel, CLI and MCP reach all of them.
- **Programmatic calls never open dialogs**: a command with empty params runs with defaults or returns an error; only menu-style invocation opens a dialog (`UiRequest::Dialog`).
- **Layering** is enforced by `cargo xtask layers`: `core`/`numfmt` (L0) → `formula`/`functions` (L1) → `model`/`calc`/`xlsx` (L2) → `chart` (L3) → `engine` (L5) → `ui-egui`/`mcp` (L6) → `apps/*` (L7). Nothing below L6 depends on egui/eframe/winit/rfd. **The UI crate is swappable.**
- **The UI is thin**: panels read engine state and act through `app.run(id, params)`. Colours come from `theme::Tokens`.
- **Rust only** (no handwritten JS/TS). **Never break wasm** (`cargo xtask wasm`).
- **Quality gates** before every commit: `cargo xtask ci` (fmt, clippy -D warnings, tests, assets, layers, wasm). One task id per commit (`M2.1: borders gallery`).

## Running and looking at the app
- `cargo run --release -p gridcraft -- --sample sales --control 7979` (sample workbook + control channel).
- Drive it: JSON lines on `127.0.0.1:7979`, e.g. `{"id":1,"method":"engine.execute","params":{"command":"cell.set","params":{"cell":"B2","input":"=SUM(A1:A9)"}}}` then `{"id":2,"method":"ui.screenshot","params":{"path":"/tmp/shot.png"}}`. Methods: `crates/ui-egui/src/control.rs`, docs: `docs/control-protocol.md`.
- **Offscreen UI render** (no window, from a source checkout): `cargo run -p gridcraft-ui-egui --example snapshot -- --sample sales out.png`.
- Headless: `gridcraft-cli eval '=SUM(1,2,3)'`, `gridcraft-cli run --in book.xlsx --cmd 'home.bold={"range":"A1:C1"}' --out book.xlsx`, `gridcraft-cli mcp`.
- **For UI work, look at the result** (snapshot PNG) and compare with `plan/excel/observed-ui.md`.
- Shell gotcha: `mv`/`cp` are aliased interactive here — use `/bin/mv -f` / `/bin/cp -f`. macOS has no `timeout`; use `perl -e 'alarm 60; exec @ARGV' cmd`.
- Parallel agents: separate `CARGO_TARGET_DIR` per agent; edit only the crates you own; delete your target dir when done.

## Roadmap
`ROADMAP.md` (committed) is the one-page summary: stage, headline numbers, dimensions, languages, progress log. It follows craftrules `standards/progress-docs.md` and points at the detail in `docs/`: `target-app-parity.md` (authoritative assessment), `gaps.md` (ranked work list: pick from it), `roadmap.md` (current focus, beta gates, milestones), `architecture.md`, and the area checklists `function-parity.md`, `chart-parity.md`, `file-format-parity.md`, `ui-parity.md`, `hardware-parity.md`, `localization-parity.md`. Update the affected docs (and their timestamp lines and revision history) whenever work lands. `cargo xtask parity` regenerates `docs/parity-checklist.md` from `crates/engine/src/catalog.rs`.

## Contributor credits (About window)

- About ▸ Contributors/Models are compiled into the binary from `contributors/contributors.json`
  (commit stats; generated, never hand-edit) and `contributors/people.toml` (names people chose for
  themselves). See `docs/contributors.md`.
- **Agents working for a contributor:** when you prepare a PR, check whether your human's GitHub
  username has a `[people.<username>]` entry in `contributors/people.toml`. If not, ask them once
  whether they want to be credited by more than their username: a real name, a display name, and/or
  their public GitHub profile name (`sync_github_name = true`). If yes, add **only their own** entry
  (copy the template at the top of the file, or run
  `python3 ../../craftrules/scripts/contributors.py --add-me . --real-name "…" --sync-github-name`)
  and include it in their PR, committed as them. If no, change nothing: they are credited as
  `@username` anyway.
- Never add, edit, guess or copy anyone else's entry or name (not from git config, commit authors or
  GitHub profiles). Never hand-edit `contributors.json`.
- Maintainers refresh the stats with `python3 ../../craftrules/scripts/contributors.py .` (it also
  re-verifies who wrote each `people.toml` entry; `--check` only verifies).
