# Workspace, service and foundation validation

> Last reviewed: 2026-10-22 · Last updated: 2026-10-22 · Change: minor

## Migration startup diagnostics — 2026-10-22

Verified on local PostgreSQL 14 using an explicitly disposable database, not Aiven:

- `make check`: formatting, locked server compilation and strict server/all-target clippy passed.
- `make test`'s equivalent `cargo test --locked -p gridcraft-server`: **24 passed**;
  the opt-in database tests were run separately below.
- `make test-db`: all **12 passed**, including four new migration regressions: version 1 → 3
  upgrade/repeat, schema conflict and lock release, checksum mismatch without history repair,
  and missing-database diagnostics without leaking its name. New schema-changing tests use
  random isolated schemas and remove them on success.
- Compiled server CLI: **9 scenarios passed** — successful/repeated migrations, verified TLS
  with multiline PEM, literal newline escapes, bad password, missing database, untrusted CA,
  hostname mismatch, invalid PEM and connection refusal. Failure output was checked for
  credential/database-name leakage; no raw driver messages are retained by diagnostics.
- `cargo xtask ci`: formatting and strict workspace clippy passed; workspace tests stopped at
  unchanged UI test `comments_pane::review_has_a_single_show_comments_button` (expected one
  New Comment tooltip, found zero). The same test failed when rerun independently; no UI
  code or test was changed. This is **not** a green full CI result.
- Assets, dependency layers, WASM compilation, generated parity and keytips checks passed independently.

No production root cause or successful Aiven deployment is claimed. The old generic log
discards the underlying error. Deploy this diagnostics build, retain `verify-full` and inspect
the next startup error before altering migration history. The container asset path must be
`/app/web`, not `app/web`; that is separate from database migration startup.

## Connected workspace — 2026-10-22

Implemented in the isolated `feat/agentic-workspace` worktree at
`/private/tmp/gridcraft-agentic-workspace`, based on verified service commit
`2bfd1f9cb9a876bd4773d0147d52f927ad106baf`. The original checkout remains on
`feat/aiven-service-baseline`; its untracked `ca.pem`, files and baseline build were not
modified or stopped. All new target/dist outputs belong to the isolated worktree.

### Focused automated checks

- Engine library: **318 passed**, including original XLSX generation/normal-load round-trip,
  EUR/formula results and the existing live bridge/retry/undo/snapshot contracts.
- UI acceptance: **19 passed** (9 workspace, 8 File menu, 2 host snapshot). Includes connected
  dispatch/cancellation, stale tools preserving manual edits, mode/draft/history retention,
  formula readback, exact late acknowledgements and platform file-picker invocation.
- Server unit/local HTTP adapter tests: **20 passed**, plus **2 CLI configuration tests**.
  Provider HTTP calls in these tests target loopback mocks, never actual accounts.
- Real disposable PostgreSQL: **8 passed** (3 durable-run, 1 HTTP/storage, 4 transaction
  suites). Covers restart/new-connection rule persistence, 65500 → 15500 cents, a source
  correction to 14500 with unchanged earlier evidence, version allocation and exact concurrent
  snapshot retries, durable provider-failure recovery, correlated tool retries, stale proposals
  and refusal to save an unsolicited model-selected rule.
- Native application and Rust server built. The browser compiled and linked to WASM, then
  generated bindings with the lockfile-matched wasm-bindgen 0.2.129.
- `cargo xtask wasm`: all 11 selected crates passed through a task-local Cargo wrapper that
  adds `-Z build-std=std,panic_abort`. This is the documented temporary Homebrew toolchain
  workaround, not a normal installed WASM target or a new repository requirement.
- Strict server clippy passed. UI clippy passed with a command-local allowance for the
  pre-existing `clippy::let_unit_value`; WASM clippy also passed with command-local allowances
  for that warning and the existing dead-code warning. Repository lint policy was not weakened.
- Assets: **34 attributed**, no added assets; layers: **17 crates**, no violations; parity
  checklist current. No spreadsheet feature/parity uplift is claimed.

`cargo xtask ci` was run again on this work. Formatting passed; strict workspace clippy
stopped at the existing `crates/ui-egui/src/desktop_theme.rs:36` unit-value binding. Its
remaining test/assets/layers/WASM/parity/keytips stages were skipped by that command. The
separate checks above are actual evidence; full CI is still not green. The web build also
retains the existing unused `Modified::At`/`Unknown` warning. No unrelated lint source changed.

### Actual compiled browser acceptance

Chrome loaded the real WASM editor from the Rust server on port **8081**. The service used
an isolated PostgreSQL 17.11 cluster on loopback **55432** and temporary HTTP OpenAI/ElevenLabs
mocks on **9091**. The PostgreSQL package was SHA-256 verified and relocated entirely under
`/tmp`; no global toolchain/server installation, Aiven data or real credentials were used.

Observed in the mounted editor:

1. The default XLSX opens in Classic with Review, Orders, and Sources and Checks. The sheet
   shows EUR 150 / 655 with working formulas and percentages. Browser inspection caught and
   corrected column widths expressed in the wrong unit and an unavailable arrow glyph.
2. Compare 655 → discard → explicitly remember the exclusion → compare 155 → Apply returns
   the actual bridge readback and exactly one completed write. Sources retain query/rule IDs,
   completed periods, row counts, revision and original records.
3. After a manual cell edit, Classic/Agentic switching retained the workbook and history.
   Native Undo restored C5's `=SUM(C2:C4)` formula and value **155**, confirmed by live inspection.
4. A stale write after a keyboard edit returned `conflict`, retained the manual cell, and
   recorded zero completed writes. The visible error remains available for acknowledgement.
5. Delaying a committed upload response, editing while it was in flight, then releasing the
   receipt left the later edit dirty. Reload restored the local dirty XLSX and cloud version.
6. Reload with an outstanding write returned the previous run for inspection without applying
   it. A fresh request used a new mounted session ID and read the preserved `manual` cell.
7. Library search, exact-version attachment and open worked. Quarterly Forecast live reads
   returned `Quarter`, **12000** revenue and **8400** cost; the start request carried its exact
   workbook/version attachment. A local Inventory XLSX delivered by browser file drop opened
   through the normal loader; live inspection read `Product` and **24** units. The native
   rendered picker contract and actual Chrome file chooser also opened the local XLSX.
8. Fake browser microphone capture produced editable `Compare sales` text without starting
   a run. Send used the same context/inspect/write workflow and 15500-cent result as typing.
   Cancelling a recording produced no transcription request or new run. The speech endpoint
   returned generated silent MP3; playback/pause controls were exercised. This validates
   capture/transport/control flow, not real speech recognition or voice quality.

Screenshots of Classic, Agentic, proposed/applied results, library, live file reads, stale
failure, late save acknowledgement, reload, transcript and playback were inspected. Browser
harnesses, generated artifacts, private preview configuration, database files and screenshots
remain outside committed source. No handwritten JS/TS was added to the product.

### Remaining limits and handoff

The running local preview is `http://127.0.0.1:8081/?webgl`, using local mock providers.
It is not the Aiven deployment. Browser recovery is bounded same-tab session storage; it
waits for unfinished interactions and restores content without native undo history. Competing
cloud version conflicts preserve the original retry and local edits; an explicit new-version
or save-copy resolution UI is a later iteration. The library currently lists the first 50
files. Only XLSX attachments and the initially bounded bridge tools are exposed.

Aiven image construction/deployment, PostgreSQL TLS on that service, real OpenAI tool calls,
live ElevenLabs STT/TTS, public HTTPS permissions, release artifacts and broader platform
checks remain unexecuted. Continuous voice agents remain deferred. Commit/push this branch,
then have the user select it in Aiven before deployed testing; leave the baseline unchanged.

The dated records below describe earlier states and are retained as historical evidence.

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
- ElevenLabs STT/TTS, microphone permission, editable transcript and truthful spoken completion.
- Release-mode artifact, broader platform testing and the repository's full CI gate.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-22 | minor | Local migration/TLS/failure diagnostics evidence and unchanged full-CI UI failure; Aiven remains unverified |
| 2026-10-22 | major | Connected workspace; real disposable database, browser, generated files, recovery and mock voice checks |
| 2026-10-22 | major | Rust service/configuration/provider checks, unchanged workbook contracts and explicit database/container/live integration gates |
| 2026-10-21 | major | Offline engine/UI checks, actual browser bridge evidence and explicit live-service limits |
