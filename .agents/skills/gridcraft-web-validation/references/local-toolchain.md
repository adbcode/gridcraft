# Conditional local toolchain recovery

Use these lessons only when the normal Rust/Trunk path fails. Recheck the current machine;
the initial validation used Homebrew Rust 1.99.1 and temporary tools in October 2026. Those
versions and temporary directories are not project requirements and may no longer exist.

- Inspect `rustc --version`, `cargo --version`, the compiler sysroot, available `rustup`,
  `trunk`, `wasm-bindgen` and linker executables. Check `Cargo.lock` for the bindgen version.
- A read-only Cargo cache needs a writable task-local `CARGO_HOME`, not dependency changes.
  The original run used `/tmp/gridcraft-cargo-home`; choose or verify a path for this run.
  Run offline only after dependencies exist. Do not read credentials or `.env` to fix builds.
- With rustup, install/use the standard `wasm32-unknown-unknown` target. Use existing Trunk
  or a task-local installation. Respect environment permissions for network or installation.
- If rustup is absent but matching Rust standard-library sources exist, a diagnostic fallback
  that worked was command-scoped `RUSTC_BOOTSTRAP=1 cargo -Z build-std=std,panic_abort check
  --target wasm32-unknown-unknown -p gridcraft-web`. Linking uses `build` instead of `check`
  and requires a compatible `rust-lld`/WASM linker. The original run extracted a standalone
  LLD into a temporary directory and prepended its executable directory to PATH. Discover
  suitable trusted tooling; do not hard-code the old Homebrew bottle/version or modify global
  PATH/Rust configuration. Report this as a workaround, not normal CI or release validation.
- For a temporary browser artifact without Trunk, match `wasm-bindgen-cli` to Cargo.lock and
  generate web bindings outside the repo. Copy the application's HTML into that temporary
  output, remove Trunk-only directives and initialize the generated bindings there. Keep the
  original HTML/assets unchanged. Serve with correct MIME types and a real browser renderer.
  A successfully linked artifact plus browser interaction is stronger evidence than check alone.
- A GPU-denied sandbox can make the offscreen renderer report no adapter. Preserve the error;
  use the available browser or permitted rendering environment. Do not change the app's
  renderer to hide an environment restriction.
- Initial strict macOS clippy failed on `clippy::let_unit_value` at
  `crates/ui-egui/src/desktop_theme.rs:36`; WASM emitted an existing dead-code warning for
  `Modified::At`/`Unknown` in `backstage.rs`. Verify whether those still exist before classifying
  a new failure. A diagnostic `-A clippy::let_unit_value` was used only on the affected native
  clippy command; it was never written to repository policy or claimed as strict CI success.

Keep generated bindings, debug WASM, downloaded toolchains, Cargo caches, local harnesses and
screenshots out of commits. Track only source, tests and maintained instructions. Do not commit
machine-specific temporary paths as mandatory project configuration.
