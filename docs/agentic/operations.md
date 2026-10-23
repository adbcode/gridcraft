# Running the service baseline

> Last reviewed: 2026-10-23 · Last updated: 2026-10-23 · Change: minor

The Rust server serves the existing GridCraft WASM editor and its HTTP API from one
origin. PostgreSQL stores application data and bounded workbook files. OpenAI Chat
Completions and ElevenLabs use backend-only HTTP adapters. The editor's Agentic panel
still runs its labelled local demo; adding credentials does not connect that panel
to a durable model loop. Lovable remains a separate landing-page project.

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
| `DATABASE_SSL_ROOT_CERT_BASE64` | Single-line standard base64 of the entire PEM file; avoids multiline environment transport |
| `OPENAI_API_KEY` | Backend-only provider credential |
| `OPENAI_BASE_URL`, `OPENAI_MODEL` | Chat Completions endpoint prefix and configured model |
| `OPENAI_REASONING_EFFORT` | Optional `reasoning_effort` request field; set `none` for models that reject function tools with reasoning on chat completions |
| `ELEVENLABS_API_KEY` | Backend-only speech credential |
| `ELEVENLABS_BASE_URL` | Speech API endpoint prefix |
| `ELEVENLABS_STT_MODEL`, `ELEVENLABS_TTS_MODEL`, `ELEVENLABS_VOICE_ID` | Explicit speech configuration |
| `GRIDCRAFT_WORKSPACE_ID` | Fixed mock workspace; keep stable to retain access to saved data |
| `GRIDCRAFT_BIND`, `PORT`, `GRIDCRAFT_STATIC_DIR` | Listener and compiled web asset directory |
| `GRIDCRAFT_MIGRATE_ON_START` | Opt-in, locked schema migration before serving |
| `GRIDCRAFT_ENV_FILE` | Optional alternative private dotenv path, selected through the process environment |

Choose exactly one of the CA path, PEM or BASE64 settings. The server reads and validates
explicit CA inputs locally before attempting a connection, including during `check-config`.
Invalid base64, escaped PEM newlines, missing certificates and invalid X.509 data produce
fixed configuration errors without printing the supplied value. Inputs are limited to 1 MiB
after decoding. Parsing is not a trust/expiry/hostname check; those checks happen during
the `verify-full` connection. Keep local certificate files under ignored
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
and explicitly reports the offline UI and missing durable agent loop. Payloads and file
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
`DATABASE_SSL_MODE=verify-full`. Paste the complete project CA certificate with real
line breaks and its `BEGIN CERTIFICATE` / `END CERTIFICATE` lines, not a path, quoted
string or literal `\n` escapes. Keep the image's listener and asset-path defaults:
`GRIDCRAFT_BIND=0.0.0.0` and `GRIDCRAFT_STATIC_DIR=/app/web` (leading slash required;
`app/web` resolves to `/app/app/web` under the container's `/app` working directory).
An environment change redeploys the previously selected commit; deploying newer code
requires **Actions → Deploy latest commit**. See
[Runtime variables](https://aiven.io/docs/products/runtime/secrets-and-variables) and
[deployment documentation](https://aiven.io/docs/products/runtime/deploy-apps).

If multiline values are unreliable, prefer `DATABASE_SSL_ROOT_CERT_BASE64` instead of
weakening TLS or committing a deployment-specific CA into the image. Encode the **entire
downloaded PEM file**, not just its existing base64 body:

```sh
python3 -c 'import base64,pathlib; print(base64.b64encode(pathlib.Path("ca.pem").read_bytes()).decode("ascii"))'
```

Set that single-line output as `DATABASE_SSL_ROOT_CERT_BASE64`, and remove/unset both
`DATABASE_SSL_ROOT_CERT_PEM` and `DATABASE_SSL_ROOT_CERT`. Keep `DATABASE_SSL_MODE=verify-full`
and the integration-managed `DATABASE_URL`. This requires a build supporting the BASE64
setting; older builds ignore it. Startup reports the CA source using a fixed label, never
its contents. If a platform supports mounting configuration files, a readable mounted PEM
at `DATABASE_SSL_ROOT_CERT` is another option; a path must exist inside the container and
be readable by UID 10001. Merely naming a local path does not upload a file to Runtime.

After deployment, check the public editor and API health endpoints, then validate
PostgreSQL writes and file reloads before connecting live model or voice actions.
Container construction, an available key and local HTTP mocks are separate from
a successful Aiven deployment or live provider response.

### Diagnosing startup migration failures

Startup migrations open a bounded, dedicated connection using the configured TLS
options. Logs identify the failing phase, migration version (when known) and SQLSTATE
(for PostgreSQL errors), but never print driver messages, SQL, credentials or database
identifiers. HTTP storage errors remain generic. `check-config` only validates local
settings; it cannot prove a database connection or certificate handshake works.

| Diagnostic | Check before redeploying |
|---|---|
| Invalid service configuration / PostgreSQL CA | Correct the named encoding/readability error; no database connection was attempted |
| Connection / endpoint refused TLS | Effective PostgreSQL hostname/port; the endpoint declined PostgreSQL's TLS request |
| Connection / TLS setup or TLS/protocol negotiation | Effective host, matching CA, and any client certificate URI options; retain `verify-full` |
| Connection / network failure or timeout | PostgreSQL service running, correct host/port and Runtime-to-database network access |
| SQLSTATE `28P01` / `28000` | Database credentials and access policy; URI-encode special characters in passwords |
| SQLSTATE `3D000` | Database name in the service connection URI |
| SQLSTATE `42501` | Migration role's schema/table creation and migration-lock privileges |
| SQLSTATE `42P07` / `42710` | Existing objects versus `_sqlx_migrations`; do not drop customer tables to force startup |
| Checksum mismatch / missing migration | Deploy the matching immutable migration history; do not delete or rewrite `_sqlx_migrations` |

This branch (`feat/agentic-workspace`) embeds migration versions 1 and 2. The separate
`feat/agentic-workspace-connected` branch embeds versions 1 and 3. Switching between
them against an already migrated database can fail history validation; do not switch
branches or rewrite history as a diagnostic workaround. This change preserves both
SQL files on the workspace branch byte-for-byte.

If `_sqlx_migrations` is absent from every schema in the database used by Runtime,
there is no recorded migration history to conflict with. Investigate the connection
and migration setup first, verifying that a query editor uses the same database and
role as Runtime. The application's public allowlist does not establish PostgreSQL-side
network access. A generic `database migration failed` from an older build does not
establish that migration SQL ran. Deploy the diagnostics build on the intended branch
and inspect its next startup log before changing schema or weakening TLS. Model/voice
settings are unrelated: no provider is contacted during migrations.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-22 | minor | Secret-safe startup diagnostics for the workspace migration history, CA formatting and absolute container asset path |
| 2026-10-22 | major | Local commands, private environment files and one-origin Aiven container baseline |
