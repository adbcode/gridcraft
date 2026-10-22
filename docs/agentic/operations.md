# Running the connected workspace

> Last reviewed: 2026-10-22 · Last updated: 2026-10-22 · Change: major

The Rust server serves the existing GridCraft WASM editor and its HTTP API from one
origin. PostgreSQL stores application data and bounded workbook files. OpenAI Chat
Completions and ElevenLabs use backend-only HTTP adapters. The browser panel connects
when database/model configuration is available; the native/standalone panel labels its local
demonstration. Lovable remains a separate landing-page project.

## Local setup

Use a Rust toolchain with the `wasm32-unknown-unknown` target and Trunk. The container
build pins Rust 1.99.1, Trunk 0.21.14, Binaryen 131 and wasm-bindgen CLI 0.2.129; the
bindgen version must match `Cargo.lock`. A normal rustup installation can prepare the
web build with:

```sh
rustup target add wasm32-unknown-unknown
cargo install --locked --version 0.21.14 trunk
make env
```

`make env` creates `.env` with mode `600`, or preserves the contents of an existing
regular `.env` and restricts its permissions. Edit the file locally. It is ignored by
Git and excluded from the container build context. Neither Make nor the server sources
it as a shell script. Process environment variables override file values, so the same
configuration names work locally and in Aiven Runtime.

| Setting | Purpose |
|---|---|
| `DATABASE_URL` | Existing PostgreSQL connection URI, including credentials |
| `DATABASE_SSL_MODE` | `verify-full` for Aiven; `disable` only for a local loopback database |
| `DATABASE_SSL_ROOT_CERT` | Local path to the Aiven project CA certificate |
| `DATABASE_SSL_ROOT_CERT_PEM` | Alternative certificate contents, useful as a platform variable |
| `OPENAI_API_KEY` | Backend-only provider credential |
| `OPENAI_BASE_URL`, `OPENAI_MODEL` | Chat Completions endpoint prefix and configured model |
| `ELEVENLABS_API_KEY` | Backend-only speech credential |
| `ELEVENLABS_BASE_URL` | Speech API endpoint prefix |
| `ELEVENLABS_STT_MODEL`, `ELEVENLABS_TTS_MODEL`, `ELEVENLABS_VOICE_ID` | Explicit speech configuration |
| `GRIDCRAFT_WORKSPACE_ID` | Fixed mock workspace; keep stable to retain access to saved data |
| `GRIDCRAFT_BIND`, `PORT`, `GRIDCRAFT_STATIC_DIR` | Listener and compiled web asset directory |
| `GRIDCRAFT_MIGRATE_ON_START` | Opt-in, locked schema migration before serving |
| `GRIDCRAFT_ENV_FILE` | Optional alternative private dotenv path, selected through the process environment |

Choose either the CA path or PEM setting. Keep local certificate files under ignored
`secrets/`; never put secrets or certificates in `dist/web`, which is publicly served.
Use the database connection details for the existing instance. An Aiven management
token is not a database password and is not needed by the application.

```sh
make check-config
make build
make run
```

Open `http://127.0.0.1:8080`. `make run` stays in the foreground; **Ctrl-C stops it**.
It does not create a background service. `make run-native` opens the native spreadsheet;
`make web-dev` starts a standalone Trunk editor preview on port 8771 without an API
proxy. Use the server origin when checking backend routes.

`GET /api/health/live` reports the running process. `GET /api/health/ready` requires the
database, implemented schema and `index.html`; an unconfigured offline instance returns
503 there. `GET /api/status` separates configured providers from unprobed connectivity
and reports whether the connected durable workflow is configured. Payloads and file
receipt handling are documented in [the service API](service-api.md).

`make build-server` and `make build-web` can be run separately. `CARGO`, `TRUNK`,
`CARGO_HOME`, and the normal Rust environment remain overridable. If a machine has
Homebrew Rust without rustup or a WASM target, see the project's
[conditional toolchain recovery notes](../../.agents/skills/gridcraft-web-validation/references/local-toolchain.md).
Those diagnostic workarounds are not release build defaults.

## Database lifecycle and checks

Schema changes and synthetic data insertion are explicit:

```sh
make db-migrate
make db-seed-demo
make test
make check
```

Migrations record their versions and use a database lock. The seed command targets
the configured mock workspace. Ordinary server startup does not seed records. Keep
`GRIDCRAFT_MIGRATE_ON_START=false` when applying migrations separately; set it to
`true` for the small Aiven deployment if startup is the chosen migration step.

Rules saved through the service persist in PostgreSQL; the editor panel's demo rules
remain session-only until its HTTP workflow is connected. Typed sales imports update
source data and a revision. Comparison responses identify immutable query evidence with
the original rows and saved rules, so a later correction cannot rewrite an earlier answer.

`make test` runs local tests and provider HTTP mocks without paid service calls.
`make test-db` is opt-in and requires `GRIDCRAFT_TEST_DATABASE_URL` pointing to a
disposable PostgreSQL database. It does not fall back to your normal `DATABASE_URL`.
The database tests may create schema and fixtures. `make ci` runs the full repository
gate; see [the validation record](validation.md) for actual results and known limits.

This is one shared mock workspace, not user authentication. PostgreSQL persistence
does not by itself add a login or isolate visitors. The existing editor bridge still
owns live workbook state; a database snapshot receipt must be acknowledged back to
that exact browser snapshot before its saved state changes.

## Aiven Runtime

A container is recommended here because it builds the Rust server and WASM assets
together and gives Aiven one HTTP process to launch. Runtime discovers the root
`Dockerfile` and its exposed port. The image runs as a non-root user, binds
`0.0.0.0:8080`, and contains only the server binary, compiled web assets and public
system CA certificates. [Aiven Containerfile documentation](https://aiven.io/docs/products/runtime/manifest-files/containerfiles)
describes manifest and port discovery.

Build the same artifact locally with `make container-build` (or
`make container-build CONTAINER=podman`). This requires a working container engine
and network access to Rust, package and tool registries. The build does not use your
`.env`; do not pass provider credentials as build arguments.

In Aiven Runtime, select this repository, the reviewed branch, the repository root
as build context and `Dockerfile` as the manifest. Connect the **existing** PostgreSQL
service and map its connection URI to `DATABASE_URL`. Aiven's
[service integration documentation](https://aiven.io/docs/products/runtime/connect-services-to-apps)
describes using an existing service.

Set the remaining values in Runtime's environment configuration. Mark
`OPENAI_API_KEY`, `ELEVENLABS_API_KEY` and manually supplied database credentials as
**Secret**. Set the Aiven project CA through `DATABASE_SSL_ROOT_CERT_PEM` and retain
`DATABASE_SSL_MODE=verify-full`. Keep the image's listener and asset-path defaults.
An environment change redeploys the previously selected commit; deploying newer code
requires **Actions → Deploy latest commit**. See
[Runtime variables](https://aiven.io/docs/products/runtime/secrets-and-variables) and
[deployment documentation](https://aiven.io/docs/products/runtime/deploy-apps).

After deployment, check the public editor and API health endpoints, then validate
PostgreSQL writes and file reloads before connecting live model or voice actions.
Container construction, an available key and local HTTP mocks are separate from
a successful Aiven deployment or live provider response.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-22 | major | Local commands, private environment files and one-origin Aiven container baseline |


## Isolated workspace development

Use a separate worktree with its own target/dist output and `PORT=8081`; a standalone Trunk
preview may use `--port 8772`. Keep an existing baseline checkout/build untouched. Configure
a disposable loopback database for `make test-db`, and explicitly migrate/seed it. Local
mock-provider acceptance can point the existing configurable HTTP adapters at loopback;
never substitute mock acceptance for actual provider or Aiven validation.

The new migration adds durable run checkpoints. `seed-demo` also creates the three mock
library XLSX files without enabling a rule. Production seeding remains explicit. To try the
first slice, open the server origin, select Agentic, compare sales, discard the initial
preview, explicitly remember the exclusion, compare again, inspect/apply, manually edit,
switch Classic/Agentic and Undo. Save acknowledges a specific committed snapshot; later
changes remain dirty. See [workspace recovery](workspace.md) before testing interruptions.
