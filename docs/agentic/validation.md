# Foundation validation

> Last reviewed: 2026-10-21 · Last updated: 2026-10-21 · Change: major

## Evidence and boundaries

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

- Real service adapters and deployment configuration, authentication/authorization.
- PostgreSQL migration/query/transaction tests, restart-safe run and snapshot receipts.
- Anthropic tool lifecycle with actual provider failures and model availability.
- Snapshot upload acknowledgement and restoration after browser reload using persisted bytes.
- ElevenLabs STT/TTS, microphone permission, editable transcript and truthful spoken completion.
- Release-mode artifact, broader platform testing and the repository's full CI gate.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-21 | major | Offline engine/UI checks, actual browser bridge evidence and explicit live-service limits |
