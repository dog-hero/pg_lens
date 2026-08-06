# Connecting pg_lens to PostgreSQL

pg_lens resolves a connection the way `psql` does, and adds one thing libpq
doesn't have: **a services file that can fetch the password from a command**,
so the secret never has to sit on disk in plaintext.

This page covers every way to connect, in the order you should reach for them.
For *which role to connect as* and what each lens needs, see
[the monitoring role page](connection-user.md).

---

## The short version

| You have… | Use |
|---|---|
| One database, quick look | `--dsn` or the libpq env vars |
| Several databases you return to | **[a services file](#3-the-services-file-recommended)** — the recommended setup |
| A team sharing one target list | [remote config](#remote-config--a-shared-services-file) (`--config-url`) |
| A secret in Vault / 1Password / Keychain | **[`password_cmd`](#password-resolution--never-store-a-secret)** |

---

## 1. `--dsn` — one-off

```sh
pg_lens --dsn "host=db.internal port=5432 user=pg_lens_ro dbname=app"
pg_lens --dsn "postgres://pg_lens_ro@db.internal:5432/app"
```

Both the `key=value` and URL forms work. Anything the DSN leaves out falls
through to the env vars, then to the defaults (`host=localhost user=postgres`).

Avoid putting the password in the DSN — it lands in your shell history and in
`ps` output. Use `PGPASSWORD` for a one-off, or a services file for anything
you'll run twice.

## 2. Environment variables — the libpq set

```sh
PGHOST=db.internal PGPORT=5432 PGUSER=pg_lens_ro PGPASSWORD=… pg_lens
```

| Env var | Maps to |
|---|---|
| `PGHOST` | hostname or Unix-socket directory |
| `PGPORT` | port |
| `PGDATABASE` | database name |
| `PGUSER` | user (default `postgres`) |
| `PGPASSWORD` | password — never displayed or logged |
| `PGAPPNAME` | `application_name` |
| `PGCONNECT_TIMEOUT` | connect timeout, whole seconds (`0` = wait forever) |

Empty values count as unset. Handy in CI and containers, where the orchestrator
already injects them.

## 3. The services file (recommended)

Register the databases you actually work with once, then connect by name:

```sh
pg_lens --service prod
pg_lens --list-services        # names + host/user — never secrets
```

The file lives at `~/.config/pg_lens/services.toml` (override with
`--services-file` or `PG_LENS_SERVICES_FILE`). It's inspired by libpq's
`pg_service.conf`, with one addition that changes everything: **`password_cmd`**.

```toml
[services.prod]
host = "db.prod.internal"
port = 5432
user = "pg_monitor_ro"
dbname = "app"
application_name = "pg_lens"
connect_timeout_secs = 5
password_cmd = "vault kv get -field=password secret/pg/prod"

[services.staging]
host = "db.staging.internal"
user = "postgres"
# sugar: a password of the form "$(...)" is treated as password_cmd
password = "$(op read op://infra/pg-staging/password)"

[services.local]
host = "localhost"
user = "postgres"
# macOS Keychain works too:
password_cmd = "security find-generic-password -s pg_local -w"
```

Select a service by flag or environment: `--service prod`,
`PG_LENS_SERVICE=prod`, or the standard `PGSERVICE=prod`.

Any field a service leaves out falls through to the env vars and defaults, so a
service can be as small as `host` + `user`.

### Interactive pickers — when you pass nothing

Start `pg_lens` with **no** connection hints (no `--dsn`, no `--service`, and
none of `PGHOST`/`PGSERVICE`/`PG_LENS_SERVICE`/`PG_LENS_DSN`) while a valid
services file with at least one entry exists, and it won't connect blindly — it
opens a picker listing every service as `name — user@host`, plus a
`localhost — (default)` entry. `j`/`k`/`↑`/`↓` move, `Enter` connects,
`q`/`Esc` quits.

`pg_lens serve` does the same on a terminal: a numbered prompt accepting a
number or a service name (a single defined service is auto-selected with a
notice). When stdin is **not** a TTY — systemd, a container, CI — it refuses
instead of prompting, printing the available service names and exiting
non-zero, so a daemon can never silently connect to the wrong server.

---

## Password resolution — never store a secret

This is the reason to use a services file. Three options, worst to best:

**Plaintext `password`** — works, discouraged. pg_lens refuses to read a file
that combines a plaintext password with group/other read permission.

**`password_cmd`** — an external command whose trimmed stdout becomes the
password:

```toml
password_cmd = "vault kv get -field=password secret/pg/prod"
password_cmd = "security find-generic-password -s pg_local -w"   # macOS Keychain
password_cmd = "op read op://infra/pg-prod/password"             # 1Password CLI
password_cmd = "aws secretsmanager get-secret-value --secret-id pg/prod --query SecretString --output text"
password_cmd = "gcloud secrets versions access latest --secret=pg-prod"
password_cmd = "pass show infra/pg/prod"                         # pass / gpg
```

**`password = "$(...)"`** — sugar for exactly the same thing, handy when the
command is short.

### How it behaves

- Runs as `sh -c <cmd>` with a **10-second timeout**.
- **Re-executed on every (re)connection attempt** — not cached at startup. That
  is what makes short-lived credentials work: a Vault lease, an SSO helper, an
  IAM token that rotates every 15 minutes keeps working across reconnects,
  because pg_lens asks again each time it reconnects.
- If the command fails, the TUI **stays alive** and shows the error in the
  banner — taken from the command's **stderr**, since stdout is treated as the
  secret and is never printed — retrying with backoff.
- The resolved secret lives only in memory for the connection. It never reaches
  a log line, the JSON API, the header, or `--list-services` output.

### File permissions

The services file can execute commands — treat it like code:

```sh
chmod 600 ~/.config/pg_lens/services.toml
```

pg_lens **refuses** a services file that is writable by group or others, and
refuses one that combines a plaintext `password` with group/other read
permission.

---

## Remote config — a shared services file

For a team that wants everyone pointed at the same curated list instead of
copying `services.toml` around:

```sh
pg_lens --config-url "github:my-org/infra/pg_lens/services.toml@main"
pg_lens --config-url "https://config.internal.example.com/pg_lens/services.toml"
```

or in `~/.config/pg_lens/config.toml`:

```toml
remote_config = "github:my-org/infra/pg_lens/services.toml@main"
```

Two forms: **`github:OWNER/REPO/PATH[@REF]`** (GitHub Contents API — works for
**private** repos with a token; `@REF` defaults to the repo's default branch),
or a verbatim `https://`/`http://` URL.

The token is resolved in this order and **never written to a file**:
`PG_LENS_CONFIG_TOKEN` → `GITHUB_TOKEN` → `remote_config_token_cmd` in
`config.toml` (an external command with trimmed stdout — the same pattern as
`password_cmd`). It is sent as `Authorization: Bearer <token>` and is **refused
outright over plain `http://`**.

A successful fetch is cached at `$XDG_CACHE_HOME/pg_lens/remote-services.toml`
(mode `0600`). If the fetch fails — network down, bad token, repo moved —
pg_lens falls back to that cache, then to the local services file, warning once
on stderr at each step. Startup never blocks on a flaky network (10s timeout),
and only hard-fails when there is genuinely nothing to connect with. Remote
entries win a same-named collision; local-only entries still work. The fetch is
strictly **read-only** — pg_lens never writes back to the remote source.

The shared file should carry `password_cmd`, not passwords: the team list then
holds only *where* to connect, and each engineer's own vault or keychain
supplies the secret.

---

## Precedence

Highest wins:

1. `--dsn` field
2. services-file entry (`--service`; remote entries override same-named local ones)
3. environment variable
4. default (`host=localhost`, `user=postgres`)

`--dsn` and `--service` are **mutually exclusive** — passing both is an error in
any flag position, so you can never silently connect somewhere you didn't mean
to. The header always shows the resolved `user@host`; the password appears
nowhere.

---

## Connection poolers

pg_lens works **directly**, through a **session-pooling** proxy, and as a
restricted non-superuser on managed services (RDS/Aurora, Cloud SQL, Azure).
Each poll runs in a short read-only transaction with its own
`statement_timeout`, so it is a good citizen on a shared server.

The one unsupported setup is a pooler in **transaction pooling** mode
(PgBouncer's default `pool_mode = transaction`, Supabase port `6543`,
Supavisor's transaction mode): named prepared statements get routed to
different backends and fail. Point pg_lens at the **session or direct
endpoint** instead (Supabase: port `5432`), or run PgBouncer ≥ 1.21 with
`max_prepared_statements > 0`.

---

## Related

- [The monitoring role & least-privilege grants](connection-user.md) — which
  role to create, and what each lens needs to light up.
- The [README](https://github.com/dog-hero/pg_lens#readme) — full flag
  reference, config file, and the Web Lens.
