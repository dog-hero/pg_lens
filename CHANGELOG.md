# Changelog

All notable changes to pg_lens. Format inspired by
[Keep a Changelog](https://keepachangelog.com); versions follow
[SemVer](https://semver.org). Dates are release dates.

## [0.22.0] — 2026-09-09 — "Dual-Lane Poller Architecture, Fast Incident Path & Sub-Second Precision"

### Added
- **Dual-Lane Poller Architecture (`client_fast` & `client_telemetry`)** — decoupled polling engine separating urgent incident triage from heavy catalog telemetry:
  - **Fast Lane (`client_fast`)**: pipelines the 6 critical incident queries (`activity`, `blocking`, `server_info`, `bgwriter`, `active_locks`, `locks_by_relation`) into a single read-only transaction (`begin_read()`), slashing network round-trips from ~39 RTTs to 1 single RTT per tick and eliminating UI lag over remote or high-latency database connections.
  - **Telemetry Lane (`client_telemetry`)**: offloads slower catalog queries into independent tiered cycles: Tier 2 @ 3s (Databases, Statements, Replication, Functions, Progress), Tier 3 @ 30s (Tables, Bloat, Sequences, SLRU, Conflicts), Tier 4 @ 60s (Indices, Table Stats, Index Stats), and on-demand schema table inspection.
  - **Thread-safe Atomic Synthesis (`Arc<RwLock<SharedTelemetry>>`)**: Fast Lane synthesizes full `DbSnapshot` instances immediately from cached telemetry snapshots, preventing slow catalog queries from blocking live activity refreshes.
  - **Single-Connection CLI Flag & Fallback (`--single-connection`)**: flag forcing single-connection operation for resource-constrained environments (e.g. strict pooler limits), with automatic graceful fallback if the telemetry connection fails to establish.

### Changed
- **Activity Table Column Ordering** — moved `State` column directly after `Xact` in both TUI Micro Lens and Web dashboard, placing transaction lifecycle and session state side-by-side with query duration for faster operational triage.
- **High-Precision Time Formatting** — sub-second durations (`< 1s`, e.g. `0.0001s`) and sub-millisecond query execution times (`< 1ms`, e.g. `0.0500ms`) now render with 4 decimal places across both TUI (`format.rs`) and Web (`format.ts`).

## [0.21.0] — 2026-09-08 — "Web Modernization, 9-Lens Parity & Runtime Cluster Switching"

### Added
- **Runtime Server / Cluster Switching (`services.toml`)** — dynamic server switching in both TUI and Web UI without restarting `pg_lens`:
  - Core domain model `ServerSwitchTarget` and poller loop handling: cleanly cancels in-flight queries via `cancel_query`, closes connection, resets history and caches, updates storage paths, and connects immediately to the new target with zero backoff.
  - TUI Server Picker modal (accessible via `C`, uppercase) displaying configured services from `services.toml` with host, port, user coordinates, and active server marker; automatically updates the `!` (`psql`) shell-out target.
  - Web API endpoints `GET /api/servers` (lists configured servers and active target) and `POST /api/server/switch` (token-gated, allowed in read-only mode).
  - Web UI header server switcher (`#server-target-group`) and Command Palette "Servers" category for keyboard navigation.
  - Strict credential safety: passwords and auth commands are never exposed via APIs or UI.
- **Web Dashboard Modernization & Vertical Space Architecture**:
  - Dedicated Macro Lens (Lens 1) cockpit housing Server Vitals cards, time-scrubber, and 1-hour uPlot history chart.
  - Compact Health Ribbon: collapses cockpit on Lenses 2–9 into a sleek single-line summary (TPS, active conns, cache hit %, locks), freeing 100% viewport height for data tables with sticky headers.
- **Slide-Over Inspector Drawer (`InspectorDrawer`)**:
  - Replaces disruptive inline accordion row expansions with a smooth slide-over inspection drawer on the right.
  - Dedicated drawer views for Sessions (properties, normalized SQL with syntax highlighting and copy), Blocking Chains (visual wait-for tree with root blocker and cycle detection), Schema Tables (columns, constraints, indexes, size breakdown), Replication Slots (lag, safe sizes, LSNs), and Statements.
- **Quick Command Palette (`Cmd+K` / `Ctrl+K`)**:
  - Global search and action runner: jump across all 9 lenses, switch databases, switch servers, toggle pause/recording, export JSON snapshots, and change themes.
- **Full 9-Lens Parity in Web UI**:
  - Complete parity across all 9 lenses: Macro (1), Live Activity (2), Blocks & Locks (3), Replication (4), Schema & Bloat (5), Indexes (6), Queries (7), Progress (8), and Records & Incident Replay (9).
- **Modular CSS Design System**:
  - Decomposed monolithic CSS into clean tokenized modules (`tokens.css`, `layout.css`, `components.css`, `drawer.css`, `palette.css`, `lenses.css`).

## [0.20.1] — 2026-09-08 — "Hotfix: PostgreSQL 16 Replication Slots Compatibility"

### Fixed
- **Replication Slots Version Gate Fix for PostgreSQL 16** — resolved a critical SQL query error (`column s.invalidated does not exist`) when connecting to PostgreSQL 16 clusters:
  - In PostgreSQL 16, `pg_replication_slots` does not include the `invalidated` column (which was introduced in PostgreSQL 17).
  - Moved the `s.invalidated` version gate from `server_version_num >= 160_000` to `server_version_num >= 170_000` (`replication_slots_post_170000.sql`).
  - PostgreSQL 16 now cleanly uses the backwards-compatible query with `NULL::text AS invalidated`, restoring full replication slot polling on PostgreSQL 16.

## [0.20.0] — 2026-09-07 — "Logical & Physical Replication Deep-Dive: Dedicated Publications, Enriched Subscriptions & Slots"

### Added
- **Dedicated Publications Panel (`pg_publication`, `pg_publication_tables`)** — deep catalog visibility into logical replication publications:
  - Core domain model `PublicationRow` and poller query with table aggregation, tracking owner, all-tables scope, and publication operations (`INSERT`, `UPDATE`, `DELETE`, `TRUNCATE`).
  - TUI Replication Lens dedicated panel rendering publication name, owner, all-tables scope, operation badges, and published table list with truncation indicator.
  - Web Lens dedicated Publications section displaying publication cards with operation tags and published table lists.
- **Enriched Subscriptions Telemetry (`pg_subscription`, `pg_stat_subscription`)** — comprehensive subscriber monitoring:
  - Core domain model `SubscriptionRow` and version-gated query (`subscriptions_post_150000.sql` for PG 15+) surfacing sanitized connection parameters (host, port, dbname), synchronous commit mode, streaming replication mode, binary transfer mode, two-phase commit support, worker count, and active table synchronization names (`syncing_table_names`).
  - TUI Replication Lens panel displaying subscription state, remote publication, worker counts, sync settings, and currently synchronizing tables.
  - Web Lens Subscriptions section displaying dedicated subscription cards with health badges and advanced replication properties.
- **Enriched Replication Slots Diagnostics & Interactive Detail Modal** — deep diagnostics for physical and logical replication slots (`pg_replication_slots`):
  - Poller query expanded with version gate (`replication_slots_post_160000.sql` for PG 16+) capturing plugin, database, temporary slot flag, active PID, client application name, client address, restart LSN, confirmed flush LSN, consumer lag in bytes, two-phase commit, and conflicting / invalidated flags.
  - Responsive multi-tier slot table in TUI Replication Lens (`>= 135` wide, `>= 100` medium, `< 100` narrow) ensuring slot names and core metrics remain legible on any terminal width.
  - Interactive Replication Slot Details dialog on `Enter` in TUI Replication Lens, displaying complete connection, LSN positions, WAL retention, and configuration details for the selected slot.
  - Web Lens replication slots table displaying plugin, database, client application, consumer lag, and failure status indicators.

### Changed
- **Replication Lens Layout & Organization** — reorganized Replication Lens in both TUI and Web into clean, uncluttered sections separating Publications, Subscriptions, WAL Senders, and Replication Slots.

## [0.19.0] — 2026-09-07 — "Observability Expansion: Sequences, SLRU, Standby Conflicts & Table Storage"

### Added
- **Sequence Exhaustion Alerts (`pg_sequences`)** — proactive monitoring of PostgreSQL sequences to prevent catastrophic integer overflow / sequence exhaustion incidents:
  - Core domain model `SequenceRow`, `SequencesSnapshot`, `calculate_sequence_exhaustion`, and multi-tier severity (`bad` > 90%, `warn` > 75%, `calm`).
  - TUI Schema Lens sub-view (accessible via `S`, with `T` returning to Tables view) displaying sequence name, schema, data type, current value, maximum value, percentage consumed, and remaining headroom with `/` search filtering.
  - Web Lens dedicated Sequences panel with search filtering, remaining capacity formatting, and visual exhaustion warning badges.
- **SLRU Cache Monitoring (`pg_stat_slru`, PG 13+)** — deep visibility into Simple LRU buffer subsystems (subtrans, multixact, clog, async, commit_ts):
  - Tracks cumulative and per-second delta rates for page reads and writes, overall hit ratio percentage, and actively flags subtransaction cache thrashing (`subtrans_warning`).
  - TUI Macro Lens SLRU panel displaying hit ratios and activity across active subsystems.
  - Web Lens vitals card reporting SLRU efficiency and warning when subtransactions thrash the SLRU cache.
- **Standby Recovery Conflicts (`pg_stat_database_conflicts`)** — telemetry for query cancellations and delays on streaming standby replicas:
  - Tracks individual conflict classes (`tablespace`, `lock`, `snapshot`, `bufferpin`, `deadlock`) and aggregate conflict rate per second.
  - TUI Replication Lens panel with real-time recovery conflict rate and category breakdown.
  - Web Lens Replication Lens row alerting on active standby conflicts.
- **Cross-Lens Navigation: Table to Index Jump (`i`)** — instant jump from Schema Lens directly into Index Lens filtered by the selected table (`i`), with `Backspace` or `Esc` returning to the originating table.
- **Granular Table Storage & Cache Breakdown** — table statistics expanded to distinguish between main heap storage and TOAST storage (`heap_bytes` vs `toast_bytes`), along with separate buffer cache hit ratios for heap, index, and toast blocks (`heap_cache_hit_pct`, `idx_cache_hit_pct`, `toast_cache_hit_pct`) across TUI detail and Web Lens detail inspectors.

### Changed
- **Replication Lens Renaming** — renamed Tab 4 from "4 Replication" to "4 Replication Lens" in TUI and "Replication Lens" in Web Lens navigation, aligning naming across all nine lenses.

## [0.18.1] — 2026-09-07 — "Records Lens & Streaming Compression"

### Added
- **Records Lens (TUI Tab 9 & Web Tab 8)** — dedicated management view for browsing, searching, replaying, copying, and deleting incident recordings:
  - TUI Tab 9 (`9 Records Lens`, direct jump `9`): table view with status indicators, creation timestamps, file sizes, compression badges (`.gz`), search filter (`/` and `\`), interactive replay launch (`Enter`), clipboard path copy (`y`), and interactive deletion (`x`) with safety confirmation modals.
  - Web Tab 8 (`Records`, shortcut `8`): responsive dashboard panel with live search filtering, file metadata, direct download endpoint (`GET /api/records/download/{filename}`), and token-gated deletion (`DELETE /api/records/{filename}`) strictly blocked under `--read-only`.
- **Real-Time Streaming Compression (`.jsonl.gz`)** — pure-Rust streaming gzip compression powered by `flate2` (`RecordSink::Gz`), reducing recording disk footprint by up to 90% with near-zero runtime CPU overhead; enabled via `--record-compress` CLI flag and `record_compress = true` config setting.
- **Transparent Format Detection** — `RecordingReader::load` auto-detects gzip vs plain JSONL files via magic header bytes (`0x1f 0x8b`) or file extension, allowing seamless replay of both `.jsonl` and `.jsonl.gz` recordings.
- **Recording Auto-Split & Retention Policies** — automatic file rotation when a recording exceeds `--record-max-mb` (default 100 MB), configurable retention window (`--record-retention-days`, default 30 days), and maximum total recording storage cap (`--record-max-total-mb`, default 1000 MB) with FIFO auto-pruning that protects currently active recording files.
- **Visual Replay Scrubber & Controls** — visual playback timeline track (`[████░░░] 42%`) in TUI replay mode displaying current frame, total frames, frame timestamp, speed multiplier, loop indicator, jump to start (`Home`/`g`), jump to end (`End`/`G`), and loop toggle (`l`/`L`).

### Changed
- **TUI & Web Lens Tab Navigation** — TUI now features 9 lenses (`1`..=`9` direct jump, with `9` Records), and Web Lens features 8 tabs (`1`..=`8` direct jump, with `8` Records). Keyboard help overlay (`?`) updated.
- **Demo & Documentation** — regenerated showcase recording and updated landing page highlighting the 9 lenses, streaming compression, and recording retention.

## [0.18.0] — 2026-09-07 — "Incident Recording & Flight Recorder"

### Added
- **Incident Flight Recorder (`Shift+R` / `Ctrl+R`)** — background recording mode capturing structured JSONL snapshot frames directly into `~/.local/state/pg_lens/recordings/` with a live header indicator (`● REC`) and frame counters.
- **Snapshot Bookmark Export (`E`)** — exports the current tick's snapshot to a single-frame `.jsonl` file with clipboard path copy and visual toast feedback.
- **Offline Incident Replay (`pg_lens replay <file>`)** — deterministic interactive replay of recorded incident `.jsonl` files across all eight lenses with play/pause (`Space`), frame scrubbing (`←`/`→`), playback speed adjustment (`[`/`]`), and auto-looping on reaching the final frame.

### Changed
- **VHS Demo Recording (`docs/demo.gif`)** — updated showcase recording to demonstrate continuous incident flight recording (`Shift+R`), snapshot bookmark export (`E`), and offline replay across all eight lenses.
- **Landing Page & Documentation (`site/index.html`)** — added Incident Replay card to "Run it" section and continuous recording highlights to the incident checklist.

## [0.17.3] — 2026-09-07 — "FSL-1.1-MIT & Governance"

### Added
- **Contributor License Agreement (`CLA.md`)** — formal Contributor License Agreement based on the standard Apache-style CLA, establishing contribution terms and IP clarity for future commercial and open-source releases.
- **Third-Party License Audit (`pg_lens licenses`)** — CLI subcommand and script generator (`scripts/generate_licenses.py`) generating complete open-source acknowledgements and third-party notices (`THIRD_PARTY_LICENSES.md`).
- **CI Automated License Enforcement** — integrated `cargo-deny check licenses` into `.github/workflows/ci.yml` and `deny.toml` ensuring strict compliance with permissive and fair-source policies.
- **Engineering Multi-Agent Architecture Guide (`AGENTS.md`)** — repository-level specification documenting the 4 specialized agent roles (`release-manager`, `lens-builder`, `qa-tester`, `feature-discovery`), hard architectural invariants, and mandatory quality gates.
- **Direct Changelog Navigation** — integrated direct links to rendered release notes (`docs/changelog.html`) across site navigation, hero badge, and documentation footer.

### Changed
- **License transition to FSL-1.1-MIT** — adopted Functional Source License, Version 1.1 with MIT Future License (`FSL-1.1-MIT`), offering complete freedom for internal and educational use while protecting core commercial exclusivity, automatically converting to full MIT after two years.

## [0.17.2] — 2026-09-06 — "Column Colors & Duration Severity"

### Changed
- **Column-specific color system (`pg_activity`-style)** — replaced whole-row tinting in the Micro Lens activity table with dedicated semantic colors per column in both TUI and Web Lens:
  - `S` (Status): `B` in bold red (blocked sessions), `W` in bold yellow (waiting).
  - `PID`: Cyan, or bold red if the session is blocked.
  - `DB`: Cyan.
  - `User`: Clean readable white/default text.
  - `Client`: Calm dark gray (plus SSL `🔒` badge if encrypted).
  - `State`: `active` (green), `idle` (dark gray), `idle in transaction` (yellow), `idle in transaction (aborted)` (bold red).
  - `Wait`: `Lock:*` events highlighted in bold red, `IO:*` and other wait events in yellow, none in dark gray.
  - `Query`: Clean, readable neutral text in the table row, keeping SQL syntax highlighting reserved for the expanded detail panel (`Enter`).
- **Duration-only time coloring** — duration-based severity colors (red bold past 30s, yellow bold past 10s, calm green at or below 10s) are now applied **strictly to the `Duration` column** for active queries, eliminating whole-row background flooding and text color overrides. Idle and non-active sessions remain calm dark gray regardless of age (transaction-age risk continues to be isolated in the `Xact` column).

## [0.17.1] — 2026-09-06 — "Progress Lens"

### Added
- **Progress Lens (`8 Progress Lens`)** — dedicated lens for real-time monitoring of all in-flight PostgreSQL maintenance and DDL operations (`pg_stat_progress_create_index`, `pg_stat_progress_vacuum`, `pg_stat_progress_cluster`, and `pg_stat_progress_analyze`):
  - TUI Tab 8 (`8 Progress Lens`, direct jump `8`): unified table displaying PID, command, target relation, phase, ASCII progress gauge (`[=====>    ] 50%`), step counters (`current / total`), and operational detail.
  - Interactive selection (`j`/`k`, `g`/`G`), detail popup on `Enter`, search filter (`/` and `\`), and direct query cancel (`c`) or backend terminate (`K`) with confirmation modals.
  - Web Lens Tab 7 (`tab-progress`, `#progress-panel`, shortcut `7`): dedicated Progress panel with live progress bars, search filtering (`/`), and clean empty/collected states.
  - Unified data model in `pg_lens_core` (`DbSnapshot::unified_progress` and `ProgressUnifiedRow`) unifying DDL progress and autovacuum progress sorted deterministically by PID.

### Changed
- **Tab consistency & naming** — Tab 3 is titled `"3 Blocks & Locks Lens"` across TUI and Web Lens for naming consistency.
- **Tab count & navigation** — TUI now features 8 tabs (`1`..=`8` direct jump), and Web Lens features 7 section tabs (`1`..=`7` direct jump). Keyboard help overlay (`?`) and documentation updated.

## [0.17.0] — 2026-09-06 — "Blocks & Locks Lens"

### Added
- **Blocks & Locks Lens (`3 Blocks & Locks Lens`)** — dedicated lens positioned between Micro
  Lens and Replication, featuring a dual-pane split view:
  - Upper pane: hierarchical blocking wait-tree (`root blocker -> waiting PID -> waiting PID`)
    identifying root blockers, blocked session count, lock modes, target relations, and wait age.
  - Lower pane: complete active locks table (`pg_locks`) showing all granted and waiting
    locks with target relation, lock type, lock mode, status (`GRANT`/`WAIT`), age, user, and query text.
  - Quick pane-switching with `p` / `o`, interactive detail panel on `Enter`, and direct
    query cancellation (`c`) or backend termination (`K`) with confirmation modals from either pane.
  - Direct jump via key `3` or mnemonic `b`. Web Lens mirrors with an interactive
    blocking tree, active locks table, and search filter on tab `2 Blocks & Locks`.
- **In-flight DDL & maintenance progress (`pg_stat_progress_*`)** — monitors live progress
  from `pg_stat_progress_create_index`, `pg_stat_progress_analyze`, and
  `pg_stat_progress_basebackup`. Surfaced directly in the Micro Lens session detail panel (`Enter`)
  and in the Web Lens Maintenance panel alongside autovacuum operations.
- **SSL / TLS connection security indicator (`pg_stat_ssl`)** — integrates `pg_stat_ssl`
  into activity and idle session tracking:
  - Visual lock badge (`🔒 `) on client address columns in the activity table.
  - Connection security status in the session detail overlay: shows cipher and protocol version
    (e.g., `TLSv1.3 (TLS_AES_256_GCM_SHA384)`) or flags unencrypted connections (`Plaintext`).
  - SSL status badge on idle connection census in both TUI and Web Lens.

### Changed
- **Tab ordering & numbering** — Blocks Lens is positioned as Tab 3 (`3 Blocks & Locks Lens`), shifting
  Replication to 4, Schema Lens to 5, Indexes to 6, and Query Lens to 7 (`1`-`7` direct jumps).
  Web Lens side navigation updated to Activity (1), Blocks & Locks (2), Replication (3), Schema (4),
  Indexes (5), and Queries (6).

## [0.16.0] — 2026-08-06 — "First impression"

### Added
- **`curl | sh` install script** — `scripts/install.sh`: detects your
  platform (macOS arm64/x86_64, Linux musl x86_64/aarch64), resolves the
  latest release via the GitHub API (or pins one with `PG_LENS_VERSION` /
  `--version`), **verifies the download against the `.sha256` sidecar the
  release already publishes**, and installs into `~/.local/bin` with no
  sudo and no rc-file edits (it prints the `export PATH=...` line for you).
  Re-running it is the upgrade path. `--dry-run` previews without
  downloading; unpublished platforms fail loud with a clear message. Also
  mirrored at `https://dog-hero.github.io/pg_lens/install.sh`.
- **Micro Lens row colors, pg_activity-style** — the whole activity row is
  now tinted by session state (active green, idle dim, idle-in-transaction
  yellow, aborted red, unknown neutral). For `active` rows only, a
  long-running query overrides that base color (orange past 10s, red past
  30s) so an old idle session is never mislabelled as a problem. A row
  whose backend reports a live `wait_event` is tinted yellow immediately,
  independent of the duration thresholds — this extends the pre-existing
  `W` marker's meaning to the whole row and is deliberate, not a
  regression. **Blocked** outranks every other tint. The `B`/`W` textual
  markers are unchanged, so the signal never depends on color alone, and
  selection highlighting stays unmistakable. TUI + Web.
- **Copy to clipboard (`y`)** — copies the current selection's full text:
  the Micro Lens's selected query, the Query Lens's selected statement, the
  Index Lens's verbatim `CREATE INDEX` definition, or the Schema Lens's
  selected table's qualified name (its column list once the structure
  detail is open). Uses the **OSC 52** terminal escape sequence directly —
  no clipboard dependency, and it works over SSH — capped at 100 KB, with
  an honest toast ("sent to clipboard (OSC 52)") since pg_lens can't
  confirm the terminal acted on it; supported in iTerm2/kitty/WezTerm/
  Ghostty and tmux with `set-clipboard on`, not in Apple Terminal. Web Lens
  gets copy buttons on the query/statement details instead.
- **I/O profile — `pg_stat_io`, PG 16+** — reads/writes/hits aggregated by
  `backend_type` × `context` (all-zero rows filtered), per-second rates
  from tick-to-tick deltas with stats-reset handling, average read/write
  latency shown as `--` when `track_io_timing` is off. Slow cadence; lives
  in the Macro Lens's checkpointer/buffer column (no new tab) and as a Web
  Lens vitals card. Cleanly absent, not broken, on PG 13–15.
- **WAL generation rate — `pg_stat_wal`, PG 14+** — WAL bytes/s and
  records/s from tick-to-tick deltas (reset-safe), with `wal_buffers_full`
  tinted only while actively climbing (a tuning nudge, not an incident);
  timing shows `--` when `track_wal_io_timing` is off. Fast tick — a
  Replication Lens panel plus a Macro Lens line. Absent on PG 13.
- **Shell completions** — `pg_lens completions <bash|zsh|fish|powershell|
  elvish>` via `clap_complete`, resolved before any DB/terminal work so it
  never touches the network or a connection.

### Changed
- **SQL keyword highlighting** moved out of the Micro/Query Lens table rows
  (which now render plain query text in the row's state/duration color)
  and lives only in the `Enter` detail panel / expanded row, in both
  lenses.

## [0.15.0] — 2026-08-05 — "Schema Lens completo"

### Added
- **Table structure detail (on-demand)** — pressing `Enter` on a Schema Lens
  table now also shows the table's STRUCTURE, the kind of information
  psql's `\d` covers but rendered in pg_lens's own detail-panel style:
  columns (name/type/nullable/default, including `generated always as
  identity` and generated-stored labels), constraints (PK/FK/UNIQUE/CHECK
  via `pg_get_constraintdef`), tables that reference this one via incoming
  foreign keys, and full index definitions. Fetched only when the detail
  opens (a new request/response channel, zero cost when no detail is open,
  never on the poll cadence), with a scrollable overlay (`j`/`k` inside the
  detail), a "loading structure…" state, and a graceful "unavailable"
  message on error. Web: `POST /api/schema/detail` (token-gated) plus a
  structure section in the expanded row. TUI + Web.
- **Partition collapsing + drill-down** — native partitioned tables now
  collapse to a single parent row by default, with aggregated stats (summed
  size/rows/dead tuples via `pg_partition_tree`) and a `[parts: N]` marker;
  `p` toggles visibility of the individual leaf partitions (dimmed `↳`
  prefix). `Enter` on a parent shows its partition list plus its structure,
  and the `Δ1h` growth ring now tracks the parent aggregate instead of
  scattering across leaves. TUI + Web (a "show partitions" checkbox).
- **Cross-lens jump to Query Lens (`x`)** — pressing `x` on a selected
  Schema Lens table jumps straight to the Query Lens, pre-filtered
  (substring match on the table name) to statements that mention it;
  `Backspace` returns, and the seeded filter is visible and clearable with
  `\` like any other lens filter. TUI + Web.
- **Per-table lock indicator** — tables with locks held against them now
  show a dim `L:N` marker in the Schema Lens Tables view, turning red
  (`L:N!`) when something is WAITING on that table — a new fast-tick,
  best-effort per-relation lock collection (reuses `pg_locks`, refreshes
  every poll rather than the slow 60s schema cadence, so it never masks a
  developing lock pile-up). TUI + Web badge.

### Fixed
- **Honest Schema Lens table counts + configurable row cap** — the
  `table_stats` query had a silent `LIMIT 200` (ranked by size): table
  201+ never appeared, and the footer reported the truncated list length
  as if it were the database's true table count. Fixed with a cheap
  `count(*)` alongside the row query so the footer now reads `"N of M
  tables — raise schema_table_limit or filter"` only when actually
  truncated, plain `"N tables"` otherwise; the cap is now configurable
  (`--schema-table-limit` / `PG_LENS_SCHEMA_TABLE_LIMIT` / `schema_table_limit`
  in config.toml, clamped 10–10000, default 200 unchanged); and the query
  now ranks candidates by the cheap catalog column `pg_class.relpages`
  first, so the expensive exact-size computation only runs for the rows
  actually shown instead of every table in the cluster. TUI + Web.
- **PG16 partition double-count** — PG16 emits an all-zero
  `pg_stat_user_tables` row for a partitioned table's parent relation,
  which was being summed into the new partition-aggregate stats on top of
  the leaves' real numbers; parent rows (`relkind = 'p'`) are now excluded
  from that aggregation.

## [0.14.0] — 2026-07-17 — "See the trend, not just the moment"

### Added
- **Vitals trend arrows** — the persisted 1h history now also carries
  `connections_total`, `cache_hit_pct`, `lock_pressure_pct`, and
  `oldest_xid_age` per tick (all `#[serde(default)]`, so pre-v0.14 JSONL
  files keep loading intact with these defaulted to unknown, nothing
  dropped). The Macro Lens vitals cards (Connections / Cache hit / Lock
  table) now show a `↑`/`↓`/`→` trend arrow comparing "now" against the
  sample from ~5 minutes ago (5% deadband so noise doesn't flicker the
  arrow), tinted yellow only when the direction is the concerning one
  (rising connections/lock pressure, falling cache hit). Web mirrors the
  arrows with a tooltip spelling out the delta (e.g. "+12.0% vs 5 min
  ago"). Foundational for future trend features. TUI + Web.
- **History time-scrubber (Web Lens)** — hover over the TPS/sessions chart
  for a live readout of that moment's vitals (TPS, sessions, connections,
  cache hit%, lock pressure%, oldest-XID age); click pins the moment (a
  dashed marker appears on the chart), and the pin survives the live SSE
  stream because it's keyed by timestamp, not index. `✕` / `Esc` / clicking
  again unpins; `←`/`→` step the pinned moment one sample at a time for a
  tick-by-tick incident walkthrough; trend arrows dim while pinned to avoid
  implying they describe the pinned moment. A pinned moment that ages out
  of the 1h ring unpins itself with a toast instead of showing stale data.
  Web-only.
- **Table size growth (Schema Lens)** — a new `Δ1h` column: signed size
  change over the last hour per table (`+120 MB`, `-3 MB`, `—` when no
  reading yet), tinted yellow/red past 10%/25% growth on tables ≥10 MiB.
  Backed by a bounded, in-memory, oid-keyed per-table ring (survives
  renames, resets cleanly on drop+recreate; capped at 200 tables ≈ 288 KB,
  never unbounded) fed only by the slow schema cadence — restarts refill
  over the next hour rather than persisting to disk. New "growth" mode in
  the `s` sort cycle (largest absolute Δ first). TUI + Web.
- **Query I/O & temp-spill profile (Query Lens)** — a new `Temp` column
  (temp bytes written, tinted yellow past 100 MiB — the #1 query-tuning
  signal the lens was missing) plus a full I/O breakdown in the `Enter`
  detail panel: temp read/written, shared blocks dirtied/written, block
  read/write time (`--` when `track_io_timing` is off), and WAL bytes
  (`--` on `pg_stat_statements` < 1.9). Three extension-version-gated SQL
  variants (1.8 base, ≥1.9 adds `wal_bytes`, ≥1.11/PG17 renamed timing
  columns). New "temp" mode in the `s` sort cycle. TUI + Web.
- **Interactive service picker for `serve`** — `pg_lens serve` started on a
  real TTY with an ambiguous `services.toml` (multiple services, none
  selected) now prompts with a numbered list (name + host/user, never
  secrets) instead of only failing loud; accepts an index or a name,
  re-prompts on invalid input, and auto-selects with a notice when exactly
  one service is defined. Non-TTY (piped/systemd/CI) keeps the v0.13
  fail-loud list-and-exit so an unattended process never hangs on stdin.

### Fixed
- Regenerated the README demo gif and web dashboard screenshot for the
  v0.13 redesign and fixed stale `v0.7.1`-pinned download-URL examples
  (repo-side, `e61a6b3`).
- Fixed the `e2e_pty_live.py` reconnect-recovery check, which had not been
  updated for the six-lens layout (repo-side, `030471f`).

## [0.13.0] — 2026-07-17

### Navigation & filters
- **Direct tab jump (`1`–`6`)** — jump straight to any lens by number; the
  tab bar now shows the digit prefix for each (`1 Macro │ 2 Micro │ …`) so
  the binding is self-documenting.
- **`Shift+Tab` backward cycle** — `Tab::prev()` was previously unwired;
  `BackTab` now cycles lenses in reverse.
- **`Backspace` last-tab toggle** — browser-back-style jump to the
  previously active lens (still deletes as expected inside a filter
  editor).
- **Fast scroll everywhere** — `Home`/`g` and `End`/`G` jump to the first/
  last row, `PageUp`/`PageDown` move by a page, on every selectable table
  in the TUI.
- **Schema and Query Lens filters (`/`)** — the textual filter, previously
  Micro-Lens-only, now also filters the Schema Lens (Tables view, by
  schema/table name) and the Query Lens (by query text), each with its own
  independent filter state; `\` clears whichever lens's filter is active.
  Web parity: `#schema-filter` / `#statements-filter` search inputs.
- All of the above added to the `?` help overlay.

### Web catch-up & redesign
- **Modern dashboard redesign** — new layout: a left sidenav (collapsing to
  an icon rail ≤1024px, a horizontal strip ≤720px) and a redesigned topbar
  (brand, current database + switcher, read-only badge, pause, connection
  state, theme toggle). Inline SVG icon sprite, no icon font or UI
  framework added. System-UI font for chrome, monospace for data. The
  Checkpoints card now leads with a `X.XX/min` headline and pushes detail
  numbers below it. Bundle grew 116→132 KB (still embedded, no new
  runtime dependency).
- **Light/dark theme toggle** — a header toggle persisted in
  `localStorage`; defaults to dark (unchanged behavior for existing users).
- **Web keyboard navigation** — `1`–`5` jump to the nav sections, `/`
  focuses the active panel's filter input, `Esc` blurs it; shortcuts are
  suppressed while typing in a text input (except `Esc`).
- **Web database switcher** — a header dropdown lists every database the
  connected role can see and switches the poller to it via the new
  `POST /api/db/switch {"database": "name"}` endpoint (token-gated the same
  way as `/api/schema/refresh` — a database switch is a read-only
  reconnect, so it works even under `--read-only`; the target is validated
  against the snapshot's `databases` list, `400` on an unknown name). The
  poller's existing db-switch channel — previously dropped in `run_serve` —
  is now threaded into `WebState`. Degrades to just showing the current
  database name when `databases` is null or has fewer than two entries.
- **`serve` fail-loud on ambiguous services** — `pg_lens serve` with a
  `services.toml` defining one or more services and no `--service`/`--dsn`/
  env var previously fell through and silently connected to `localhost`
  (the TUI shows an interactive picker in this situation, but `serve` has
  no TTY to show one to). It now lists the available service names
  (host/user, never secrets) to stderr and exits non-zero instead.

### Fixed
- **`--dsn` / `--service` conflict across the `serve` subcommand
  boundary** — both flags are declared `global`, which let clap's
  `conflicts_with` be silently bypassed when one flag was given before
  `serve` and the other after; pg_lens would connect to `--dsn` and
  quietly ignore `--service`, a wrong-server footgun. Fixed with a runtime
  backstop, `ConnArgs::ensure_conn_flags_consistent()`, called right after
  argument parsing regardless of flag position.

## [0.11.0] — 2026-07-17 — "Incident precursors & connection visibility"

### Added
- **Idle connection / connection-age census** — the Micro Lens `I` key toggles
  the activity table to a dedicated idle-session view (`state = 'idle'`,
  oldest-first, capped at 100): PID, age, user, database, application, and
  client address, with a headline ("N idle connections, oldest …") and
  yellow/red age tiers (30 min / 4 h). Solves the classic pool-exhaustion
  incident — many connections used but few active — where the existing
  activity table filters idle sessions out entirely. `Esc` closes it;
  `/`/`w`/`c`/`K`/`s` are inert while it's open. Best-effort, fast tick.
  TUI + Web.
- **Lock-table pressure gauge** — a third gauge in the Macro Lens vitals
  strip: held locks vs. capacity (`max_locks_per_transaction ×
  (max_connections + max_prepared_transactions)`), yellow at 60%, red at
  85%, warning before "out of shared memory, you might need to increase
  max_locks_per_transaction". Best-effort, fast tick. TUI + Web.
- **Invalid / not-ready index flag** — the Index Lens now flags indexes left
  behind by a failed `CREATE INDEX CONCURRENTLY` (`pg_index.indisvalid` /
  `indisready`) as a new `INVALID` finding, ranked ahead of `UNUSED`, with
  actionable detail text (drop and rebuild). TUI + Web.
- **Open a `psql` shell from pg_lens** (`!`, TUI-only) — suspends the TUI and
  spawns `psql` on the exact connection pg_lens is polling (host/port/user/
  dbname), restoring the terminal on exit, spawn failure, or `psql` missing
  from `PATH`. The password is never passed on the command line — it's
  resolved as late as possible and handed to the child only via a
  `PGPASSWORD` environment variable. Under `--read-only`, the shell launches
  with `PGOPTIONS=-c default_transaction_read_only=on` and prints a notice
  that this is a default, not a hard sandbox — a full `psql` session can
  still override it explicitly. Disabled (with a clear message) in `--mock`,
  since there's no real connection to hand psql.

## [0.10.0] — 2026-07-16

### Added
- **Read-only mode** — a `--read-only` flag / `PG_LENS_READ_ONLY` env var /
  `read_only = true` in `config.toml` (precedence: flag → env → config →
  default `false`) hard-disables every admin/mutating action for
  shared or audited deployments. This is a real server-side gate, not UI
  hiding: in the TUI, `open_confirm()` refuses `c`/`K` *before* the confirm
  modal ever opens (inline "read-only mode — action disabled" feedback,
  plus a permanent yellow `RO` marker in the header so the mode is never
  silently active); in the Web Lens, the `/api/admin/*` endpoints return
  `403` even when a valid `PG_LENS_AUTH_TOKEN` is presented. A new
  `GET /api/config` endpoint exposes `{"read_only": bool}` and the web
  frontend disables the cancel/terminate buttons and shows a badge to
  match. Schema refresh (`R`) is unaffected — it only ever opens a
  read-only transaction. `pg_lens serve` inherits the same flag/env/config
  resolution. TUI + Web.
- **Remote connection config** — `--config-url <URL>` / `PG_LENS_CONFIG_URL`
  env / `remote_config` in `config.toml` loads a shared `services.toml`
  from a remote source, so a team can point every machine at one curated
  target list instead of copying the file by hand. Accepts either a
  `github:OWNER/REPO/PATH[@REF]` shorthand (fetched via the GitHub
  Contents API) or a verbatim `https://`/`http://` URL. The token is
  never stored in a file: it comes from `PG_LENS_CONFIG_TOKEN`, then
  `GITHUB_TOKEN`, then a `remote_config_token_cmd` in `config.toml`
  (mirrors the existing `password_cmd` pattern — an external command,
  trimmed stdout), sent as `Authorization: Bearer`; a token is refused
  outright over plain `http://`. A successful fetch is cached at
  `$XDG_CACHE_HOME/pg_lens/remote-services.toml` (mode `0600`); a failed
  fetch falls back to that cache, then the local services file, with a
  stderr warning at each step — startup never blocks on a flaky network
  (10s timeout) and never hard-fails while a local or cached file can
  still serve. Remote entries win on a same-named collision with local
  entries; the fetch is strictly read-only, pg_lens never writes back to
  the remote source.

## [0.9.0] — 2026-07-16 — "Problem transactions"

The cheap, cohesive batch around long/idle transactions and blocking —
every item reuses data already polled from `pg_stat_activity`, plus a
keyboard help overlay that also clears the stale-README debt.

### Added
- **Idle-in-transaction / transaction-age hunter** — a new transaction-age
  column and marker in the Micro Lens, plus a headline for the oldest
  `idle in transaction` / long-running transaction (the session driving
  XID-wraparound risk and lock retention). Yellow/red age tiers (idle-in-tx
  escalates earlier than a plain long-running transaction). TUI + Web.
- **Blocking chain / lock-wait graph** — the Micro Lens detail panel for a
  selected PID now renders the full wait-for chain (`A→B→C`) to the root
  blocker, with the root highlighted and a warning if a deadlock cycle is
  detected. Reuses the `blocked_by` data already polled. TUI + Web.
- **Prepared-transaction (orphaned 2PC) watch** — orphaned two-phase
  commits (`pg_prepared_xacts`) hold locks and block vacuum forever; they
  now show up (gid, age, owner, database) inside the Vacuum sub-view, with
  yellow/red age tiers and a best-effort absent panel when none exist or
  the view isn't visible. TUI + Web.
- **Keyboard help overlay (`?`)** — a static reference listing every
  binding, grouped by navigation / sub-views / data & refresh / admin /
  quit; doubles as the source of truth the README keybindings table is
  reconciled against.
- **Docs: connection user & least-privilege** — a new
  [`docs/connection-user.md`](docs/connection-user.md) page (linked from
  the README) covering how to create the monitoring role and the exact
  grants each lens needs, what degrades to an absent panel without them,
  and the read-only posture.

## [0.8.0] — 2026-07-16 — "Room to breathe"

Give the v0.7 data room to breathe: dedicated tabs, scrollable lists,
per-database navigation.

### Added
- **Index Lens** — the index advisor promoted to its own tab (full-height
  table, `Enter` detail with indexdef + duplicate partner); the `i` toggle
  inside Schema Lens is gone.
- **Replication Lens** — dedicated tab with every WAL sender/receiver and
  **all** replication slots as a scrollable table (worst severity first);
  the Macro Lens panel stays as the compact summary with a
  "Tab → Replication for all" hint when it clips.
- Web: top-level tabs now mirror the TUI (Activity │ Replication │ Schema │
  Indexes │ Queries).
- **Database selector** — `d` opens a picker of the cluster's databases
  (name + size, current marked); selecting reconnects the poller to the
  chosen database with per-database state (schema/queries/history) reset.
  The header now names the current database. Web switching deferred (the
  web follows the poller's database).
- **Full waits panel** — `w` in the Micro Lens opens the complete ranked
  wait list (count, % of waiting sessions, proportional bar), beyond the
  one-line strip. Web: a collapsible waits list under the activity table.
- **Vacuum sub-view** — `v` in the Schema Lens switches to a full-height
  vacuum view: cluster wraparound headline, all worst tables by XID age
  (scrollable, with dead-tuple ratio and last-vacuum age), and live
  `pg_stat_progress_vacuum`. Web renders the full worst-tables list.

## [0.7.1] — 2026-07-16

### Fixed
- `pg_lens serve` no longer hangs on Ctrl+C while SSE clients (browser
  tabs) are attached — the poller now shuts down inside axum's
  graceful-shutdown window, closing the streams that shutdown was waiting on.
- Replication slots are no longer pushed out of the Macro Lens panel by
  many active WAL senders: senders cap at 4 with "… +N more", slots keep
  their own section ranked worst-first.
- Double-`Esc` quit barrier: a stray/hammered `Esc` closing overlays never
  exits the app anymore — a hint appears and only a second `Esc` within ~2s
  quits (`q`/Ctrl+C stay immediate).

## [0.7.0] — 2026-07-16 — "What should I go fix"

### Added
- **Top waits strip** (Micro Lens): live ranked aggregation of
  `wait_event_type:wait_event` across all sessions — Lock:* red, IO:* yellow.
- **Vacuum health & XID wraparound**: cluster `age(datfrozenxid)` headline
  (yellow >200M, red >500M xids) with a Macro Lens banner past thresholds,
  worst tables by age + dead-tuple ratio, and live
  `pg_stat_progress_vacuum` progress.
- **Replication slots view**: `pg_replication_slots` (both roles) with
  retained WAL, `wal_status`, `safe_wal_size`; inactive-retaining slots
  yellow, `unreserved`/`lost` red — the classic full-disk incident, visible.
- **Index advisor**: unused (constraint indexes never flagged),
  exact-duplicate and prefix-redundant index detection with sizes, scans
  and stats-reset age — signal, not verdict.
- **Checkpointer / bgwriter panel**: checkpoints/min (timed vs requested),
  buffers/s by source, avg write/sync time; checkpoint-pressure warning
  when requested > timed. Version-gated across the PG 17 catalog split.

All five in both the TUI and the Web Lens.

## [0.6.1] — 2026-07-15

### Fixed
- Quitting pg_lens now cancels its in-flight query server-side
  (CancelRequest) — a heavy on-demand bloat estimate no longer keeps
  running after exit until `statement_timeout`.

## [0.6.0] — 2026-07-15

### Added
- **Activity filter**: `/` in the Micro Lens filters live by pid, db, user,
  application, client, state, wait or query text (`Enter` applies, `Esc`
  reverts); a search box mirrors it in the web.
- **Persistent history**: the TPS/sessions chart survives restarts (JSONL
  per connection target under the XDG state dir); ring capacity raised to
  1 hour at the default 2s tick.
- **config.toml**: persistent defaults for `interval`, `schema_interval`
  and `listen` (`~/.config/pg_lens/config.toml`; precedence
  flag → env → config → default).
- **Web parity**: pause button, schema/bloat refresh button, and
  cancel/terminate actions in the web — admin strictly requires
  `PG_LENS_AUTH_TOKEN` (403 otherwise).
- Empty-state messages for the activity table ("no sessions match" vs
  "no active sessions").

## [0.5.3] — 2026-07-15

### Changed
- Every poll now runs inside a per-tick **read-only transaction** with
  `SET LOCAL statement_timeout` — pooler-safe (prepare + execute on one
  backend), a consistent MVCC snapshot per tick, and a hard safety ceiling
  for every query. The poller session identifies itself
  (`application_name = 'pg_lens'`).

### Fixed
- Connection flags (`--service`, `--dsn`, …) are now global: `pg_lens
  --service X serve` no longer silently ignores the service.

### Known limitation
- PgBouncer *transaction* pooling remains unsupported (named prepared
  statements leak across backends); use session pooling or a direct
  connection — documented in the README.

## [0.5.2] — 2026-07-15

### Fixed
- Poll no longer dies on restricted/managed servers (RDS, Cloud SQL, …):
  replication/WAL queries are best-effort — a denied view degrades to an
  absent panel instead of a dead poll; poll errors now carry the real
  PostgreSQL message + SQLSTATE.
- The Homebrew cask clears the Gatekeeper quarantine itself (`postflight`)
  — Homebrew removed the `--no-quarantine` flag.

## [0.5.1] — 2026-07-15

### Fixed
- Connecting is instant again: the slow schema collection no longer blocks
  the first snapshot, and estimated bloat is **on-demand** (`R` in the
  Schema Lens) — the ioguix bloat queries are too heavy for the auto cadence.

## [0.5.0] — 2026-07-15

### Added
- **Query Lens**: `pg_stat_statements` top statements (calls, total/mean
  time, rows, hit%) with a friendly explainer when the extension is
  missing/old.
- **Replication/WAL panel**: `pg_stat_replication` (primary) /
  `pg_stat_wal_receiver` (standby) with tiered lag severity.
- **Admin actions**: `c` cancel / `K` terminate with a confirmation modal
  (TUI only at the time).
- **Pause**: spacebar freezes the view for point-in-time analysis.
- Web SQL syntax highlighting.
- Published to **crates.io** (`cargo install pg_lens_tui`, binstall
  metadata included).

## [0.4.0] — 2026-07-15

### Added
- Interactive service picker on startup (no flags + valid services file).
- TUI polish pass; demo gif + web screenshot.

## [0.3.0] — 2026-07-14

### Added
- **Schema Lens**: `pg_stat_user_tables` + on-disk sizes on a slow cadence,
  estimated table/index bloat (ioguix queries, BSD-2-Clause attribution),
  severity markers.

## [0.2.x] — 2026-07-14

### Added
- Binary renamed to `pg_lens`; Homebrew tap (formula + cask), Docker/GHCR
  image, deb/rpm packages — full distribution pipeline from one tag push.
- Advanced connections: libpq env vars, `services.toml` with
  `password_cmd` (secrets from vault/keychain, never in the file).

## [0.1.0] — 2026-07-14

### Added
- MVP: Macro Lens (vitals, TPS/sessions sparklines) + Micro Lens (activity
  with blocked/waiting markers, detail panel), real data layer with
  version-gated SQL (PG 13+), resilient poller (reconnect + last-good-data
  banner), mock mode, PTY e2e harness.
- **Web Lens**: `pg_lens serve` — axum + SSE streaming the same snapshots,
  embedded TypeScript frontend, bearer-token auth, non-loopback bind
  refused without a token.
