# Foundation and service validation

> Last reviewed: 2026-10-22 · Last updated: 2026-10-22 · Change: major

## Voice (checkpoint 4) — 2026-10-22

- Composer microphone (connected mode with ElevenLabs STT): browser `getUserMedia` +
  `MediaRecorder`, Stop transcribes, Cancel discards (a late recording is ignored), recordings stop
  after 60 s. The transcript replaces the draft for editing and is never auto-submitted; Send uses
  the same `/api/runs/start` path as typed text.
- Only voice-originated runs are spoken, and only after the run completes: the first sentence
  (at most 280 characters) is synthesized and played, with Play/Stop controls for replay.
- Tests: `speech_fake_elevenlabs` (2: trimmed transcript, empty/unsupported audio rejected; bounded
  synthesis, missing voice → 503) and 2 UI tests (voice → editable draft → typed path → spoken
  outcome; typed requests never spoken; cancelled recording ignored).
- Live ElevenLabs smoke against the local service: see `ckpt4-*` evidence (synthesize → MP3,
  transcribe that MP3 back to text, oversized text → 422).

## Save receipts and reload recovery (checkpoint 3) — 2026-10-22

- Agentic **Save** (`ui.agent.save`, connected mode): registers a workbook UUID on first save,
  captures one bridge snapshot, exports those exact bytes, uploads them with the expected version
  and marks the document saved only after the committed receipt (matching export ID, identity and
  workbook) and the editor's acknowledgement of that capture. Later edits stay unsaved.
- Network failure re-sends the identical export (same export ID, metadata and bytes); HTTP 409
  reports a version conflict and leaves the workbook edited.
- The workbook binding, run ID and session ID persist in browser storage. On reload the editor
  downloads the latest committed version, opens it through the normal loader, waits for that
  correlated open before agent actions are enabled, then fetches the stored run (interrupted runs
  offer Retry → resume).
- UI tests: 16 passed (3 new: receipt/ack with later edits kept; identical retry then conflict;
  reload recovery then run). Live service contract checked with curl against the local service:
  receipt fields, exact retry 200, stale expected version 409, latest download version/SHA headers.

## Connected harness (checkpoint 2) — 2026-10-22

- `cargo xtask ci`: all 8 steps passed (fmt, clippy, test, assets, layers, wasm, parity, keytips).
- Disposable PostgreSQL (`GRIDCRAFT_TEST_DATABASE_URL`): 7 ignored DB tests passed, including
  `runs_fake_openai` (local fake Chat Completions server: start → `compare_sales` →
  `write_sales_review` plan → continue → completed with 1 action; request-ID replay and 409 on a
  different body; exact continue replay without a second receipt; 409 for a different outcome or
  tool call ID; stale run → interrupted → resume; `remember_rule` then current total €155.00; mock
  file listing and bounded preview).
- `gridcraft-ui-egui --test agent_workspace`: 13 passed, 7 of them connected mode against a scripted
  `AgentApi`: manual edit → Apply → `stale_workbook` with the edit kept; busy editor → `busy` with the
  draft kept; Discard → `user_declined`; verified write and action count = completed receipts;
  network failure retries the identical body (3 attempts) then offers Retry; file search, attach and
  unsupported formats.
- Live provider smoke (local service, local database, configured OpenAI model): compare €150.00 →
  €655.00 with a sourced `sales_query`; "Remember to exclude test orders" saved as a rule; compare
  again €150.00 → €155.00. The model needed `OPENAI_REASONING_EFFORT=none`: without it, Chat
  Completions returned HTTP 400 (`invalid_request_error`, param `reasoning_effort`) for function tools.
- Known pre-existing issue: `cargo clippy --target wasm32-unknown-unknown -p gridcraft-web` reports
  `backstage.rs` dead variants and a `desktop_theme.rs` lint, which are unchanged from the baseline.

## Service baseline — 2026-10-22

Work started from the clean `feat/agentic-foundation` commit
`8b89f0e663ca31cd62a98b948fc4f7f0ccee8710` on the new
`feat/aiven-service-baseline` branch. This section records the new service checks;
the earlier foundation/browser evidence below remains dated evidence for unchanged code.

- Server unit and local HTTP mock tests: **20 passed**. Includes Chat Completions tool
  correlation and assistant-message replay; truncated/unknown tool rejection; speech
  multipart/audio contracts; bounded bodies; no automatic retry on timeout; redirect and
  error redaction; configuration/TLS policy; static-file protection; API origin checks;
  and honest unconfigured readiness/status. Mock HTTP listeners required local network
  permission in this execution environment. No real provider requests were made.
- Configuration subprocess tests: **2 passed**. Real process environment overrides a
  private dotenv file, including an empty value disabling file configuration; invalid
  configuration exits without printing credential values.
- `cargo clippy --locked --offline -p gridcraft-server --all-targets -- -D warnings`:
  **passed**, including integration-test compilation.
- Existing live bridge and review contract tests: **14 + 8 passed**. The service reuses
  these contracts; the existing editor and bridge implementation were not changed.
- Native CLI `check-config`: **passed** with no configured services and no network calls.
- Actual HTTP process smoke check: liveness/status and the existing compiled editor index
  returned 200, readiness correctly returned 503 without PostgreSQL, WASM had the correct
  MIME type, and `.env` returned 404. The process stopped cleanly on SIGTERM. This used the
  earlier local browser artifact, not a newly built release container.
- WASM checking: **passed** using the already-documented local `build-std` workaround.
  The unchanged `backstage.rs` unused-variant warning remains. This checks that server-only
  dependencies do not enter the web application build.
- Layering: **17 crates, no violations**. Assets: **34 attributed files**, no assets added.
- The private `.env` was created with mode `600` and verified ignored by Git. Make's env
  bootstrap preserves existing values, refuses symlinks and does not source the file.
  The container context excludes dotenv, secrets, credential files and private certificates.

The five real PostgreSQL tests were **compiled but not run**. They are opt-in through
`make test-db` and never fall back to the normal `DATABASE_URL`. Four cover transactions,
workspace scope, retry recovery, durable rules and changed-source evidence; one exercises
the HTTP handlers with actual generated XLSX bytes and an exact snapshot acknowledgement.
Local PostgreSQL provisioning could not complete: container-image transfers were very slow,
and alternative temporary native/package downloads ended in read timeouts. No migration was
applied to a local or Aiven database. Database correctness is therefore an outstanding gate,
not established by the passing unit tests. Run `make test-db` against an explicitly selected
disposable PostgreSQL database before deploying this storage baseline.

The complete `cargo xtask ci` gate was rerun: formatting passed, but strict workspace clippy
still stopped at the existing macOS `clippy::let_unit_value` in
`crates/ui-egui/src/desktop_theme.rs:36`. Later stages were skipped by that command. The
separate server, engine, assets, layering and WASM checks above passed; full CI is not green.

Container construction was attempted with Podman. It reached Rust base-image downloads,
with slow measurable transfer, and was stopped before application compilation. The Dockerfile
and build inputs were reviewed; no successful image build or Aiven deployment is claimed.
Temporary network workarounds did not alter global toolchain, proxy or VM configuration.
All task-owned test servers and container/download jobs were stopped; no preview was left running.

No Aiven service credentials, deployment, customer data or live OpenAI/ElevenLabs calls were
used. The UI is still the offline demonstration; durable runs, connected browser saving,
voice capture and authentication remain separate delivery work.

## Original foundation evidence — 2026-10-21

The base checkout was clean at `8ae67c5534f3d7f145423a57206cf5fd08fe9221`.
That base pin does not contain this foundation. Record the reviewed foundation commit and
built artifact digest before deployment or distribution.
The exact checked paths and commands below are the reproducible evidence; no live Aiven,
Anthropic or ElevenLabs calls, deployment, credentials, customer data or Excel observation
were used. PostgreSQL schema is a proposal and has not been executed.

## Local checks

- `cargo test -p gridcraft-engine --lib`: **317 passed**. Includes 14 bridge cases and eight
  workflow cases, plus the existing engine tests. They cover formula readback, manual edits,
  revision gaps, wrong document/sheet, stale tokens, repeated writes/undo, receipt exhaustion,
  pending edits, malformed ranges, failed command rollback, manual calculation refusal,
  snapshots, exact sales/rule results, precision and tool continuation recovery.
- `cargo test -p gridcraft-ui-egui`: the complete UI library, integration and doc-test suite
  passed. This includes six Agentic workspace tests, two host snapshot tests and eight File
  menu tests. Rendered controls drive the same-session rule/preview/apply flow; manual changes
  invalidate previews, one native undo restores replaced cells, and pending drafts survive.
  Export captures the actual document and preserves edits made after upload began.
- `cargo check -p gridcraft --locked`: native application passed.
- `cargo run -p gridcraft-engine --example agentic_review -- /tmp/review.xlsx`: passed with
  a fresh destination. Wrote original synthetic XLSX; verified Jackets delta −4000 cents,
  growth −0.4, total delta +500 cents, replayed write and native undo. Persistence was an
  actual local file write, not a PostgreSQL transaction.
- `cargo xtask assets`: all 34 assets attributed. No new assets or vendor material added.
- `cargo xtask layers`: 16 crates, no violations.
- `cargo xtask parity --check`: generated checklist current; no Excel parity uplift claimed.
- Formatting checked with `cargo fmt --all -- --check`.

Strict native clippy encountered an existing macOS `clippy::let_unit_value` at
`crates/ui-egui/src/desktop_theme.rs:36` with the installed Rust 1.99.1. The affected engine/UI
libraries, tests and examples passed `cargo clippy ... -- -D warnings -A clippy::let_unit_value`.
That exception was command-local; repository lint policy and unrelated source were not changed.
Before the foundation commit, `cargo xtask ci` was run: formatting passed, then strict clippy
failed on that existing macOS warning. Later CI stages were skipped; the separate checks
recorded above remain valid. The full CI gate is not green.

## Browser bridge acceptance

The actual compiled WASM app was served locally and driven through a same-origin parent
using postMessage, with no mocked engine. A temporary test harness outside the repository
checked:

1. Hello/ready reconnects to the same mounted protocol-2 session.
2. Ready alone does not authorize actions; a successful load has its own correlated reply.
3. Formula write returns the live calculated result.
4. Identical request retry replays once; changed input under its ID is rejected.
5. A stale write is rejected; one native undo restores the previous value.
6. XLSX snapshot export returns bytes; acknowledging after another edit leaves it dirty.
7. Parent layout toggles preserve the same iframe/session.
8. Unsupported operations return errors without dialogs.
9. A malformed ZIP/XLSX returns correlated `open-error`.

Additionally, a real keyboard edit changed A1 to 7, and bridge inspection read 7 in the same
session. A pending keyboard draft reported `busy`; a bridge mutation was rejected until the
user cancelled it. Screenshots were inspected. Plain text renamed `.xlsx` is deliberately
not the malformed-file test: the existing native loader can recognize/import it as CSV.

Native offscreen renders of the actual Rust Agentic preview and applied workbook were also
inspected. The example renderer accepts `--cmd 'ui.agent.toggle={"on":true}'`,
`--cmd 'ui.agent.remember={}'`, `--cmd 'ui.agent.compare={}'` and optionally
`--cmd 'ui.agent.apply={}'` to reproduce those states. No external shell is needed.
The rebuilt WASM interface was also driven through its visible controls: enable Agentic,
remember the rule, compare, apply, switch Classic/Agentic, then undo. The sheet displayed
15000 to 15500 EUR cents with live formulas and readable provenance; undo restored the blank
sheet while preserving the conversation and rule. All nine bridge checks passed again on
that final build.

These transport checks do not prove durable browser-reload recovery, cross-user authorization,
real provider retries, a PostgreSQL commit or deployed HTTPS/microphone behavior.

## Build environment

This machine had Homebrew Rust 1.99.1 but no rustup WASM target, Trunk or WASM linker.
Dependencies were fetched into `/tmp/gridcraft-cargo-home`. The original web application
passed WASM checking. The implementation also compiled and linked using the installed Rust
standard-library sources and a temporary standalone LLD, with no global tool installation:

```sh
CARGO_HOME=/tmp/gridcraft-cargo-home RUSTC_BOOTSTRAP=1 \
  cargo -Z build-std=std,panic_abort check --target wasm32-unknown-unknown -p gridcraft-web
```

For the browser artifact, the same `build-std` command used `build`,
`CARGO_PROFILE_DEV_DEBUG=0` and a temporary `rust-lld` in PATH. Generated bindings used
`wasm-bindgen-cli 0.2.129`, matching `Cargo.lock`. This is a local toolchain workaround,
not a new project build requirement. Normal development uses a WASM target and Trunk as
documented in `apps/gridcraft-web/Trunk.toml` and `packaging/web/README.md`.
The web build still reports the pre-existing unused `Modified::At`/`Unknown` variants in
`backstage.rs`; it compiled and linked successfully.

## Remaining live gates

- Successful container build and Aiven deployment; any future multi-user authentication.
- Aiven PostgreSQL TLS/connectivity and real-instance migrations; durable run receipts.
- OpenAI tool lifecycle with actual provider failures and model availability.
- Snapshot upload acknowledgement and restoration after browser reload using persisted bytes.
- Microphone permission prompts across browsers (Safari `audio/mp4`) in a real browser session.
- Release-mode artifact, broader platform testing and the repository's full CI gate.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-22 | major | Checkpoint 4 bounded speech routes, browser recording and verified spoken outcomes |
| 2026-10-22 | major | Checkpoint 3 save receipts, exact retry/conflict and reload recovery |
| 2026-10-22 | major | Checkpoint 2 durable runs, fake-provider/DB/UI tests and live OpenAI smoke |
| 2026-10-22 | major | Rust service/configuration/provider checks, unchanged workbook contracts and explicit database/container/live integration gates |
| 2026-10-21 | major | Offline engine/UI checks, actual browser bridge evidence and explicit live-service limits |
