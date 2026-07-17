# The pg_lens monitoring role — creating the user & least-privilege grants

pg_lens is a **read-only observability tool**. It never issues DDL or DML on
your data; the only writes it can perform are two explicit admin actions you
trigger by hand (`c` cancel a query, `K` terminate a backend), and even those
are signals, not data changes. This page shows how to create a dedicated role
for it and exactly which privileges each lens needs — so you can hand pg_lens
the *least* privilege that still lights up the panels you care about.

> TL;DR — for almost everyone, this is the whole answer:
>
> ```sql
> CREATE ROLE pg_lens_ro LOGIN PASSWORD 'change-me';
> GRANT pg_monitor TO pg_lens_ro;
> ```
>
> `pg_monitor` is a PostgreSQL **predefined role** (PG 10+) that bundles
> `pg_read_all_settings`, `pg_read_all_stats`, and `pg_stat_scan_tables`. It is
> read-only and unlocks every lens except the two admin actions. Managed
> providers expose it: RDS/Aurora `GRANT pg_monitor TO ...`, Cloud SQL and
> Azure grant it to your admin role so you can pass it on.

Then connect with it — see [Connecting](../README.md#connecting):

```sh
PGPASSWORD='change-me' pg_lens --dsn "host=db.internal user=pg_lens_ro dbname=appdb"
```

Store the password out of the file with a `password_cmd` in the
[services file](../README.md#services-file) — pg_lens never needs the secret
written in plaintext.

---

## What each lens needs, and what happens without it

pg_lens is built so a **missing privilege degrades to an absent panel, never a
crash or a dead poll** — a denied view just doesn't render. So you can grant
narrowly and add privileges only for the lenses you want. This table maps lens
to the privilege that lights it up.

| Lens / panel | Data source | Needs | Without it |
|---|---|---|---|
| **Macro Lens** — vitals, TPS, sessions | `pg_stat_database`, `pg_stat_activity` | base connect; **`pg_read_all_stats`** (in `pg_monitor`) to see *other* users' rows | you see only your own sessions; aggregate counts under-report |
| **Micro Lens** — full activity, query text, waits, xact-age, blocking chains | `pg_stat_activity` | **`pg_read_all_stats`** — otherwise `query`, `state` etc. of other backends are `NULL`/hidden | only your own backends visible; blocking chains can't resolve the root blocker |
| **Query Lens** | `pg_stat_statements` | the **extension installed** (`CREATE EXTENSION pg_stat_statements` + `shared_preload_libraries`) **and** read access — `pg_monitor` grants it | friendly "extension missing/old" explainer; panel absent |
| **Schema Lens** — table stats & sizes | `pg_stat_user_tables`, `pg_relation_size(...)` | readable by any role for its own tables; **`pg_stat_scan_tables`** / table `SELECT` for full coverage | tables you can't see are omitted |
| **Schema Lens** — estimated bloat (`R`) | `pg_class`, `pg_stats` | **`pg_read_all_stats`** (statistics rows) | bloat estimate blank; the rest of the lens still works |
| **Vacuum sub-view** (`v`) — XID age, progress, prepared xacts | `pg_class`, `pg_stat_progress_vacuum`, `pg_prepared_xacts` | **`pg_read_all_stats`** to see other backends' vacuum progress; `pg_prepared_xacts` is world-readable | wraparound headline still works; you only see *your* vacuum progress |
| **Replication Lens** — senders, receivers, slots | `pg_stat_replication`, `pg_replication_slots`, `pg_stat_wal_receiver` | **`pg_read_all_stats`** (the lag/LSN columns are privileged) | panel absent on restricted roles (this is expected on RDS replicas etc.) |
| **Index Lens** — unused/redundant indexes | `pg_stat_user_indexes`, catalogs | same as Schema Lens | indexes on unreadable tables omitted |
| **Admin actions** — `c` cancel, `K` terminate | `pg_cancel_backend()`, `pg_terminate_backend()` | see below — **not** covered by `pg_monitor` | the action returns the server's "insufficient privilege" error; nothing else breaks |

### The two admin actions are a separate, opt-in privilege

`pg_monitor` is purely read-only and does **not** let you cancel or terminate
other users' backends. PostgreSQL only lets a role signal backends **owned by
the same login role**, unless it is a member of
[`pg_signal_backend`](https://www.postgresql.org/docs/current/predefined-roles.html):

```sql
-- only if you want `c`/`K` to work against OTHER users' backends:
GRANT pg_signal_backend TO pg_lens_ro;
```

`pg_signal_backend` still cannot signal superuser backends. If you want pg_lens
to be strictly look-but-don't-touch, **omit this grant** — the panels all work,
and `c`/`K` simply report a permission error when used. (A first-class
read-only *mode* that disables these actions entirely is on the roadmap for
v0.10.)

---

## Recipes

### 1. Full monitoring, read-only (recommended)

```sql
CREATE ROLE pg_lens_ro LOGIN PASSWORD 'change-me';
GRANT pg_monitor TO pg_lens_ro;
-- optional, for cancel/terminate of other users' backends:
-- GRANT pg_signal_backend TO pg_lens_ro;
```

### 2. Managed PostgreSQL (RDS / Aurora / Cloud SQL / Azure)

You usually can't be superuser, but the provider lets you grant `pg_monitor`:

```sql
-- RDS/Aurora: run as the master user
CREATE ROLE pg_lens_ro LOGIN PASSWORD 'change-me';
GRANT pg_monitor TO pg_lens_ro;
```

Replication panels may still be absent on read replicas / restricted tiers —
that's the provider's restriction, and pg_lens degrades to an absent panel as
designed. Point pg_lens at the **session or direct endpoint**, not a
transaction-pooling port — see
[Connection poolers](../README.md#connection-poolers-pgbouncer--supavisor--rds-proxy).

### 3. Minimal — activity only, no `pg_monitor`

If your policy forbids `pg_monitor`, the closest single grant is the stats role:

```sql
CREATE ROLE pg_lens_ro LOGIN PASSWORD 'change-me';
GRANT pg_read_all_stats TO pg_lens_ro;   -- full pg_stat_activity, replication, progress views
```

`pg_read_all_stats` alone covers the Micro/Macro/Replication/Vacuum lenses.
Add `pg_read_all_settings` if you want every server setting to resolve, and
the `pg_stat_statements` grant for the Query Lens. This is a strict subset of
`pg_monitor`.

### 4. Per-database `SELECT` grants (no predefined roles at all)

On very locked-down clusters you can skip predefined roles entirely and grant
only what a given lens reads, but you'll lose visibility into other users'
sessions (that's inherent to `pg_stat_activity` without `pg_read_all_stats`).
`pg_lens` will still run — it just shows less. There is no configuration to
change; the panels self-adjust to what the role can see.

---

## Security posture

- **Connect read-only.** With `pg_monitor` (no `pg_signal_backend`), pg_lens
  cannot modify a single row or signal a single backend. This is the intended
  production posture.
- **Password never in the file.** Use `password_cmd` in the
  [services file](../README.md#services-file) to pull the secret from a vault
  or keychain at connect time.
- **Every poll is a short read-only transaction** with its own
  `SET LOCAL statement_timeout`, so pg_lens is safe to point at a busy primary
  and safe behind a connection pooler.
- **Web Lens admin is separately gated** behind `PG_LENS_AUTH_TOKEN` (403
  without it) on top of the database-role check — see
  [Web Lens security notes](../README.md#security-notes).
