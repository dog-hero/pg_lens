//! TEA-style Model + update for the TUI.
//!
//! `App` is pure state; [`update`] is the only place that mutates it. The
//! `Action` enum is internal to this crate — `pg_lens_core` never sees it.

use std::sync::Arc;
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use pg_lens_core::{AdminCommand, AdminKind, AdminOutcome, DbSnapshot, PollerStatus};
use ratatui::widgets::TableState;

/// Default poll interval; `+`/`-` move it in [`REFRESH_STEP`] steps.
pub const DEFAULT_REFRESH: Duration = Duration::from_secs(2);
const REFRESH_STEP: Duration = Duration::from_millis(500);
const REFRESH_MIN: Duration = Duration::from_millis(500);
const REFRESH_MAX: Duration = Duration::from_secs(10);
/// How long admin feedback stays on screen: ticks are 250ms, so 40 ≈ 10s.
/// Tick-based on purpose — the view stays synchronous, no timers in `ui/`.
pub const ADMIN_FEEDBACK_TICKS: u64 = 40;

/// How long (in 250ms UI ticks) an Esc press stays "armed" for quitting —
/// ~2s: long enough to read the hint and confirm, short enough that a stray
/// Esc doesn't leave a quit landmine behind.
pub const ESC_QUIT_WINDOW_TICKS: u64 = 8;

/// `PageUp`/`PageDown` step, in rows. A fixed constant rather than the
/// visible table height: `App` never learns the frame size (that lives only
/// in `ui/`, which is 100% synchronous rendering with no channel back to the
/// model), and 10 is a reasonable page on any terminal this app targets —
/// smaller than the shortest table body, but a clear jump versus single-row
/// `j`/`k`.
const PAGE_SIZE: i64 = 10;

/// Which lens (tab) is on screen.
// The "Lens" postfix is the product vocabulary (Macro/Micro/Schema Lens),
// not naming noise — keep it despite clippy's shared-postfix lint.
#[allow(clippy::enum_variant_names)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tab {
    #[default]
    MacroLens,
    MicroLens,
    /// The full replication view (U1): all senders/receiver + every slot as
    /// a scrollable table. The Macro Lens keeps its own compact, capped
    /// summary — this lens is where nothing clips.
    ReplicationLens,
    SchemaLens,
    /// The index advisor (U1), promoted out of the Schema Lens's old `i`
    /// toggle into its own full-height tab.
    IndexLens,
    QueryLens,
}

impl Tab {
    // v0.12: number-prefixed so the tab bar is self-documenting about the
    // `1`-`6` direct-jump keys (see `handle_key`'s digit arm). The prefix is
    // additive on top of the original title text (never replaces it) so
    // every pre-existing `screen.contains("Macro Lens")`-style assertion
    // keeps matching unchanged.
    pub const TITLES: [&'static str; 6] = [
        "1 Macro Lens",
        "2 Micro Lens",
        "3 Replication",
        "4 Schema Lens",
        "5 Indexes",
        "6 Query Lens",
    ];

    pub fn index(self) -> usize {
        match self {
            Tab::MacroLens => 0,
            Tab::MicroLens => 1,
            Tab::ReplicationLens => 2,
            Tab::SchemaLens => 3,
            Tab::IndexLens => 4,
            Tab::QueryLens => 5,
        }
    }

    /// Inverse of [`Tab::index`] — used by the `1`-`6` direct-jump keys.
    /// `None` for anything outside `0..6` (there is no seventh tab).
    pub fn from_index(index: usize) -> Option<Self> {
        match index {
            0 => Some(Tab::MacroLens),
            1 => Some(Tab::MicroLens),
            2 => Some(Tab::ReplicationLens),
            3 => Some(Tab::SchemaLens),
            4 => Some(Tab::IndexLens),
            5 => Some(Tab::QueryLens),
            _ => None,
        }
    }

    pub fn next(self) -> Self {
        match self {
            Tab::MacroLens => Tab::MicroLens,
            Tab::MicroLens => Tab::ReplicationLens,
            Tab::ReplicationLens => Tab::SchemaLens,
            Tab::SchemaLens => Tab::IndexLens,
            Tab::IndexLens => Tab::QueryLens,
            Tab::QueryLens => Tab::MacroLens,
        }
    }

    /// Backward cycle (`BackTab` / Shift+Tab) — the exact inverse of
    /// [`Tab::next`].
    pub fn prev(self) -> Self {
        match self {
            Tab::MacroLens => Tab::QueryLens,
            Tab::MicroLens => Tab::MacroLens,
            Tab::ReplicationLens => Tab::MicroLens,
            Tab::SchemaLens => Tab::ReplicationLens,
            Tab::IndexLens => Tab::SchemaLens,
            Tab::QueryLens => Tab::IndexLens,
        }
    }
}

/// Sort column of the Micro Lens table; `s` cycles through the variants.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SortMode {
    /// Longest-running first.
    #[default]
    Duration,
    /// Alphabetical by state, then pid.
    State,
    /// Ascending pid.
    Pid,
}

impl SortMode {
    pub fn next(self) -> Self {
        match self {
            SortMode::Duration => SortMode::State,
            SortMode::State => SortMode::Pid,
            SortMode::Pid => SortMode::Duration,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            SortMode::Duration => "duration",
            SortMode::State => "state",
            SortMode::Pid => "pid",
        }
    }
}

/// Sort column of the Schema Lens table; `s` cycles through the variants
/// while that lens is active (the Micro Lens keeps its own [`SortMode`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SchemaSortMode {
    /// Largest total relation size first (the lens's default).
    #[default]
    TotalSize,
    /// Most dead tuples first.
    DeadTuples,
    /// Highest estimated bloat% first; tables without a usable estimate
    /// (`is_na` or no matching bloat row) sort last.
    BloatPct,
    /// Most sequential scans first.
    SeqScans,
}

impl SchemaSortMode {
    pub fn next(self) -> Self {
        match self {
            SchemaSortMode::TotalSize => SchemaSortMode::DeadTuples,
            SchemaSortMode::DeadTuples => SchemaSortMode::BloatPct,
            SchemaSortMode::BloatPct => SchemaSortMode::SeqScans,
            SchemaSortMode::SeqScans => SchemaSortMode::TotalSize,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            SchemaSortMode::TotalSize => "size",
            SchemaSortMode::DeadTuples => "dead",
            SchemaSortMode::BloatPct => "bloat%",
            SchemaSortMode::SeqScans => "seq",
        }
    }
}

/// Which sub-view of the Schema Lens is on screen (U3, `v` toggles). Mirrors
/// U1's retired `SchemaView::{Tables,Indexes}` toggle in shape — a full-
/// height sub-view instead of a squeezed footer — but this one stays INSIDE
/// the Schema Lens: XID wraparound/vacuum debt is per-database schema
/// health, not its own top-level lens the way the index advisor became.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SchemaView {
    /// The per-table stats + estimated bloat list (the lens's default). The
    /// vacuum/wraparound block shrinks to a one-line headline + hint here —
    /// see `ui/schema_lens.rs::draw_vacuum_footer`.
    #[default]
    Tables,
    /// Full-height: cluster wraparound headline, the COMPLETE worst-tables
    /// list (all `VACUUM_TABLES_LIMIT` rows, scrollable via its own
    /// `vacuum_table_state`), and the in-flight vacuum progress section.
    Vacuum,
}

impl SchemaView {
    pub fn next(self) -> Self {
        match self {
            SchemaView::Tables => SchemaView::Vacuum,
            SchemaView::Vacuum => SchemaView::Tables,
        }
    }
}

/// Which body the Micro Lens shows (v0.11, `I` toggles). Same shape as
/// [`SchemaView`]'s Tables/Vacuum swap: a full body replacement, not an
/// overlay panel like `waits_open`/`detail_open` — the idle census reuses
/// the SAME table component (see `ui/micro_lens.rs::draw_idle_table`)
/// rather than crowding the active-session table with more columns.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MicroView {
    /// The active-session table (the lens's default) — everything this
    /// module already rendered before v0.11.
    #[default]
    Activity,
    /// The idle connection / connection-age census: every `state = 'idle'`
    /// session, oldest first, with its own cursor (`idle_table_state`).
    Idle,
}

impl MicroView {
    pub fn next(self) -> Self {
        match self {
            MicroView::Activity => MicroView::Idle,
            MicroView::Idle => MicroView::Activity,
        }
    }
}

/// Fixed severity order of one [`pg_lens_core::IndexFinding`] — lower sorts
/// first. `Invalid` (red) is the strongest claim — a failed `CREATE INDEX
/// CONCURRENTLY` is both dead weight and a build that needs cleanup/retry;
/// `Unused` (red) is the strongest read-based, cheapest-to-verify claim;
/// `DuplicatePrefix` (dim-yellow) the weakest. Shared by [`resort_indexes`]
/// (row order) and `ui/index_lens.rs` (marker/color), so the two never
/// disagree about which finding is "worse". The Index Lens (U1) has no
/// user-chosen sort mode of its own — this fixed order is it.
pub fn index_finding_rank(finding: &pg_lens_core::IndexFinding) -> u8 {
    match finding {
        pg_lens_core::IndexFinding::Invalid => 0,
        pg_lens_core::IndexFinding::Unused => 1,
        pg_lens_core::IndexFinding::DuplicateExact { .. } => 2,
        pg_lens_core::IndexFinding::DuplicatePrefix { .. } => 3,
        pg_lens_core::IndexFinding::None => 4,
    }
}

/// Fixed severity rank of one [`pg_lens_core::ReplicationSlotRow`] — lower
/// sorts first (worse first). Mirrors the Macro Lens's original
/// `slot_severity` rule exactly: `wal_status` of `unreserved`/`lost` is
/// always worst; otherwise an inactive slot retaining WAL is a rising
/// concern (>10 GB is as bad as it gets), and anything else is calm. Pure
/// core logic (no ratatui) so it can be the single source of truth for both
/// [`resort_replication`] (row order) and `ui/replication.rs` (marker/color)
/// — the two must never disagree about which slot is worse.
pub fn slot_severity_rank(slot: &pg_lens_core::ReplicationSlotRow) -> u8 {
    if matches!(slot.wal_status.as_deref(), Some("unreserved") | Some("lost")) {
        return 0;
    }
    if !slot.active {
        let retained = slot.retained_wal_bytes.unwrap_or(0);
        if retained > 10 * 1024 * 1024 * 1024 {
            return 0;
        }
        if retained > 0 {
            return 1;
        }
    }
    2
}

/// Sort column of the Query Lens table; `s` cycles through the variants
/// while that lens is active (each lens keeps its own sort mode).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StatementsSortMode {
    /// Highest total execution time first (the lens's default — matches
    /// the SQL's ORDER BY).
    #[default]
    TotalTime,
    /// Most calls first.
    Calls,
    /// Highest mean execution time first.
    Mean,
    /// Most rows first.
    Rows,
}

impl StatementsSortMode {
    pub fn next(self) -> Self {
        match self {
            StatementsSortMode::TotalTime => StatementsSortMode::Calls,
            StatementsSortMode::Calls => StatementsSortMode::Mean,
            StatementsSortMode::Mean => StatementsSortMode::Rows,
            StatementsSortMode::Rows => StatementsSortMode::TotalTime,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            StatementsSortMode::TotalTime => "total",
            StatementsSortMode::Calls => "calls",
            StatementsSortMode::Mean => "mean",
            StatementsSortMode::Rows => "rows",
        }
    }
}

/// The `table_bloat` row matching a table, joined by (schema, name). The
/// sort (here) and the view (`ui/schema_lens.rs`) must agree on this join.
pub fn find_table_bloat<'a>(
    schema: &'a pg_lens_core::SchemaSnapshot,
    table: &pg_lens_core::TableStatRow,
) -> Option<&'a pg_lens_core::BloatRow> {
    schema
        .table_bloat
        .iter()
        .find(|b| b.schema == table.schema && b.name == table.name)
}

/// The `tables` row matching a Vacuum sub-view worst-table row, joined by
/// (schema, name) — the U3 twin of [`find_table_bloat`], used to show "last
/// (auto)vacuum" without a new SQL column: `vacuum_table_ages.sql`'s own doc
/// comment already promises every row has a `table_stats` partner (both
/// share the `pg_stat_user_tables` scope), though a table that fell out of
/// `table_stats`'s own row cap is handled gracefully (`None`, never a panic).
pub fn find_table_for_vacuum_row<'a>(
    schema: &'a pg_lens_core::SchemaSnapshot,
    row: &pg_lens_core::VacuumTableRow,
) -> Option<&'a pg_lens_core::TableStatRow> {
    schema
        .tables
        .iter()
        .find(|t| t.schema == row.schema && t.name == row.name)
}

/// One selectable row of the startup service picker. Built in `main.rs`
/// from `settings::list_services` summaries — display-safe by construction
/// (name + host/user only; a `password`/`password_cmd` never reaches here).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PickerEntry {
    /// Display name (`[services.<name>]`, or "localhost" for the default).
    pub name: String,
    /// What the services file says, verbatim: `user@host` with `?` for
    /// fields the entry leaves out (env/default fallbacks NOT applied), or
    /// `(default)` for the final localhost entry.
    pub detail: String,
    /// `Some(name)` = resolve with this service; `None` = the plain
    /// no-service default resolution (`host=localhost user=postgres`).
    pub service: Option<String>,
}

/// State of the startup service picker (`App::picker`); present only while
/// the picker is on screen — no poller exists yet during that time.
#[derive(Clone, Debug)]
pub struct PickerState {
    pub entries: Vec<PickerEntry>,
    /// Index into `entries`; j/k/↑/↓ move it, saturating at both ends
    /// (same behavior as the lens tables).
    pub selected: usize,
}

impl PickerState {
    pub fn new(entries: Vec<PickerEntry>) -> Self {
        Self {
            entries,
            selected: 0,
        }
    }
}

/// State of the in-session database picker (`d`, any lens, U2). Unlike the
/// startup [`PickerState`] (a pre-poller, full-screen mode), this is an
/// OVERLAY on top of the dashboard — a poller already exists, and PostgreSQL
/// cannot switch databases without reconnecting, so Enter here always means
/// "ask the poller to reconnect", never an in-place update.
#[derive(Clone, Debug)]
pub struct DbPickerState {
    /// Snapshot of `DbSnapshot::databases` taken when the picker opened —
    /// like the startup picker's `entries`, this list does not live-refresh
    /// while the overlay is on screen.
    pub entries: Vec<pg_lens_core::DatabaseRow>,
    /// Index into `entries`; j/k/↑/↓ move it, saturating at both ends.
    pub selected: usize,
}

impl DbPickerState {
    /// Starts the cursor on the currently connected database when it is
    /// among the entries (a small UX nicety: the picker opens already
    /// pointing at "you are here"), falling back to the first entry.
    pub fn new(entries: Vec<pg_lens_core::DatabaseRow>, current_database: &str) -> Self {
        let selected = entries
            .iter()
            .position(|e| e.name == current_database)
            .unwrap_or(0);
        Self { entries, selected }
    }
}

/// State of the admin confirmation modal (`c` = cancel query, `K` =
/// terminate backend, Micro Lens only). While `App::confirm` is `Some`,
/// every key except `y` (confirm) and `n`/`Esc` (abort) is inert.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfirmState {
    pub command: AdminCommand,
    /// Target row's user/database, shown in the modal ("user@db").
    pub username: String,
    pub database: String,
}

/// Transient admin-action feedback rendered above the body ("cancel sent to
/// PID 1234…", then the outcome). Expires by tick count — no timers in ui/.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdminFeedback {
    pub text: String,
    /// Errors (and the returned-false privilege case) render loud/red.
    pub error: bool,
    /// The `App::tick_count` at which the message disappears.
    pub expires_at_tick: u64,
}

/// Everything that can happen, funneled through one mpsc channel.
#[derive(Clone, Debug)]
pub enum Action {
    Key(KeyEvent),
    /// Terminal was resized. Carries no dimensions on purpose: the next
    /// `Terminal::draw` reads the real size from the frame; the action only
    /// exists to wake the loop up for an immediate redraw.
    Resize,
    Snapshot(Arc<DbSnapshot>),
    /// The connection label resolved after a picker selection (`main.rs`
    /// spawns the poller lazily and feeds the display-safe `user@host`
    /// label back through update() — the sole mutation point).
    HostLabel(String),
    /// `main.rs` finished (or refused) an `!`-requested `psql` session and
    /// reports the outcome as statusbar feedback — the same
    /// `AdminFeedback` mechanism `c`/`K` already use, so nothing new needs
    /// to be rendered. `update()` stays the sole place `App` mutates even
    /// though the actual suspend/spawn/restore happened in `main.rs` (see
    /// its module doc for why that dance cannot live in `update()` or
    /// `ui/`).
    PsqlResult { text: String, error: bool },
    Tick,
    Quit,
}

/// The Model: pure state, no I/O.
#[derive(Debug)]
pub struct App {
    pub active_tab: Tab,
    /// The lens `active_tab` held before the last tab change (forward `Tab`,
    /// `BackTab`, or a `1`-`6` jump), if any — set right before every such
    /// mutation. `Backspace` swaps `active_tab`/`previous_tab` (browser-back);
    /// swapping again toggles right back, so repeated presses bounce between
    /// the two most recent lenses rather than only ever going one step deep.
    pub previous_tab: Option<Tab>,
    pub snapshot: Arc<DbSnapshot>,
    /// Indices into `snapshot.activity` in display order (see `sort_mode`).
    pub row_order: Vec<usize>,
    pub sort_mode: SortMode,
    pub table_state: TableState,
    /// Indices into `snapshot.schema.tables` in display order (Schema Lens
    /// twin of `row_order`; see `schema_sort_mode`).
    pub schema_row_order: Vec<usize>,
    pub schema_sort_mode: SchemaSortMode,
    /// Schema Lens selection, independent from the Micro Lens one so
    /// switching lenses never loses either cursor.
    pub schema_table_state: TableState,
    /// Which Schema Lens sub-view is on screen (U3, `v` toggles); see
    /// [`SchemaView`].
    pub schema_view: SchemaView,
    /// Vacuum sub-view's worst-tables selection (U3), independent from
    /// `schema_table_state` — switching `v` never loses either cursor. Reads
    /// `snapshot.schema.vacuum_tables` directly: that vector is already
    /// worst-first from its own `ORDER BY` and carries no user sort/filter,
    /// so (unlike `schema_row_order`) no separate display-order vec exists.
    pub vacuum_table_state: TableState,
    /// Indices into `snapshot.schema.indexes` in severity-then-size display
    /// order (the Index Lens's twin of `schema_row_order`; no sort mode of
    /// its own — see [`index_finding_rank`]).
    pub index_row_order: Vec<usize>,
    /// Index Lens selection, independent from every other lens's cursor
    /// (U1: it used to share `SchemaView::Indexes`'s state before the
    /// promotion to its own tab; the field name is unchanged).
    pub index_table_state: TableState,
    /// Indices into `snapshot.replication_slots` in severity-then-retained
    /// display order (the Replication Lens's twin of `index_row_order`; see
    /// [`slot_severity_rank`]).
    pub replication_row_order: Vec<usize>,
    /// Replication Lens slot-table selection, independent from every other
    /// lens's cursor.
    pub replication_table_state: TableState,
    /// Indices into `snapshot.statements.statements` in display order
    /// (Query Lens twin of `row_order`; see `statements_sort_mode`).
    pub statements_row_order: Vec<usize>,
    pub statements_sort_mode: StatementsSortMode,
    /// Query Lens selection, independent like the schema one.
    pub statements_table_state: TableState,
    /// Whether the detail panel is open (Micro Lens: full query of the
    /// selected session; Schema Lens: full vacuum/analyze stats + index
    /// bloat of the selected table). While open: `j`/`k` still move the
    /// selection (the panel follows it), `Enter`/`Esc` close the panel,
    /// `Tab` closes it and switches lens, `q` quits as always.
    pub detail_open: bool,
    /// Whether the Micro Lens's full waits panel is open (U3, `w` toggles).
    /// Overlay semantics like `detail_open` (they are mutually exclusive —
    /// opening one closes the other): `Esc` closes it WITHOUT arming the
    /// top-level quit barrier, `Tab` closes it and switches lens.
    pub waits_open: bool,
    /// Which body the Micro Lens shows (v0.11, `I` toggles) — see
    /// [`MicroView`]. Persists across Tab switches like `schema_view`.
    pub micro_view: MicroView,
    /// Idle census cursor (v0.11), independent from `table_state` — toggling
    /// `I` never loses either cursor, same contract as
    /// `vacuum_table_state`/`schema_table_state`.
    pub idle_table_state: TableState,
    /// Times `R` was pressed (schema force-recollect). The main loop mirrors
    /// this counter into the poller's `watch::Sender<u64>` after every
    /// update — the same message-passing pattern as `refresh_interval`.
    /// `R` works from any lens: recollecting is harmless and the result is
    /// waiting when the user tabs over.
    pub schema_refresh_requests: u64,
    /// Connection label shown in the header (`PG 16.3 @ user@host`); the
    /// core's `ConnLabel` (resolved in `main.rs`) — the full DSN/`Config`
    /// (which may carry a password) never reaches the view.
    pub host: String,
    /// Desired poll interval. The main loop mirrors this into the poller's
    /// `watch::Receiver<Duration>` after every update, so `+`/`-` take
    /// effect live (Fase 4).
    pub refresh_interval: Duration,
    /// When the last `Action::Snapshot` arrived — drives the staleness
    /// indicator in the statusbar. `None` until the first snapshot.
    pub last_snapshot_at: Option<Instant>,
    /// When the first *Ok* snapshot arrived — `None` means real data has
    /// never been on screen, which is exactly the condition for the
    /// full-screen connection splash (`ui/splash.rs`). Once set it never
    /// clears: later disconnects keep the banner-over-last-data behavior.
    pub first_data_at: Option<Instant>,
    /// Counts `Action::Tick` (250ms cadence) — drives the splash spinner
    /// animation. Mutated only in [`update`], like everything else.
    pub tick_count: u64,
    /// `Some` while the startup service picker is on screen (no poller
    /// exists yet); `None` in normal operation. Set once by `main.rs`
    /// before the loop starts, cleared by Enter inside [`update`].
    pub picker: Option<PickerState>,
    /// The entry chosen in the picker. Set (once, by Enter in [`update`])
    /// and never cleared; `main.rs` watches it to spawn the real poller.
    pub picked: Option<PickerEntry>,
    /// `Some` while the admin confirmation modal is on screen (`c`/`K` on
    /// the Micro Lens). All other keys are inert until y/n/Esc resolves it.
    pub confirm: Option<ConfirmState>,
    /// `Some` while the in-session database picker is on screen (`d`, any
    /// lens, U2). Overlay semantics like `confirm`: every other key is
    /// inert while it is open, and Esc closes it WITHOUT arming the quit
    /// barrier.
    pub db_picker: Option<DbPickerState>,
    /// The database name picked by Enter in `db_picker`, queued for the main
    /// loop to forward to the poller (same mirror pattern as
    /// `schema_refresh_requests`/`pending_admin`). `None` once forwarded, or
    /// whenever there is nothing to switch to.
    pub pending_db_switch: Option<String>,
    /// Set once by `main.rs` at startup (`--mock`); read by
    /// `handle_db_picker_key` to show the "not simulated" toast instead of
    /// queuing a real switch that no mock poller would ever act on.
    pub is_mock: bool,
    /// Set once by `main.rs` at startup (`--read-only` / `PG_LENS_READ_ONLY`
    /// / `config.toml`'s `read_only = true`). The real gate: `open_confirm`
    /// refuses `c`/`K` BEFORE the confirmation modal opens (never mind
    /// `pending_admin`/`AdminCommand`) whenever this is true — hiding the
    /// keys in the UI alone would not be enforcement. Surfaced in the header
    /// as a permanent `RO` marker so the mode is never silently active.
    pub read_only: bool,
    /// Admin commands confirmed by `y` but not yet handed to the poller.
    /// `update()` only queues (pure state); the main loop drains this into
    /// the poller's `mpsc::Sender<AdminCommand>` after every update — the
    /// same mirror pattern as `refresh_interval`/`schema_refresh_requests`.
    pub pending_admin: Vec<AdminCommand>,
    /// Transient statusline for admin actions (sent/succeeded/failed).
    pub admin_feedback: Option<AdminFeedback>,
    /// `at_epoch_ms` of the last `last_admin_action` already announced —
    /// the poller re-stamps its most recent result on every snapshot, so
    /// feedback must fire once per result, not once per snapshot.
    pub admin_seen_epoch_ms: Option<u64>,
    /// UI-side freeze (`Space`): while true, incoming snapshots park in
    /// `pending_snapshot` instead of replacing `snapshot`, so every surface
    /// (tables, sparklines, detail panels, schema) renders point-in-time
    /// data. The poller keeps running untouched — DB load is unchanged;
    /// this is purely a display freeze.
    pub paused: bool,
    /// The newest snapshot that arrived while paused (last-wins: each
    /// arrival replaces the previous one). Resume applies it — the view
    /// jumps straight to the latest data, never replays intermediates.
    pub pending_snapshot: Option<Arc<DbSnapshot>>,
    /// Micro Lens activity filter (case-insensitive substring over pid, db,
    /// user, application, client, state, wait and query text). Empty = no
    /// filter. Applied in [`resort`] before sorting, so the cursor and admin
    /// actions operate only on visible rows.
    pub filter: String,
    /// `true` while the user is typing the filter (`/`): printable keys edit
    /// [`filter`] live, Enter commits, Esc reverts to [`filter_saved`]. All
    /// lens keybindings are inert during editing.
    pub filter_editing: bool,
    /// The filter value captured when editing began, restored on Esc.
    pub filter_saved: String,
    /// v0.12: Schema Lens Tables-view filter — the exact twin of `filter`/
    /// `filter_editing`/`filter_saved`, but its OWN field: a shared/generic
    /// filter field would leak the search term across lenses when tabbing
    /// (e.g. typing "orders" on the Schema Lens would silently narrow the
    /// Micro Lens's activity table too). Case-insensitive substring over
    /// schema name, table name, and the fully-qualified `schema.table`.
    /// Applied in [`resort_schema`] before sorting; Tables view only — the
    /// Vacuum sub-view has no filter of its own (same "no filter" story as
    /// the Micro Lens's idle census).
    pub schema_filter: String,
    pub schema_filter_editing: bool,
    pub schema_filter_saved: String,
    /// v0.12: Query Lens filter — the same per-lens-state discipline as
    /// [`schema_filter`](App::schema_filter). Case-insensitive substring
    /// over the normalized query text (and queryid, if present). Applied in
    /// [`resort_statements`] before sorting.
    pub statements_filter: String,
    pub statements_filter_editing: bool,
    pub statements_filter_saved: String,
    /// Double-Esc quit barrier: `Some(tick)` while a first top-level Esc is
    /// armed — a second Esc at or before that tick quits; later ones re-arm.
    pub esc_quit_armed_until: Option<u64>,
    /// `true` while the keyboard-help overlay (`?`) is on screen — a static,
    /// no-data modal (see `ui/help.rs`). Overlay semantics like `confirm`/
    /// `db_picker`: every other key is inert while it is open, and Esc (or
    /// `?` again) closes it WITHOUT arming the top-level quit barrier. It
    /// takes priority over every other overlay's Esc handling — see the
    /// dedicated check at the top of [`handle_key`].
    pub help_open: bool,
    /// `true` for exactly one pass through `main.rs`'s loop after `!` is
    /// pressed — the loop is the only place that owns the terminal and can
    /// suspend it to spawn `psql`, so `update()` can only request the
    /// action, never perform it. `main.rs` clears this flag (via
    /// `std::mem::take`) the instant it observes it, then reports the
    /// outcome back through [`Action::PsqlResult`].
    pub launch_psql_requested: bool,
    pub should_quit: bool,
}

impl App {
    pub fn new() -> Self {
        let mut app = Self {
            active_tab: Tab::default(),
            previous_tab: None,
            snapshot: Arc::new(DbSnapshot::mock()),
            row_order: Vec::new(),
            sort_mode: SortMode::default(),
            table_state: TableState::default().with_selected(0),
            schema_row_order: Vec::new(),
            schema_sort_mode: SchemaSortMode::default(),
            schema_table_state: TableState::default().with_selected(0),
            schema_view: SchemaView::default(),
            vacuum_table_state: TableState::default().with_selected(0),
            index_row_order: Vec::new(),
            index_table_state: TableState::default().with_selected(0),
            replication_row_order: Vec::new(),
            replication_table_state: TableState::default().with_selected(0),
            statements_row_order: Vec::new(),
            statements_sort_mode: StatementsSortMode::default(),
            statements_table_state: TableState::default().with_selected(0),
            detail_open: false,
            waits_open: false,
            micro_view: MicroView::default(),
            idle_table_state: TableState::default().with_selected(0),
            schema_refresh_requests: 0,
            host: "localhost".to_string(),
            refresh_interval: DEFAULT_REFRESH,
            last_snapshot_at: None,
            first_data_at: None,
            tick_count: 0,
            picker: None,
            picked: None,
            confirm: None,
            db_picker: None,
            pending_db_switch: None,
            is_mock: false,
            read_only: false,
            pending_admin: Vec::new(),
            admin_feedback: None,
            admin_seen_epoch_ms: None,
            paused: false,
            pending_snapshot: None,
            filter: String::new(),
            filter_editing: false,
            filter_saved: String::new(),
            schema_filter: String::new(),
            schema_filter_editing: false,
            schema_filter_saved: String::new(),
            statements_filter: String::new(),
            statements_filter_editing: false,
            statements_filter_saved: String::new(),
            esc_quit_armed_until: None,
            help_open: false,
            launch_psql_requested: false,
            should_quit: false,
        };
        resort(&mut app);
        resort_schema(&mut app);
        resort_indexes(&mut app);
        resort_replication(&mut app);
        resort_statements(&mut app);
        app
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

impl App {
    /// The activity row currently under the cursor, in display order
    /// (`table_state` indexes `row_order`, which indexes the snapshot).
    pub fn selected_row(&self) -> Option<&pg_lens_core::ActivityRow> {
        let display_idx = self.table_state.selected()?;
        let snapshot_idx = *self.row_order.get(display_idx)?;
        self.snapshot.activity.get(snapshot_idx)
    }

    /// Whether the full-screen connection splash renders instead of the
    /// dashboard: true only while no Ok snapshot has EVER arrived AND the
    /// poller is not currently Ok (pre-first-data Connecting/Error). After
    /// the first real data, errors fall back to the classic banner.
    pub fn show_splash(&self) -> bool {
        self.first_data_at.is_none() && !matches!(self.snapshot.status, PollerStatus::Ok)
    }

    /// The Schema Lens table currently under the cursor, in display order.
    pub fn selected_table(&self) -> Option<&pg_lens_core::TableStatRow> {
        let schema = self.snapshot.schema.as_deref()?;
        let display_idx = self.schema_table_state.selected()?;
        let snapshot_idx = *self.schema_row_order.get(display_idx)?;
        schema.tables.get(snapshot_idx)
    }

    /// The Index Advisor row currently under the cursor, in display order
    /// (Indexes view of the Schema Lens).
    pub fn selected_index(&self) -> Option<&pg_lens_core::IndexRow> {
        let schema = self.snapshot.schema.as_deref()?;
        let display_idx = self.index_table_state.selected()?;
        let snapshot_idx = *self.index_row_order.get(display_idx)?;
        schema.indexes.get(snapshot_idx)
    }

    /// The Query Lens statement currently under the cursor, in display order.
    pub fn selected_statement(&self) -> Option<&pg_lens_core::StatementRow> {
        let statements = self.snapshot.statements.as_deref()?;
        let display_idx = self.statements_table_state.selected()?;
        let snapshot_idx = *self.statements_row_order.get(display_idx)?;
        statements.statements.get(snapshot_idx)
    }
}

/// The single mutation point of the Model.
pub fn update(app: &mut App, action: Action) {
    match action {
        Action::Key(key) => handle_key(app, key),
        Action::Snapshot(snapshot) => {
            if app.paused {
                // Frozen: park the newest arrival (last-wins) instead of
                // applying it. `last_snapshot_at` stays put on purpose —
                // the statusbar staleness keeps counting up, telling the
                // user exactly how old the frozen picture is.
                app.pending_snapshot = Some(snapshot);
            } else {
                apply_snapshot(app, snapshot);
            }
        }
        // The next draw reads the new terminal size from the frame itself.
        Action::Resize => {}
        Action::HostLabel(label) => app.host = label,
        Action::PsqlResult { text, error } => {
            app.admin_feedback = Some(AdminFeedback {
                text,
                error,
                expires_at_tick: app.tick_count + ADMIN_FEEDBACK_TICKS,
            });
        }
        // Advance the splash spinner / feedback clock (and force a redraw).
        Action::Tick => {
            app.tick_count = app.tick_count.wrapping_add(1);
            if app
                .admin_feedback
                .as_ref()
                .is_some_and(|f| app.tick_count >= f.expires_at_tick)
            {
                app.admin_feedback = None;
            }
        }
        Action::Quit => app.should_quit = true,
    }
}

/// Makes `snapshot` the one on screen: freshness stamp, splash gate,
/// admin-result feedback, re-sorts and selection clamps. Shared by the
/// live path (`Action::Snapshot` while not paused) and [`resume`] (which
/// applies the parked `pending_snapshot`).
fn apply_snapshot(app: &mut App, snapshot: Arc<DbSnapshot>) {
    app.snapshot = snapshot;
    app.last_snapshot_at = Some(Instant::now());
    // First Ok snapshot ever: leave the splash for the dashboard,
    // permanently (see `App::show_splash`).
    if app.first_data_at.is_none() && matches!(app.snapshot.status, PollerStatus::Ok) {
        app.first_data_at = Some(Instant::now());
    }
    note_admin_result(app);
    resort(app);
    resort_schema(app);
    resort_indexes(app);
    resort_replication(app);
    resort_statements(app);
    clamp_selection(app);
}

/// `Space`: freeze the view for point-in-time analysis, or thaw it. Resume
/// jumps to the newest parked snapshot (if any) — see [`resume`].
fn toggle_pause(app: &mut App) {
    if app.paused {
        resume(app);
    } else {
        app.paused = true;
    }
}

/// Unfreezes the view: applies the parked `pending_snapshot` (the LATEST
/// arrival while paused — intermediates were already superseded) so the
/// screen jumps straight to current data.
fn resume(app: &mut App) {
    app.paused = false;
    if let Some(snapshot) = app.pending_snapshot.take() {
        apply_snapshot(app, snapshot);
    }
}

/// Announces a fresh `last_admin_action` (deduped by `at_epoch_ms` — the
/// poller re-stamps its latest result on every snapshot) as feedback text.
fn note_admin_result(app: &mut App) {
    let Some(result) = app.snapshot.last_admin_action.as_ref() else {
        return;
    };
    if app.admin_seen_epoch_ms == Some(result.at_epoch_ms) {
        return;
    }
    app.admin_seen_epoch_ms = Some(result.at_epoch_ms);
    let pid = result.pid;
    let (text, error) = match (&result.kind, &result.outcome) {
        (AdminKind::Cancel, AdminOutcome::Signalled(true)) => {
            (format!("query cancelled (PID {pid})"), false)
        }
        (AdminKind::Terminate, AdminOutcome::Signalled(true)) => {
            (format!("backend terminated (PID {pid})"), false)
        }
        // pg_cancel/terminate_backend returned false: the PID vanished, or
        // the connected role may not signal it (needs the same user or
        // pg_signal_backend membership — see README).
        (_, AdminOutcome::Signalled(false)) => (
            format!(
                "PID {pid} not signalled \u{2014} gone or insufficient privilege \
                 (needs same user or pg_signal_backend)"
            ),
            true,
        ),
        (kind, AdminOutcome::Error(msg)) => {
            let verb = match kind {
                AdminKind::Cancel => "cancel",
                AdminKind::Terminate => "terminate",
            };
            // Modern PostgreSQL raises "permission denied to ..." instead
            // of returning false — append the same actionable hint.
            let hint = if msg.contains("permission denied") || msg.contains("must be a member") {
                " (needs same user or pg_signal_backend)"
            } else {
                ""
            };
            (format!("{verb} PID {pid} failed: {msg}{hint}"), true)
        }
    };
    app.admin_feedback = Some(AdminFeedback {
        text,
        error,
        expires_at_tick: app.tick_count + ADMIN_FEEDBACK_TICKS,
    });
}

fn handle_key(app: &mut App, key: KeyEvent) {
    if key.kind != KeyEventKind::Press {
        return;
    }
    // Picker mode: its own tiny keymap — none of the lens keybindings
    // (Tab/s/R/+/-/Enter-detail) are active while it is on screen.
    if app.picker.is_some() {
        handle_picker_key(app, key);
        return;
    }
    // Keyboard help overlay (`?`): the highest-priority overlay — checked
    // before the admin modal/db picker/filter editing below, so its own
    // Esc handling always wins and never falls through to arm the
    // double-Esc quit barrier or close some OTHER overlay underneath.
    // Mutually exclusive with every other overlay by construction: `?` is
    // only recognized in the bottom match (reached only when none of the
    // other overlay checks fired), so it can never open while one of them
    // is already up.
    if app.help_open {
        handle_help_key(app, key);
        return;
    }
    // Admin confirmation modal: y confirms, n/Esc aborts, EVERYTHING else
    // (including q) is deliberately inert — no accidental double-meaning
    // while a destructive action awaits confirmation.
    if app.confirm.is_some() {
        handle_confirm_key(app, key);
        return;
    }
    // In-session database picker (`d`, U2): j/k move, Enter selects, Esc
    // closes — an overlay like `confirm`, so every other key (including q)
    // is inert while it is open.
    if app.db_picker.is_some() {
        handle_db_picker_key(app, key);
        return;
    }
    // Filter editing (`/`): printable keys edit the ACTIVE lens's filter
    // live, so its table narrows as you type; Enter commits, Esc reverts.
    // Every lens keybinding is inert until then. v0.12: three independent
    // filters (Micro/Schema/Query) share this shape but never their state —
    // `filter_target` below picks which triple of fields is live, so typing
    // on one lens can never leak into another's search term.
    if app.filter_editing || app.schema_filter_editing || app.statements_filter_editing {
        handle_filter_key(app, key);
        return;
    }
    match key.code {
        KeyCode::Char('q') => app.should_quit = true,
        // Esc closes the detail panel when it is open. At the top level it
        // ARMS quitting instead of quitting: overlays (detail, filter,
        // modal) all consume Esc, so a hammered Esc used to fall through and
        // kill the app by accident. First press shows "Esc again to quit"
        // for ESC_QUIT_WINDOW_TICKS; a second press inside that window
        // quits. `q` and Ctrl+C stay immediate (deliberate keys).
        KeyCode::Esc => {
            if app.detail_open {
                app.detail_open = false;
            } else if app.waits_open {
                app.waits_open = false;
            } else if app.active_tab == Tab::SchemaLens && app.schema_view != SchemaView::Tables {
                // The `v` Vacuum sub-view is an overlay too: Esc returns to
                // the Tables view, it does NOT arm quitting.
                app.schema_view = SchemaView::Tables;
            } else if app.active_tab == Tab::MicroLens && app.micro_view != MicroView::Activity {
                // The `I` idle census is the same "overlay-like sub-view"
                // story as the Vacuum sub-view above: Esc returns to the
                // Activity table, it does NOT arm quitting.
                app.micro_view = MicroView::Activity;
            } else if app
                .esc_quit_armed_until
                .is_some_and(|until| app.tick_count <= until)
            {
                app.should_quit = true;
            } else {
                app.esc_quit_armed_until = Some(app.tick_count + ESC_QUIT_WINDOW_TICKS);
                app.admin_feedback = Some(AdminFeedback {
                    text: "Press Esc again to quit (q quits immediately)".to_string(),
                    error: false,
                    expires_at_tick: app.tick_count + ESC_QUIT_WINDOW_TICKS,
                });
            }
        }
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.should_quit = true;
        }
        // Admin actions (Micro Lens only, on the selected row; they work
        // with the detail panel open or closed): `c` asks to cancel the
        // query, `K` (uppercase only — deliberate friction; lowercase k
        // stays navigation) asks to terminate the backend. Both only OPEN
        // the confirmation modal; nothing executes before `y`.
        KeyCode::Char('c') => open_confirm(app, false),
        KeyCode::Char('K') => open_confirm(app, true),
        // Enter toggles the detail panel of the active lens's selected row
        // (Micro: session query; Schema: table stats + index bloat; Index
        // Lens: indexdef + duplicate partner, if any). The Replication Lens
        // has no detail panel (U1): every slot field already fits in its
        // row, so Enter is a no-op there, same as the Macro Lens. If the
        // waits panel (U3, `w`) is open, Enter closes it first — overlays
        // never stack, the same "close before open" rule Esc follows.
        KeyCode::Enter => {
            if app.detail_open {
                app.detail_open = false;
            } else if app.waits_open {
                app.waits_open = false;
            } else if (app.active_tab == Tab::MicroLens
                && app.micro_view == MicroView::Activity
                && app.table_state.selected().is_some())
                || (app.active_tab == Tab::SchemaLens
                    && app.schema_view == SchemaView::Tables
                    && app.selected_table().is_some())
                || (app.active_tab == Tab::IndexLens && app.selected_index().is_some())
                || (app.active_tab == Tab::QueryLens && app.selected_statement().is_some())
            {
                app.detail_open = true;
            }
        }
        KeyCode::Tab => {
            app.previous_tab = Some(app.active_tab);
            app.detail_open = false;
            app.waits_open = false;
            app.active_tab = app.active_tab.next();
        }
        // v0.12: Shift+Tab cycles backward — the exact inverse of `Tab`,
        // same overlay-close-then-switch discipline (note `schema_view`/
        // `micro_view` deliberately are NOT reset here, matching the
        // forward `Tab` arm: both sub-views persist across lens switches by
        // design — see their doc comments).
        KeyCode::BackTab => {
            app.previous_tab = Some(app.active_tab);
            app.detail_open = false;
            app.waits_open = false;
            app.active_tab = app.active_tab.prev();
        }
        // v0.12: direct tab jump, in `Tab::TITLES`/`Tab::index()` order (the
        // tab bar's number prefixes document this). Reached only when no
        // overlay/modal/picker/filter-edit owns input (they all return
        // earlier in this function), so it can never hijack a digit typed
        // into the filter editor or a confirm-modal keystroke. A no-op if
        // already on that tab (nothing to remember as "previous").
        KeyCode::Char(c @ '1'..='6') => {
            if let Some(tab) = Tab::from_index(c as usize - '1' as usize)
                && tab != app.active_tab
            {
                app.previous_tab = Some(app.active_tab);
                app.detail_open = false;
                app.waits_open = false;
                app.active_tab = tab;
            }
        }
        // v0.12: "go back" — swaps with `previous_tab` (browser-back), so a
        // second press bounces right back to where you jumped from. Inert
        // with no history yet. NOTE: this arm is only reached at the
        // top level — `handle_filter_key` intercepts Backspace as delete
        // BEFORE `handle_key` ever dispatches here (see the `filter_editing`
        // early-return above), so typing in the filter editor is unaffected.
        KeyCode::Backspace => {
            if let Some(prev) = app.previous_tab {
                let current = app.active_tab;
                app.detail_open = false;
                app.waits_open = false;
                app.active_tab = prev;
                app.previous_tab = Some(current);
            }
        }
        // `/` starts (or resumes) editing the active lens's own filter (see
        // `filter_target`'s doc comment for why there are three of these,
        // never one shared field). Micro Lens's Activity view only — the
        // idle census (v0.11) has no filter of its own; same story for the
        // Schema Lens's Vacuum sub-view (Tables view only, guarded below).
        KeyCode::Char('/')
            if app.active_tab == Tab::MicroLens && app.micro_view == MicroView::Activity =>
        {
            app.filter_saved = app.filter.clone();
            app.filter_editing = true;
        }
        KeyCode::Char('/')
            if app.active_tab == Tab::SchemaLens && app.schema_view == SchemaView::Tables =>
        {
            app.schema_filter_saved = app.schema_filter.clone();
            app.schema_filter_editing = true;
        }
        KeyCode::Char('/') if app.active_tab == Tab::QueryLens => {
            app.statements_filter_saved = app.statements_filter.clone();
            app.statements_filter_editing = true;
        }
        // v0.12: `\` clears the ACTIVE lens's committed filter in one key —
        // inert when there is nothing to clear (empty filter) or while
        // editing (Esc already reverts there). Chosen over `Esc` (already
        // overloaded: closes overlays, then arms the quit barrier — adding
        // a THIRD meaning would make a stray Esc unpredictable) and over a
        // digit/letter already claimed by v0.12's own navigation batch
        // (`1`-`6`, `g`/`G`, Backspace, BackTab) or by an existing lens key
        // (`c`/`d`/`s`/`v`/`w`/`I`/`R`/`K`/`!`/`?`). `\` is unused anywhere
        // in `handle_key` and reads naturally as "cancel/undo the slash".
        KeyCode::Char('\\') => match app.active_tab {
            Tab::MicroLens
                if app.micro_view == MicroView::Activity && !app.filter.is_empty() =>
            {
                app.filter.clear();
                resort(app);
                clamp_selection(app);
            }
            Tab::SchemaLens
                if app.schema_view == SchemaView::Tables && !app.schema_filter.is_empty() =>
            {
                app.schema_filter.clear();
                resort_schema(app);
                clamp_selection(app);
            }
            Tab::QueryLens if !app.statements_filter.is_empty() => {
                app.statements_filter.clear();
                resort_statements(app);
                clamp_selection(app);
            }
            _ => {}
        },
        // `w` (U3): the Micro Lens's full ranked-waits panel — the one-line
        // strip only ever shows the top few; this is the complete list.
        // Toggle, Micro Lens only; opening it closes any open detail panel
        // (overlays never stack — see the Enter/Esc handling above/below).
        KeyCode::Char('w')
            if app.active_tab == Tab::MicroLens && app.micro_view == MicroView::Activity =>
        {
            app.waits_open = !app.waits_open;
            if app.waits_open {
                app.detail_open = false;
            }
        }
        // `v` (U3): toggles the Schema Lens between the Tables list and the
        // full-height Vacuum sub-view (see [`SchemaView`]). Schema Lens
        // only; closes any open Tables-view detail panel (the Vacuum view
        // has none of its own).
        KeyCode::Char('v') if app.active_tab == Tab::SchemaLens => {
            app.schema_view = app.schema_view.next();
            app.detail_open = false;
        }
        // `I` (v0.11, mnemonic "idle"): toggles the Micro Lens between the
        // Activity table and the idle connection / connection-age census
        // (see [`MicroView`]) — the SAME body-swap shape as `v`'s Vacuum
        // sub-view, not a stacking overlay, so it closes any open detail
        // panel (the idle census has none of its own).
        KeyCode::Char('I') if app.active_tab == Tab::MicroLens => {
            app.micro_view = app.micro_view.next();
            app.detail_open = false;
            app.waits_open = false;
        }
        // `d` opens the database picker (U2) from ANY lens — reconnecting is
        // a cluster-wide, not a per-lens, action.
        KeyCode::Char('d') => open_db_picker(app),
        // `!` (v0.11, mnemonic "shell out"): asks `main.rs` (the only place
        // that owns the terminal) to suspend the TUI and open an
        // interactive `psql` shell on the SAME connection. `--mock` has no
        // real connection to hand psql, so it short-circuits here with the
        // same calm "not simulated" feedback the database picker (`d`) uses
        // in mock mode — main.rs never sees the request. Works from any
        // lens, like `d`; every overlay above already returned before this
        // match, so no overlay is ever left dangling underneath psql.
        KeyCode::Char('!') => {
            if app.is_mock {
                app.admin_feedback = Some(AdminFeedback {
                    text: "mock mode: no real connection for psql".to_string(),
                    error: false,
                    expires_at_tick: app.tick_count + ADMIN_FEEDBACK_TICKS,
                });
            } else {
                app.launch_psql_requested = true;
            }
        }
        // `?` opens the keyboard help overlay (v0.9) from ANY lens — a
        // static, no-data modal (see `ui/help.rs`). Closing it is handled
        // entirely by `handle_help_key` (Esc or `?` again), reached via the
        // dedicated check above `handle_key`'s overlay chain.
        KeyCode::Char('?') => app.help_open = true,
        KeyCode::Up | KeyCode::Char('k') => move_selection(app, -1),
        KeyCode::Down | KeyCode::Char('j') => move_selection(app, 1),
        // v0.12: fast scroll on long tables — reuses `move_selection`'s
        // existing per-lens (state, len) routing, so this works on every
        // lens with a selectable table for free. `PAGE_SIZE` is a fixed
        // constant (not derived from the frame height, which `ui/` — the
        // only place that knows terminal size — never reports back to
        // `App`; a constant keeps the model 100% synchronous and terminal-
        // size-independent, same reasoning as `ESC_QUIT_WINDOW_TICKS` being
        // tick-based rather than wall-clock).
        KeyCode::PageUp => move_selection(app, -PAGE_SIZE),
        KeyCode::PageDown => move_selection(app, PAGE_SIZE),
        KeyCode::Home | KeyCode::Char('g') => move_selection_to(app, 0),
        KeyCode::End | KeyCode::Char('G') => move_selection_to(app, i64::MAX),
        // `s` cycles the sort of whichever lens is active (each keeps its
        // own mode, so tabbing away and back never loses the choice). The
        // Index Lens and Replication Lens have no sort mode of their own
        // (fixed severity order — see `index_finding_rank`/
        // `slot_severity_rank`), so `s` is inert there. The Schema Lens's
        // Vacuum sub-view (U3) and the Micro Lens's idle census (v0.11) are
        // the same story: both are fixed worst/oldest-first orders, not a
        // user-chosen sort.
        KeyCode::Char('s') => match app.active_tab {
            Tab::SchemaLens if app.schema_view == SchemaView::Tables => {
                app.schema_sort_mode = app.schema_sort_mode.next();
                resort_schema(app);
            }
            Tab::SchemaLens => {}
            Tab::QueryLens => {
                app.statements_sort_mode = app.statements_sort_mode.next();
                resort_statements(app);
            }
            Tab::IndexLens | Tab::ReplicationLens => {}
            Tab::MicroLens if app.micro_view == MicroView::Idle => {}
            _ => {
                app.sort_mode = app.sort_mode.next();
                resort(app);
            }
        },
        // Space: pause/resume the display refresh (UI-side freeze; the
        // poller keeps its cadence — see `App::paused`). Works in all three
        // lenses AND with a detail panel open (point-in-time analysis is
        // exactly when a detail is being read); inert on the connection
        // splash — there is no data to freeze yet, and pausing there would
        // silently swallow the first snapshot with no indicator on screen.
        // Picker/confirm-modal inertness falls out of their own keymaps
        // (both return before this match).
        KeyCode::Char(' ') => {
            if !app.show_splash() {
                toggle_pause(app);
            }
        }
        // `R` (uppercase, deliberately distinct from the lowercase keys):
        // request an immediate schema re-collection. Allowed from any lens —
        // it is harmless, and the fresh data is ready when the user tabs in.
        // While paused the signal still goes out (the poller recollects as
        // usual) but the result stays parked in `pending_snapshot` until
        // resume — a deliberate, documented freeze-wins choice.
        KeyCode::Char('R') => {
            app.schema_refresh_requests += 1;
        }
        KeyCode::Char('+') | KeyCode::Char('=') => {
            app.refresh_interval = (app.refresh_interval + REFRESH_STEP).min(REFRESH_MAX);
        }
        KeyCode::Char('-') => {
            app.refresh_interval = app
                .refresh_interval
                .saturating_sub(REFRESH_STEP)
                .max(REFRESH_MIN);
        }
        _ => {}
    }
}

/// Opens the admin confirmation modal for the selected Micro Lens row.
/// A no-op on any other lens or with no selection — the keys must never
/// half-work: without a target there is nothing to confirm.
///
/// Read-only mode's REAL gate lives here: when `app.read_only` is set, `c`/
/// `K` are refused before the modal ever opens — no `ConfirmState`, no
/// `pending_admin` entry, no `AdminCommand` reaches the poller. Hiding the
/// keys in a view module would not be enforcement (the model is the only
/// place state mutates); this early return is it.
fn open_confirm(app: &mut App, terminate: bool) {
    // The idle census (v0.11) has its own cursor (`idle_table_state`), not
    // `table_state` — `c`/`K` reading `selected_row()` here would silently
    // act on whatever the Activity table's cursor last pointed at, not the
    // idle row the operator is actually looking at. Out of scope for this
    // census (a future "kill this idle connection" action would need its
    // own selected-row lookup), so both keys stay inert there.
    if app.active_tab != Tab::MicroLens || app.micro_view != MicroView::Activity {
        return;
    }
    let Some(row) = app.selected_row() else {
        return;
    };
    if app.read_only {
        app.admin_feedback = Some(AdminFeedback {
            text: "read-only mode — action disabled".to_string(),
            error: true,
            expires_at_tick: app.tick_count + ADMIN_FEEDBACK_TICKS,
        });
        return;
    }
    let command = if terminate {
        AdminCommand::TerminateBackend(row.pid)
    } else {
        AdminCommand::CancelBackend(row.pid)
    };
    app.confirm = Some(ConfirmState {
        command,
        username: row.username.clone(),
        database: row.database.clone(),
    });
}

/// Keymap of the admin confirmation modal: `y` queues the command (the main
/// loop forwards it to the poller) and shows the "sent…" feedback; `n`/`Esc`
/// abort. Anything else is inert while the modal is open.
fn handle_confirm_key(app: &mut App, key: KeyEvent) {
    let Some(confirm) = app.confirm.as_ref() else {
        return;
    };
    match key.code {
        KeyCode::Char('y') => {
            let command = confirm.command;
            app.pending_admin.push(command);
            let verb = match command.kind() {
                AdminKind::Cancel => "cancel",
                AdminKind::Terminate => "terminate",
            };
            app.admin_feedback = Some(AdminFeedback {
                text: format!("{verb} sent to PID {}\u{2026}", command.pid()),
                error: false,
                expires_at_tick: app.tick_count + ADMIN_FEEDBACK_TICKS,
            });
            app.confirm = None;
            // Design decision: confirming an admin action while paused
            // auto-resumes (apply the parked snapshot + unfreeze). The
            // action's RESULT travels inside the snapshot envelope — on a
            // frozen screen the outcome would never appear; resuming is
            // the simplest behavior that always shows it.
            if app.paused {
                resume(app);
            }
        }
        KeyCode::Char('n') | KeyCode::Esc => app.confirm = None,
        _ => {}
    }
}

/// Opens the in-session database picker (`d`). A no-op when the poller has
/// not yet collected the database list (`snapshot.databases` is `None`
/// before the first successful fast tick, or on any collection failure) —
/// nothing to pick from yet; the key simply does nothing rather than open an
/// empty, useless overlay.
fn open_db_picker(app: &mut App) {
    let Some(databases) = app.snapshot.databases.clone() else {
        return;
    };
    if databases.is_empty() {
        return;
    }
    app.db_picker = Some(DbPickerState::new(databases, &app.snapshot.vitals.database));
}

/// Keymap of the in-session database picker: j/k/↑/↓ move (saturating),
/// Enter selects, Esc closes — WITHOUT arming the quit barrier (it is an
/// overlay, not a top-level Esc; see `KeyCode::Esc` above). `q` is
/// deliberately inert while the picker is open, same convention as the
/// admin confirm modal.
fn handle_db_picker_key(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Esc => app.db_picker = None,
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.should_quit = true;
        }
        KeyCode::Up | KeyCode::Char('k') => {
            if let Some(picker) = app.db_picker.as_mut() {
                picker.selected = picker.selected.saturating_sub(1);
            }
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if let Some(picker) = app.db_picker.as_mut()
                && !picker.entries.is_empty()
            {
                picker.selected = (picker.selected + 1).min(picker.entries.len() - 1);
            }
        }
        KeyCode::Enter => {
            if let Some(picker) = app.db_picker.take()
                && let Some(entry) = picker.entries.get(picker.selected)
            {
                let name = entry.name.clone();
                if name == app.snapshot.vitals.database {
                    // Already connected here: nothing to do.
                } else if app.is_mock {
                    app.admin_feedback = Some(AdminFeedback {
                        text: "mock mode: database switch not simulated".to_string(),
                        error: false,
                        expires_at_tick: app.tick_count + ADMIN_FEEDBACK_TICKS,
                    });
                } else {
                    app.pending_db_switch = Some(name);
                }
            }
        }
        _ => {}
    }
}

/// Keymap of the keyboard help overlay (`?`): `Esc` or `?` again closes it
/// WITHOUT arming the top-level quit barrier — the same overlay-dismissal
/// rule the detail panel and other overlays follow. `Ctrl+C` still quits
/// (universal escape hatch, same convention as the confirm modal and the
/// database picker); every other key — including `q` — is deliberately
/// inert while it is open (it is a pure reference screen, not a place where
/// stray keystrokes should do anything).
fn handle_help_key(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Esc | KeyCode::Char('?') => app.help_open = false,
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.should_quit = true;
        }
        _ => {}
    }
}

/// Which per-lens filter triple (`*_filter`/`*_filter_editing`/
/// `*_filter_saved`) is currently being edited. [`handle_filter_key`]
/// dispatches on this instead of taking a generic `&mut String` target: a
/// shared/generic filter field was explicitly rejected (see `App::filter`'s
/// sibling doc comments) because it would leak one lens's search term into
/// another's row set the moment the user tabbed away mid-edit. This enum is
/// the single least-duplicative point where the THREE keymaps (identical
/// key-by-key behavior, different fields) converge into one implementation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FilterLens {
    Micro,
    Schema,
    Query,
}

/// `None` when no filter is being edited — defensive; `handle_key` only
/// routes into [`handle_filter_key`] when at least one `*_filter_editing`
/// flag is set, and the three flags are mutually exclusive by construction
/// (only one `/` arm can fire per keypress, each setting exactly one).
fn active_filter_lens(app: &App) -> Option<FilterLens> {
    if app.filter_editing {
        Some(FilterLens::Micro)
    } else if app.schema_filter_editing {
        Some(FilterLens::Schema)
    } else if app.statements_filter_editing {
        Some(FilterLens::Query)
    } else {
        None
    }
}

/// Re-sorts (and re-filters — the filter step lives inside each `resort_*`)
/// whichever lens [`FilterLens`] names. Shared by every editing keystroke
/// that changes the filter text.
fn resort_for(app: &mut App, lens: FilterLens) {
    match lens {
        FilterLens::Micro => resort(app),
        FilterLens::Schema => resort_schema(app),
        FilterLens::Query => resort_statements(app),
    }
}

/// Keymap while editing ANY lens's filter (`app.filter_editing` /
/// `schema_filter_editing` / `statements_filter_editing` — exactly one is
/// true when this is reached): every printable char edits that lens's own
/// filter live (its table re-filters on each keystroke), Backspace deletes,
/// Enter commits (keeps the text, stops editing), Esc reverts to what the
/// filter was before editing began. The selection is re-clamped after each
/// change because the visible row count can shrink to zero.
fn handle_filter_key(app: &mut App, key: KeyEvent) {
    // Ctrl+C is a universal escape hatch — it quits even mid-edit.
    if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
        app.should_quit = true;
        return;
    }
    let Some(lens) = active_filter_lens(app) else {
        return;
    };
    match key.code {
        KeyCode::Enter => match lens {
            FilterLens::Micro => app.filter_editing = false,
            FilterLens::Schema => app.schema_filter_editing = false,
            FilterLens::Query => app.statements_filter_editing = false,
        },
        KeyCode::Esc => {
            match lens {
                FilterLens::Micro => {
                    app.filter = std::mem::take(&mut app.filter_saved);
                    app.filter_editing = false;
                }
                FilterLens::Schema => {
                    app.schema_filter = std::mem::take(&mut app.schema_filter_saved);
                    app.schema_filter_editing = false;
                }
                FilterLens::Query => {
                    app.statements_filter = std::mem::take(&mut app.statements_filter_saved);
                    app.statements_filter_editing = false;
                }
            }
            resort_for(app, lens);
            clamp_selection(app);
        }
        KeyCode::Backspace => {
            match lens {
                FilterLens::Micro => {
                    app.filter.pop();
                }
                FilterLens::Schema => {
                    app.schema_filter.pop();
                }
                FilterLens::Query => {
                    app.statements_filter.pop();
                }
            }
            resort_for(app, lens);
            clamp_selection(app);
        }
        // Ignore control chords (e.g. Ctrl+C is handled above already).
        KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            match lens {
                FilterLens::Micro => app.filter.push(c),
                FilterLens::Schema => app.schema_filter.push(c),
                FilterLens::Query => app.statements_filter.push(c),
            }
            resort_for(app, lens);
            clamp_selection(app);
        }
        _ => {}
    }
}

/// Keymap of the startup service picker: j/k/↑/↓ move (saturating, like
/// the lens tables), Enter picks the highlighted entry (main.rs then
/// resolves + spawns the poller), q/Esc/Ctrl+C quit cleanly. Everything
/// else is deliberately inert — there is no poller to talk to yet.
fn handle_picker_key(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Char('q') | KeyCode::Esc => app.should_quit = true,
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.should_quit = true;
        }
        KeyCode::Up | KeyCode::Char('k') => {
            if let Some(picker) = app.picker.as_mut() {
                picker.selected = picker.selected.saturating_sub(1);
            }
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if let Some(picker) = app.picker.as_mut()
                && !picker.entries.is_empty()
            {
                picker.selected = (picker.selected + 1).min(picker.entries.len() - 1);
            }
        }
        KeyCode::Enter => {
            if let Some(picker) = app.picker.take() {
                match picker.entries.get(picker.selected) {
                    Some(entry) => app.picked = Some(entry.clone()),
                    // Empty picker cannot be built (main.rs requires >=1
                    // service), but stay defensive: keep it on screen.
                    None => app.picker = Some(picker),
                }
            }
        }
        _ => {}
    }
}

/// Moves the active lens's table selection by `delta`, saturating at both
/// ends (no wrap). The Macro Lens has no table; j/k default to the Micro
/// Lens cursor there (harmless, matches the pre-S3 behavior).
fn move_selection(app: &mut App, delta: i64) {
    let (state, len) = selection_target(app);
    move_state(state, len, delta);
}

/// Jumps the active lens's table selection to an absolute position, clamped
/// into range: `target <= 0` goes to the first row, `target >=
/// len.saturating_sub(1)` goes to the last (so `i64::MAX` is the idiomatic
/// "last row" — see the `Home`/`End`/`g`/`G` arms in `handle_key`). Shares
/// the exact same per-lens (state, len) routing as [`move_selection`], so it
/// works on every lens that has a selectable table with no per-lens code.
fn move_selection_to(app: &mut App, target: i64) {
    let (state, len) = selection_target(app);
    if len == 0 {
        state.select(None);
        return;
    }
    let idx = target.max(0) as u64;
    let idx = (idx as usize).min(len - 1);
    state.select(Some(idx));
}

/// The active lens's selection state + its display-order row count. Shared
/// by [`move_selection`] and [`move_selection_to`] — every lens that gets a
/// new fast-scroll key here (`Home`/`End`/`PageUp`/`PageDown`/`g`/`G`) falls
/// out of this single routing table for free.
fn selection_target(app: &mut App) -> (&mut TableState, usize) {
    match app.active_tab {
        Tab::IndexLens => (&mut app.index_table_state, app.index_row_order.len()),
        Tab::ReplicationLens => (
            &mut app.replication_table_state,
            app.replication_row_order.len(),
        ),
        // U3: the Vacuum sub-view keeps its own cursor over its own row set.
        Tab::SchemaLens if app.schema_view == SchemaView::Vacuum => (
            &mut app.vacuum_table_state,
            app.snapshot
                .schema
                .as_deref()
                .map_or(0, |s| s.vacuum_tables.len()),
        ),
        // v0.12: navigates the FILTERED display order (`schema_row_order`),
        // same story as the Micro Lens's `row_order` below — an active
        // `schema_filter` must shrink what j/k/Home/End can reach.
        Tab::SchemaLens => (&mut app.schema_table_state, app.schema_row_order.len()),
        // v0.12: same filtered-display-order story via `statements_row_order`.
        Tab::QueryLens => (
            &mut app.statements_table_state,
            app.statements_row_order.len(),
        ),
        // v0.11: the idle census keeps its own cursor over its own row set.
        Tab::MicroLens if app.micro_view == MicroView::Idle => (
            &mut app.idle_table_state,
            app.snapshot.idle_sessions.as_deref().map_or(0, |v| v.len()),
        ),
        // Micro Lens navigates the FILTERED display order, not the raw
        // snapshot — `table_state` indexes `row_order`.
        _ => (&mut app.table_state, app.row_order.len()),
    }
}

fn move_state(state: &mut TableState, len: usize, delta: i64) {
    if len == 0 {
        state.select(None);
        return;
    }
    let current = state.selected().unwrap_or(0).min(len - 1);
    let next = if delta < 0 {
        current.saturating_sub(delta.unsigned_abs() as usize)
    } else {
        (current + delta as usize).min(len - 1)
    };
    state.select(Some(next));
}

/// Keeps both selections valid after the row sets change size.
fn clamp_selection(app: &mut App) {
    // Micro Lens: clamp against the FILTERED display order, not the raw
    // snapshot (an active filter can shrink it to fewer — or zero — rows).
    let len = app.row_order.len();
    if len == 0 {
        app.table_state.select(None);
        // Nothing to detail anymore (only if this lens's detail was open).
        if app.active_tab == Tab::MicroLens {
            app.detail_open = false;
        }
    } else {
        let clamped = app.table_state.selected().unwrap_or(0).min(len - 1);
        app.table_state.select(Some(clamped));
    }

    // v0.12: clamp against the FILTERED display order, not the raw snapshot
    // — same reasoning as `row_order` above (`schema_filter` can shrink it
    // to fewer, or zero, rows).
    let schema_len = app.schema_row_order.len();
    if schema_len == 0 {
        app.schema_table_state.select(None);
        if app.active_tab == Tab::SchemaLens {
            app.detail_open = false;
        }
    } else {
        let clamped = app
            .schema_table_state
            .selected()
            .unwrap_or(0)
            .min(schema_len - 1);
        app.schema_table_state.select(Some(clamped));
    }

    // Vacuum sub-view (U3): no detail panel to close, just a cursor to keep
    // valid — same shape as the Replication Lens's clamp below.
    let vacuum_len = app
        .snapshot
        .schema
        .as_deref()
        .map_or(0, |s| s.vacuum_tables.len());
    if vacuum_len == 0 {
        app.vacuum_table_state.select(None);
    } else {
        let clamped = app
            .vacuum_table_state
            .selected()
            .unwrap_or(0)
            .min(vacuum_len - 1);
        app.vacuum_table_state.select(Some(clamped));
    }

    let index_len = app.index_row_order.len();
    if index_len == 0 {
        app.index_table_state.select(None);
        if app.active_tab == Tab::IndexLens {
            app.detail_open = false;
        }
    } else {
        let clamped = app
            .index_table_state
            .selected()
            .unwrap_or(0)
            .min(index_len - 1);
        app.index_table_state.select(Some(clamped));
    }

    // Replication Lens has no detail panel (see the Enter keymap), so there
    // is nothing to clear here — only the cursor needs clamping.
    let replication_len = app.replication_row_order.len();
    if replication_len == 0 {
        app.replication_table_state.select(None);
    } else {
        let clamped = app
            .replication_table_state
            .selected()
            .unwrap_or(0)
            .min(replication_len - 1);
        app.replication_table_state.select(Some(clamped));
    }

    // v0.12: same filtered-display-order clamp as `schema_len` above.
    let statements_len = app.statements_row_order.len();
    if statements_len == 0 {
        app.statements_table_state.select(None);
        if app.active_tab == Tab::QueryLens {
            app.detail_open = false;
        }
    } else {
        let clamped = app
            .statements_table_state
            .selected()
            .unwrap_or(0)
            .min(statements_len - 1);
        app.statements_table_state.select(Some(clamped));
    }

    // v0.11: the idle census has no detail panel to clear, just a cursor to
    // keep valid — same shape as the Vacuum sub-view's clamp above.
    let idle_len = app.snapshot.idle_sessions.as_deref().map_or(0, |v| v.len());
    if idle_len == 0 {
        app.idle_table_state.select(None);
    } else {
        let clamped = app
            .idle_table_state
            .selected()
            .unwrap_or(0)
            .min(idle_len - 1);
        app.idle_table_state.select(Some(clamped));
    }
}

/// Case-insensitive substring match of `needle` (already lowercased) against
/// the fields a DBA filters activity by. `pid` matches as text so `/123`
/// finds a backend; every other field is a plain contains.
fn row_matches(row: &pg_lens_core::ActivityRow, needle: &str) -> bool {
    row.pid.to_string().contains(needle)
        || row.database.to_lowercase().contains(needle)
        || row.username.to_lowercase().contains(needle)
        || row.application_name.to_lowercase().contains(needle)
        || row.client.to_lowercase().contains(needle)
        || row.state.to_lowercase().contains(needle)
        || row
            .wait_event
            .as_deref()
            .is_some_and(|w| w.to_lowercase().contains(needle))
        || row.query.to_lowercase().contains(needle)
}

/// Recomputes `row_order` from the current snapshot + filter + sort mode. The
/// view renders rows in this order; the snapshot itself is never mutated.
fn resort(app: &mut App) {
    let rows = &app.snapshot.activity;
    let needle = app.filter.to_lowercase();
    let mut order: Vec<usize> = (0..rows.len())
        .filter(|&i| needle.is_empty() || row_matches(&rows[i], &needle))
        .collect();
    match app.sort_mode {
        SortMode::Duration => order.sort_by(|&a, &b| {
            rows[b]
                .duration_secs
                .total_cmp(&rows[a].duration_secs)
                .then_with(|| rows[a].pid.cmp(&rows[b].pid))
        }),
        SortMode::State => order.sort_by(|&a, &b| {
            rows[a]
                .state
                .cmp(&rows[b].state)
                .then_with(|| rows[a].pid.cmp(&rows[b].pid))
        }),
        SortMode::Pid => order.sort_by_key(|&i| rows[i].pid),
    }
    app.row_order = order;
}

/// Case-insensitive substring match of `needle` (already lowercased) against
/// the Schema Lens Tables view's own filter (`/`) fields — schema name,
/// table name, and the fully-qualified `schema.table` (covers a term that
/// straddles the dot, e.g. "lic.orders"). Mirrors [`row_matches`]'s shape
/// for the Micro Lens.
fn schema_row_matches(row: &pg_lens_core::TableStatRow, needle: &str) -> bool {
    row.schema.to_lowercase().contains(needle)
        || row.name.to_lowercase().contains(needle)
        || format!("{}.{}", row.schema, row.name)
            .to_lowercase()
            .contains(needle)
}

/// Recomputes `schema_row_order` from the current snapshot + filter + schema
/// sort mode (the Schema Lens twin of [`resort`]). Ties break by total size
/// descending, then schema.name ascending, so the order is deterministic.
fn resort_schema(app: &mut App) {
    let Some(schema) = app.snapshot.schema.as_deref() else {
        app.schema_row_order = Vec::new();
        return;
    };
    let rows = &schema.tables;
    let needle = app.schema_filter.to_lowercase();
    let mut order: Vec<usize> = (0..rows.len())
        .filter(|&i| needle.is_empty() || schema_row_matches(&rows[i], &needle))
        .collect();
    let by_size_then_name = |a: usize, b: usize| {
        rows[b]
            .total_bytes
            .cmp(&rows[a].total_bytes)
            .then_with(|| (&rows[a].schema, &rows[a].name).cmp(&(&rows[b].schema, &rows[b].name)))
    };
    match app.schema_sort_mode {
        SchemaSortMode::TotalSize => order.sort_by(|&a, &b| by_size_then_name(a, b)),
        SchemaSortMode::DeadTuples => order.sort_by(|&a, &b| {
            rows[b]
                .n_dead_tup
                .cmp(&rows[a].n_dead_tup)
                .then_with(|| by_size_then_name(a, b))
        }),
        SchemaSortMode::BloatPct => {
            // Descending by estimated bloat%; tables without a usable
            // estimate (is_na / no bloat row) sort last, keyed as -1.0 —
            // valid percentages are always >= 0 after the SQL's clamp.
            let pct = |i: usize| {
                find_table_bloat(schema, &rows[i])
                    .and_then(|b| b.bloat_pct)
                    .unwrap_or(-1.0)
            };
            order.sort_by(|&a, &b| pct(b).total_cmp(&pct(a)).then_with(|| by_size_then_name(a, b)));
        }
        SchemaSortMode::SeqScans => order.sort_by(|&a, &b| {
            rows[b]
                .seq_scan
                .cmp(&rows[a].seq_scan)
                .then_with(|| by_size_then_name(a, b))
        }),
    }
    app.schema_row_order = order;
}

/// Recomputes `index_row_order` from the current snapshot (the Index Lens's
/// twin of [`resort_schema`]). Fixed severity-then-size order (no
/// user-chosen sort — see [`index_finding_rank`]); ties break by
/// schema/table/name ascending so the order is deterministic.
fn resort_indexes(app: &mut App) {
    let Some(schema) = app.snapshot.schema.as_deref() else {
        app.index_row_order = Vec::new();
        return;
    };
    let rows = &schema.indexes;
    let mut order: Vec<usize> = (0..rows.len()).collect();
    order.sort_by(|&a, &b| {
        index_finding_rank(&rows[a].finding)
            .cmp(&index_finding_rank(&rows[b].finding))
            .then_with(|| rows[b].index_bytes.cmp(&rows[a].index_bytes))
            .then_with(|| {
                (&rows[a].schema, &rows[a].table, &rows[a].name)
                    .cmp(&(&rows[b].schema, &rows[b].table, &rows[b].name))
            })
    });
    app.index_row_order = order;
}

/// Recomputes `replication_row_order` from the current snapshot (the
/// Replication Lens's twin of [`resort_indexes`]). Fixed severity-then-
/// retained order (no user-chosen sort — see [`slot_severity_rank`]); ties
/// break by slot name ascending so the order is deterministic.
fn resort_replication(app: &mut App) {
    let Some(slots) = app.snapshot.replication_slots.as_deref() else {
        app.replication_row_order = Vec::new();
        return;
    };
    let mut order: Vec<usize> = (0..slots.len()).collect();
    order.sort_by(|&a, &b| {
        slot_severity_rank(&slots[a])
            .cmp(&slot_severity_rank(&slots[b]))
            .then_with(|| {
                slots[b]
                    .retained_wal_bytes
                    .unwrap_or(0)
                    .cmp(&slots[a].retained_wal_bytes.unwrap_or(0))
            })
            .then_with(|| slots[a].slot_name.cmp(&slots[b].slot_name))
    });
    app.replication_row_order = order;
}

/// Case-insensitive substring match of `needle` (already lowercased) against
/// the Query Lens's own filter (`/`) fields — the normalized query text and,
/// cheaply, the queryid (when present). Mirrors [`row_matches`]'s shape.
fn statements_row_matches(row: &pg_lens_core::StatementRow, needle: &str) -> bool {
    row.query.to_lowercase().contains(needle)
        || row
            .query_id
            .as_deref()
            .is_some_and(|id| id.to_lowercase().contains(needle))
}

/// Recomputes `statements_row_order` from the current snapshot + filter +
/// sort mode (the Query Lens twin of [`resort`]). All modes are descending —
/// the lens answers "what is the heaviest" — with ties broken by calls
/// descending, then query text ascending, so the order is deterministic.
fn resort_statements(app: &mut App) {
    let Some(statements) = app.snapshot.statements.as_deref() else {
        app.statements_row_order = Vec::new();
        return;
    };
    let rows = &statements.statements;
    let needle = app.statements_filter.to_lowercase();
    let mut order: Vec<usize> = (0..rows.len())
        .filter(|&i| needle.is_empty() || statements_row_matches(&rows[i], &needle))
        .collect();
    let tiebreak = |a: usize, b: usize| {
        rows[b]
            .calls
            .cmp(&rows[a].calls)
            .then_with(|| rows[a].query.cmp(&rows[b].query))
    };
    match app.statements_sort_mode {
        StatementsSortMode::TotalTime => order.sort_by(|&a, &b| {
            rows[b]
                .total_exec_ms
                .total_cmp(&rows[a].total_exec_ms)
                .then_with(|| tiebreak(a, b))
        }),
        StatementsSortMode::Calls => order.sort_by(|&a, &b| {
            rows[b]
                .calls
                .cmp(&rows[a].calls)
                .then_with(|| rows[a].query.cmp(&rows[b].query))
        }),
        StatementsSortMode::Mean => order.sort_by(|&a, &b| {
            rows[b]
                .mean_exec_ms
                .total_cmp(&rows[a].mean_exec_ms)
                .then_with(|| tiebreak(a, b))
        }),
        StatementsSortMode::Rows => order.sort_by(|&a, &b| {
            rows[b]
                .rows
                .cmp(&rows[a].rows)
                .then_with(|| tiebreak(a, b))
        }),
    }
    app.statements_row_order = order;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(code: KeyCode) -> Action {
        Action::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn displayed<'a, T>(app: &'a App, field: impl Fn(&'a pg_lens_core::ActivityRow) -> T) -> Vec<T> {
        app.row_order
            .iter()
            .map(|&i| field(&app.snapshot.activity[i]))
            .collect()
    }

    #[test]
    fn q_quits() {
        let mut app = App::new();
        update(&mut app, press(KeyCode::Char('q')));
        assert!(app.should_quit);
    }

    /// Double-Esc barrier: one Esc arms (hint shown), a second inside the
    /// window quits. A hammered Esc closing overlays no longer exits by
    /// accident.
    #[test]
    fn esc_arms_then_second_esc_quits() {
        let mut app = App::new();
        update(&mut app, press(KeyCode::Esc));
        assert!(!app.should_quit, "first Esc must not quit");
        assert!(app.esc_quit_armed_until.is_some());
        let feedback = app.admin_feedback.as_ref().expect("hint shown");
        assert!(feedback.text.contains("Esc again"), "{}", feedback.text);
        update(&mut app, press(KeyCode::Esc));
        assert!(app.should_quit, "second Esc inside the window quits");
    }

    #[test]
    fn esc_barrier_expires_after_the_window() {
        let mut app = App::new();
        update(&mut app, press(KeyCode::Esc));
        assert!(!app.should_quit);
        // Let the window lapse (ticks advance past the armed deadline).
        for _ in 0..=ESC_QUIT_WINDOW_TICKS {
            update(&mut app, Action::Tick);
        }
        update(&mut app, press(KeyCode::Esc));
        assert!(!app.should_quit, "late Esc re-arms instead of quitting");
        update(&mut app, press(KeyCode::Esc));
        assert!(app.should_quit);
    }

    #[test]
    fn esc_closing_the_detail_does_not_arm_quitting() {
        let mut app = App::new();
        app.active_tab = Tab::MicroLens;
        update(&mut app, press(KeyCode::Enter));
        assert!(app.detail_open);
        update(&mut app, press(KeyCode::Esc));
        assert!(!app.detail_open);
        assert!(!app.should_quit);
        assert!(
            app.esc_quit_armed_until.is_none(),
            "closing an overlay must not arm the quit barrier"
        );
    }

    // --- U3: waits panel (`w`) / vacuum sub-view (`v`) ---------------------

    #[test]
    fn w_toggles_the_waits_panel_on_the_micro_lens_only() {
        let mut app = App::new();
        app.active_tab = Tab::MicroLens;
        update(&mut app, press(KeyCode::Char('w')));
        assert!(app.waits_open);
        update(&mut app, press(KeyCode::Char('w')));
        assert!(!app.waits_open);

        // Inert on every other lens.
        for tab in [
            Tab::MacroLens,
            Tab::ReplicationLens,
            Tab::SchemaLens,
            Tab::IndexLens,
            Tab::QueryLens,
        ] {
            let mut app = App::new();
            app.active_tab = tab;
            update(&mut app, press(KeyCode::Char('w')));
            assert!(!app.waits_open, "{tab:?}");
        }
    }

    #[test]
    fn w_and_enter_detail_are_mutually_exclusive_overlays() {
        let mut app = App::new();
        app.active_tab = Tab::MicroLens;
        update(&mut app, press(KeyCode::Enter));
        assert!(app.detail_open);
        // Opening the waits panel closes the detail panel.
        update(&mut app, press(KeyCode::Char('w')));
        assert!(app.waits_open);
        assert!(!app.detail_open);
        // Enter, with the waits panel open, closes IT first (never opens
        // detail underneath — overlays never stack).
        update(&mut app, press(KeyCode::Enter));
        assert!(!app.waits_open);
        assert!(!app.detail_open);
    }

    /// Esc closes the waits panel WITHOUT arming the top-level quit barrier
    /// — the same overlay-dismissal rule the detail panel follows.
    #[test]
    fn esc_closing_the_waits_panel_does_not_arm_quitting() {
        let mut app = App::new();
        app.active_tab = Tab::MicroLens;
        update(&mut app, press(KeyCode::Char('w')));
        assert!(app.waits_open);
        update(&mut app, press(KeyCode::Esc));
        assert!(!app.waits_open);
        assert!(!app.should_quit);
        assert!(
            app.esc_quit_armed_until.is_none(),
            "closing an overlay must not arm the quit barrier"
        );
    }

    // --- v0.9: keyboard help overlay (`?`) ---------------------------------

    #[test]
    fn question_mark_opens_the_help_overlay_from_any_lens() {
        for tab in [
            Tab::MacroLens,
            Tab::MicroLens,
            Tab::ReplicationLens,
            Tab::SchemaLens,
            Tab::IndexLens,
            Tab::QueryLens,
        ] {
            let mut app = App::new();
            app.active_tab = tab;
            update(&mut app, press(KeyCode::Char('?')));
            assert!(app.help_open, "{tab:?}");
        }
    }

    #[test]
    fn esc_closes_help_without_arming_the_quit_barrier() {
        let mut app = App::new();
        update(&mut app, press(KeyCode::Char('?')));
        assert!(app.help_open);
        update(&mut app, press(KeyCode::Esc));
        assert!(!app.help_open);
        assert!(!app.should_quit);
        assert!(
            app.esc_quit_armed_until.is_none(),
            "closing the help overlay must not arm the quit barrier"
        );
    }

    #[test]
    fn question_mark_again_closes_help() {
        let mut app = App::new();
        update(&mut app, press(KeyCode::Char('?')));
        assert!(app.help_open);
        update(&mut app, press(KeyCode::Char('?')));
        assert!(!app.help_open);
    }

    /// Help takes priority over every other overlay's Esc handling: opening
    /// it while the waits panel is up (impossible via real input, since `?`
    /// is only reachable from the bottom match — but proven directly here
    /// for the precedence contract) means Esc closes HELP first.
    #[test]
    fn help_esc_precedence_closes_help_before_other_overlay_state() {
        let mut app = App::new();
        app.active_tab = Tab::MicroLens;
        app.waits_open = true;
        app.help_open = true;
        update(&mut app, press(KeyCode::Esc));
        assert!(!app.help_open, "help must close first");
        assert!(app.waits_open, "the overlay underneath is untouched");
        assert!(!app.should_quit);
    }

    #[test]
    fn q_and_navigation_are_inert_while_help_is_open() {
        let mut app = App::new();
        app.active_tab = Tab::MicroLens;
        let before = app.table_state.selected();
        update(&mut app, press(KeyCode::Char('?')));
        update(&mut app, press(KeyCode::Char('q')));
        assert!(!app.should_quit, "q must be inert while help is open");
        update(&mut app, press(KeyCode::Char('j')));
        assert_eq!(app.table_state.selected(), before);
        assert!(app.help_open);
    }

    #[test]
    fn ctrl_c_still_quits_while_help_is_open() {
        let mut app = App::new();
        update(&mut app, press(KeyCode::Char('?')));
        update(
            &mut app,
            Action::Key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
        );
        assert!(app.should_quit);
    }

    #[test]
    fn tab_switch_closes_the_waits_panel() {
        let mut app = App::new();
        app.active_tab = Tab::MicroLens;
        update(&mut app, press(KeyCode::Char('w')));
        assert!(app.waits_open);
        update(&mut app, press(KeyCode::Tab));
        assert!(!app.waits_open);
    }

    #[test]
    fn v_toggles_the_schema_lens_vacuum_view_only() {
        let mut app = App::new();
        app.active_tab = Tab::SchemaLens;
        assert_eq!(app.schema_view, SchemaView::Tables);
        update(&mut app, press(KeyCode::Char('v')));
        assert_eq!(app.schema_view, SchemaView::Vacuum);
        update(&mut app, press(KeyCode::Char('v')));
        assert_eq!(app.schema_view, SchemaView::Tables);

        // Inert on every other lens.
        for tab in [
            Tab::MacroLens,
            Tab::MicroLens,
            Tab::ReplicationLens,
            Tab::IndexLens,
            Tab::QueryLens,
        ] {
            let mut app = App::new();
            app.active_tab = tab;
            update(&mut app, press(KeyCode::Char('v')));
            assert_eq!(app.schema_view, SchemaView::Tables, "{tab:?}");
        }
    }

    /// Regression (qa v0.8): Esc in the Vacuum sub-view must CLOSE it (back
    /// to Tables), not fall through and arm the double-Esc quit barrier —
    /// same overlay-close contract as the `w` waits panel and `d` picker.
    #[test]
    fn esc_closes_the_vacuum_view_without_arming_quit() {
        let mut app = App::new();
        app.active_tab = Tab::SchemaLens;
        update(&mut app, press(KeyCode::Char('v')));
        assert_eq!(app.schema_view, SchemaView::Vacuum);
        update(&mut app, press(KeyCode::Esc));
        assert_eq!(app.schema_view, SchemaView::Tables, "Esc returns to Tables");
        assert!(!app.should_quit);
        assert!(
            app.esc_quit_armed_until.is_none(),
            "closing the sub-view must not arm the quit barrier"
        );
    }

    #[test]
    fn v_closes_the_tables_view_detail_panel() {
        let mut app = App::new();
        app.active_tab = Tab::SchemaLens;
        update(&mut app, press(KeyCode::Enter));
        assert!(app.detail_open);
        update(&mut app, press(KeyCode::Char('v')));
        assert_eq!(app.schema_view, SchemaView::Vacuum);
        assert!(!app.detail_open);
    }

    /// The Vacuum sub-view has no detail panel of its own — Enter is inert
    /// there (mirrors the Replication Lens's "no detail panel" contract).
    #[test]
    fn enter_is_inert_in_the_vacuum_view() {
        let mut app = App::new();
        app.active_tab = Tab::SchemaLens;
        update(&mut app, press(KeyCode::Char('v')));
        update(&mut app, press(KeyCode::Enter));
        assert!(!app.detail_open);
    }

    /// `s` (sort) is inert in the Vacuum sub-view (fixed worst-first order),
    /// but still cycles the Tables view's own sort mode once toggled back.
    #[test]
    fn s_is_inert_in_the_vacuum_view() {
        let mut app = App::new();
        app.active_tab = Tab::SchemaLens;
        update(&mut app, press(KeyCode::Char('v')));
        let before = app.schema_sort_mode;
        update(&mut app, press(KeyCode::Char('s')));
        assert_eq!(app.schema_sort_mode, before, "sort mode must not change");
    }

    /// The Vacuum sub-view's cursor is independent from the Tables cursor —
    /// `j`/`k` there move `vacuum_table_state`, never `schema_table_state`.
    #[test]
    fn vacuum_view_scrolls_its_own_independent_cursor() {
        let mut app = App::new();
        app.active_tab = Tab::SchemaLens;
        update(&mut app, press(KeyCode::Char('j')));
        assert_eq!(app.schema_table_state.selected(), Some(1));

        update(&mut app, press(KeyCode::Char('v')));
        assert_eq!(app.vacuum_table_state.selected(), Some(0));
        update(&mut app, press(KeyCode::Char('j')));
        assert_eq!(app.vacuum_table_state.selected(), Some(1));
        // Tables cursor untouched by Vacuum-view navigation.
        assert_eq!(app.schema_table_state.selected(), Some(1));
    }

    // --- v0.11: idle connection / connection-age census (`I`) -------------

    #[test]
    fn i_toggles_the_micro_lens_idle_view_only() {
        let mut app = App::new();
        app.active_tab = Tab::MicroLens;
        assert_eq!(app.micro_view, MicroView::Activity);
        update(&mut app, press(KeyCode::Char('I')));
        assert_eq!(app.micro_view, MicroView::Idle);
        update(&mut app, press(KeyCode::Char('I')));
        assert_eq!(app.micro_view, MicroView::Activity);

        // Inert on every other lens.
        for tab in [
            Tab::MacroLens,
            Tab::SchemaLens,
            Tab::ReplicationLens,
            Tab::IndexLens,
            Tab::QueryLens,
        ] {
            let mut app = App::new();
            app.active_tab = tab;
            update(&mut app, press(KeyCode::Char('I')));
            assert_eq!(app.micro_view, MicroView::Activity, "{tab:?}");
        }
    }

    /// Esc in the idle census must CLOSE it (back to Activity), not fall
    /// through and arm the double-Esc quit barrier — same overlay-close
    /// contract as the Vacuum sub-view / waits panel / db picker.
    #[test]
    fn esc_closes_the_idle_view_without_arming_quit() {
        let mut app = App::new();
        app.active_tab = Tab::MicroLens;
        update(&mut app, press(KeyCode::Char('I')));
        assert_eq!(app.micro_view, MicroView::Idle);
        update(&mut app, press(KeyCode::Esc));
        assert_eq!(app.micro_view, MicroView::Activity, "Esc returns to Activity");
        assert!(!app.should_quit);
        assert!(
            app.esc_quit_armed_until.is_none(),
            "closing the idle view must not arm the quit barrier"
        );
    }

    #[test]
    fn i_closes_the_activity_view_detail_panel() {
        let mut app = App::new();
        app.active_tab = Tab::MicroLens;
        update(&mut app, press(KeyCode::Enter));
        assert!(app.detail_open);
        update(&mut app, press(KeyCode::Char('I')));
        assert_eq!(app.micro_view, MicroView::Idle);
        assert!(!app.detail_open);
    }

    /// The idle census has no detail panel of its own — Enter is inert
    /// there, mirroring the Vacuum sub-view's contract.
    #[test]
    fn enter_is_inert_in_the_idle_view() {
        let mut app = App::new();
        app.active_tab = Tab::MicroLens;
        update(&mut app, press(KeyCode::Char('I')));
        update(&mut app, press(KeyCode::Enter));
        assert!(!app.detail_open);
    }

    /// `s` (sort) is inert in the idle census (fixed oldest-first order),
    /// but still cycles the Activity view's own sort mode once toggled back.
    #[test]
    fn s_is_inert_in_the_idle_view() {
        let mut app = App::new();
        app.active_tab = Tab::MicroLens;
        update(&mut app, press(KeyCode::Char('I')));
        let before = app.sort_mode;
        update(&mut app, press(KeyCode::Char('s')));
        assert_eq!(app.sort_mode, before, "sort mode must not change");
    }

    /// The idle census's cursor is independent from the Activity cursor —
    /// `j`/`k` there move `idle_table_state`, never `table_state`.
    #[test]
    fn idle_view_scrolls_its_own_independent_cursor() {
        let mut app = App::new();
        app.active_tab = Tab::MicroLens;
        update(&mut app, press(KeyCode::Char('j')));
        assert_eq!(app.table_state.selected(), Some(1));

        update(&mut app, press(KeyCode::Char('I')));
        assert_eq!(app.idle_table_state.selected(), Some(0));
        update(&mut app, press(KeyCode::Char('j')));
        assert_eq!(app.idle_table_state.selected(), Some(1));
        // Activity cursor untouched by idle-view navigation.
        assert_eq!(app.table_state.selected(), Some(1));
    }

    /// `c`/`K` (admin actions) read the Activity cursor, not the idle one —
    /// both stay inert while the idle census is showing (see `open_confirm`).
    #[test]
    fn admin_actions_are_inert_in_the_idle_view() {
        let mut app = App::new();
        app.active_tab = Tab::MicroLens;
        update(&mut app, press(KeyCode::Char('I')));
        update(&mut app, press(KeyCode::Char('c')));
        assert!(app.confirm.is_none());
        update(&mut app, press(KeyCode::Char('K')));
        assert!(app.confirm.is_none());
    }

    #[test]
    fn tab_cycles_the_six_lenses() {
        let mut app = App::new();
        assert_eq!(app.active_tab, Tab::MacroLens);
        update(&mut app, press(KeyCode::Tab));
        assert_eq!(app.active_tab, Tab::MicroLens);
        update(&mut app, press(KeyCode::Tab));
        assert_eq!(app.active_tab, Tab::ReplicationLens);
        update(&mut app, press(KeyCode::Tab));
        assert_eq!(app.active_tab, Tab::SchemaLens);
        update(&mut app, press(KeyCode::Tab));
        assert_eq!(app.active_tab, Tab::IndexLens);
        update(&mut app, press(KeyCode::Tab));
        assert_eq!(app.active_tab, Tab::QueryLens);
        update(&mut app, press(KeyCode::Tab));
        assert_eq!(app.active_tab, Tab::MacroLens);
        assert!(!app.should_quit);
    }

    // --- v0.12: navigation & scroll polish ----------------------------------

    #[test]
    fn back_tab_cycles_the_six_lenses_backward() {
        let mut app = App::new();
        assert_eq!(app.active_tab, Tab::MacroLens);
        update(&mut app, press(KeyCode::BackTab));
        assert_eq!(app.active_tab, Tab::QueryLens);
        update(&mut app, press(KeyCode::BackTab));
        assert_eq!(app.active_tab, Tab::IndexLens);
        update(&mut app, press(KeyCode::BackTab));
        assert_eq!(app.active_tab, Tab::SchemaLens);
        update(&mut app, press(KeyCode::BackTab));
        assert_eq!(app.active_tab, Tab::ReplicationLens);
        update(&mut app, press(KeyCode::BackTab));
        assert_eq!(app.active_tab, Tab::MicroLens);
        update(&mut app, press(KeyCode::BackTab));
        assert_eq!(app.active_tab, Tab::MacroLens);
        assert!(!app.should_quit);
    }

    #[test]
    fn digit_keys_jump_directly_to_the_matching_tab() {
        let mut app = App::new();
        for (digit, tab) in [
            ('1', Tab::MacroLens),
            ('2', Tab::MicroLens),
            ('3', Tab::ReplicationLens),
            ('4', Tab::SchemaLens),
            ('5', Tab::IndexLens),
            ('6', Tab::QueryLens),
        ] {
            update(&mut app, press(KeyCode::Char(digit)));
            assert_eq!(app.active_tab, tab, "digit {digit}");
        }
    }

    /// Digit jumps close the same transient overlays as `Tab`/`BackTab`.
    #[test]
    fn digit_jump_closes_detail_and_waits_overlays() {
        let mut app = App::new();
        app.active_tab = Tab::MicroLens;
        update(&mut app, press(KeyCode::Enter));
        assert!(app.detail_open);
        update(&mut app, press(KeyCode::Char('4')));
        assert_eq!(app.active_tab, Tab::SchemaLens);
        assert!(!app.detail_open);
    }

    /// Digit keys must not hijack a digit typed into the filter editor.
    #[test]
    fn digit_keys_are_inert_while_filter_editing() {
        let mut app = App::new();
        app.active_tab = Tab::MicroLens;
        update(&mut app, press(KeyCode::Char('/')));
        assert!(app.filter_editing);
        update(&mut app, press(KeyCode::Char('4')));
        assert_eq!(app.active_tab, Tab::MicroLens, "digit must stay in the filter text");
        assert_eq!(app.filter, "4");
    }

    /// Digit keys must not hijack a confirm-modal keystroke either (the
    /// modal only recognizes y/n/Esc — this proves `4` cannot slip through).
    #[test]
    fn digit_keys_are_inert_while_the_confirm_modal_is_open() {
        let mut app = App::new();
        app.active_tab = Tab::MicroLens;
        update(&mut app, press(KeyCode::Char('c')));
        assert!(app.confirm.is_some());
        update(&mut app, press(KeyCode::Char('4')));
        assert_eq!(app.active_tab, Tab::MicroLens);
        assert!(app.confirm.is_some());
    }

    #[test]
    fn backspace_swaps_to_the_previous_tab_and_toggles_back() {
        let mut app = App::new();
        assert!(app.previous_tab.is_none());
        // No history yet: harmless no-op.
        update(&mut app, press(KeyCode::Backspace));
        assert_eq!(app.active_tab, Tab::MacroLens);

        update(&mut app, press(KeyCode::Char('5'))); // → Index Lens
        assert_eq!(app.active_tab, Tab::IndexLens);
        update(&mut app, press(KeyCode::Backspace)); // → back to Macro Lens
        assert_eq!(app.active_tab, Tab::MacroLens);
        update(&mut app, press(KeyCode::Backspace)); // toggles right back
        assert_eq!(app.active_tab, Tab::IndexLens);
    }

    /// Backspace must stay a delete key inside the filter editor, never a
    /// go-back — `handle_filter_key` intercepts it before the top-level
    /// dispatch ever sees it.
    #[test]
    fn backspace_deletes_in_the_filter_editor_not_go_back() {
        let mut app = App::new();
        app.active_tab = Tab::MicroLens;
        update(&mut app, press(KeyCode::Char('/')));
        type_str(&mut app, "abc");
        assert_eq!(app.filter, "abc");
        update(&mut app, press(KeyCode::Backspace));
        assert_eq!(app.filter, "ab");
        assert_eq!(app.active_tab, Tab::MicroLens, "must not have navigated");
    }

    fn type_str(app: &mut App, s: &str) {
        for c in s.chars() {
            update(app, press(KeyCode::Char(c)));
        }
    }

    #[test]
    fn slash_filters_activity_live_and_moves_cursor_within_matches() {
        let mut app = App::new();
        app.active_tab = Tab::MicroLens;
        let total = app.snapshot.activity.len();

        update(&mut app, press(KeyCode::Char('/')));
        assert!(app.filter_editing);
        type_str(&mut app, "bench");

        // Every visible row matches the needle somewhere, and there are fewer
        // than the full set (the mock has non-bench rows).
        assert!(!app.row_order.is_empty());
        assert!(app.row_order.len() < total);
        let needle = "bench";
        for &i in &app.row_order {
            let r = &app.snapshot.activity[i];
            let hay = format!(
                "{} {} {} {} {}",
                r.pid, r.application_name, r.database, r.username, r.query
            )
            .to_lowercase();
            assert!(hay.contains(needle), "row {i} does not match: {hay}");
        }
        // Commit, then navigate: the cursor cannot point past the filtered
        // set (j/k walk `row_order`, not the raw snapshot).
        update(&mut app, press(KeyCode::Enter));
        for _ in 0..total + 5 {
            update(&mut app, press(KeyCode::Char('j')));
        }
        assert!(app.table_state.selected().unwrap() < app.row_order.len());
    }

    #[test]
    fn filter_enter_commits_esc_reverts() {
        let mut app = App::new();
        app.active_tab = Tab::MicroLens;
        let total = app.snapshot.activity.len();

        // Commit a filter with Enter: editing stops, text and narrowing stay.
        update(&mut app, press(KeyCode::Char('/')));
        type_str(&mut app, "shop");
        update(&mut app, press(KeyCode::Enter));
        assert!(!app.filter_editing);
        assert_eq!(app.filter, "shop");
        let narrowed = app.row_order.len();
        assert!(narrowed < total);

        // Re-enter, type more, then Esc: reverts to the committed "shop".
        update(&mut app, press(KeyCode::Char('/')));
        type_str(&mut app, "xyz");
        assert!(app.row_order.is_empty()); // "shopxyz" matches nothing
        update(&mut app, press(KeyCode::Esc));
        assert!(!app.filter_editing);
        assert_eq!(app.filter, "shop");
        assert_eq!(app.row_order.len(), narrowed);
    }

    #[test]
    fn backspace_widens_the_filter() {
        let mut app = App::new();
        app.active_tab = Tab::MicroLens;
        let total = app.snapshot.activity.len();
        update(&mut app, press(KeyCode::Char('/')));
        type_str(&mut app, "bench");
        let narrowed = app.row_order.len();
        for _ in 0..5 {
            update(&mut app, press(KeyCode::Backspace));
        }
        assert_eq!(app.filter, "");
        assert_eq!(app.row_order.len(), total);
        assert!(narrowed < total);
    }

    #[test]
    fn slash_is_inert_off_the_micro_lens() {
        let mut app = App::new();
        app.active_tab = Tab::MacroLens;
        update(&mut app, press(KeyCode::Char('/')));
        assert!(!app.filter_editing);
    }

    #[test]
    fn ctrl_c_quits_even_while_filtering() {
        let mut app = App::new();
        app.active_tab = Tab::MicroLens;
        update(&mut app, press(KeyCode::Char('/')));
        update(
            &mut app,
            Action::Key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
        );
        assert!(app.should_quit);
    }

    // --- v0.12: Schema Lens Tables-view filter ------------------------------

    #[test]
    fn slash_arms_the_schema_filter_only_on_the_tables_view() {
        let mut app = App::new();
        app.active_tab = Tab::SchemaLens;
        assert_eq!(app.schema_view, SchemaView::Tables);
        update(&mut app, press(KeyCode::Char('/')));
        assert!(app.schema_filter_editing);
        // The Micro Lens's own filter must stay untouched.
        assert!(!app.filter_editing);

        // Vacuum sub-view: `/` is inert (mirrors the Micro Lens's idle
        // census having no filter of its own).
        let mut app = App::new();
        app.active_tab = Tab::SchemaLens;
        app.schema_view = SchemaView::Vacuum;
        update(&mut app, press(KeyCode::Char('/')));
        assert!(!app.schema_filter_editing);
    }

    #[test]
    fn slash_filters_the_schema_tables_view_live_and_narrows_the_count() {
        let mut app = App::new();
        app.active_tab = Tab::SchemaLens;
        let total = app
            .snapshot
            .schema
            .as_deref()
            .expect("mock schema")
            .tables
            .len();

        update(&mut app, press(KeyCode::Char('/')));
        type_str(&mut app, "order");
        assert!(!app.schema_row_order.is_empty());
        assert!(app.schema_row_order.len() < total);
        let schema = app.snapshot.schema.as_deref().expect("mock schema");
        for &i in &app.schema_row_order {
            let t = &schema.tables[i];
            let hay = format!("{}.{}", t.schema, t.name).to_lowercase();
            assert!(hay.contains("order"), "row {i} does not match: {hay}");
        }

        // Commit, then navigate: the cursor cannot walk past the filtered
        // set (mirrors the Micro Lens's cursor-clamp contract).
        update(&mut app, press(KeyCode::Enter));
        assert!(!app.schema_filter_editing);
        for _ in 0..total + 5 {
            update(&mut app, press(KeyCode::Char('j')));
        }
        assert!(app.schema_table_state.selected().unwrap() < app.schema_row_order.len());
    }

    #[test]
    fn schema_filter_esc_reverts_to_the_committed_value() {
        let mut app = App::new();
        app.active_tab = Tab::SchemaLens;
        update(&mut app, press(KeyCode::Char('/')));
        type_str(&mut app, "order");
        update(&mut app, press(KeyCode::Enter));
        let narrowed = app.schema_row_order.len();

        update(&mut app, press(KeyCode::Char('/')));
        type_str(&mut app, "zzz");
        assert!(app.schema_row_order.is_empty());
        update(&mut app, press(KeyCode::Esc));
        assert!(!app.schema_filter_editing);
        assert_eq!(app.schema_filter, "order");
        assert_eq!(app.schema_row_order.len(), narrowed);
    }

    // --- v0.12: Query Lens filter -------------------------------------------

    #[test]
    fn slash_arms_the_query_lens_filter() {
        let mut app = App::new();
        app.active_tab = Tab::QueryLens;
        update(&mut app, press(KeyCode::Char('/')));
        assert!(app.statements_filter_editing);
        assert!(!app.filter_editing);
        assert!(!app.schema_filter_editing);
    }

    #[test]
    fn slash_filters_the_query_lens_live_and_narrows_the_count() {
        let mut app = App::new();
        app.active_tab = Tab::QueryLens;
        let total = app
            .snapshot
            .statements
            .as_deref()
            .expect("mock statements")
            .statements
            .len();

        update(&mut app, press(KeyCode::Char('/')));
        type_str(&mut app, "pgbench_accounts");
        assert!(!app.statements_row_order.is_empty());
        assert!(app.statements_row_order.len() < total);
        let statements = app.snapshot.statements.as_deref().expect("mock statements");
        for &i in &app.statements_row_order {
            let hay = statements.statements[i].query.to_lowercase();
            assert!(hay.contains("pgbench_accounts"), "row {i}: {hay}");
        }
    }

    // --- v0.12: one-key clear-filter (`\`) -----------------------------------

    #[test]
    fn backslash_clears_the_committed_filter_of_the_active_lens_only() {
        // Micro Lens.
        let mut app = App::new();
        app.active_tab = Tab::MicroLens;
        update(&mut app, press(KeyCode::Char('/')));
        type_str(&mut app, "bench");
        update(&mut app, press(KeyCode::Enter));
        assert_eq!(app.filter, "bench");
        update(&mut app, press(KeyCode::Char('\\')));
        assert_eq!(app.filter, "");
        assert_eq!(app.row_order.len(), app.snapshot.activity.len());

        // Schema Lens Tables view.
        let mut app = App::new();
        app.active_tab = Tab::SchemaLens;
        update(&mut app, press(KeyCode::Char('/')));
        type_str(&mut app, "order");
        update(&mut app, press(KeyCode::Enter));
        assert_eq!(app.schema_filter, "order");
        update(&mut app, press(KeyCode::Char('\\')));
        assert_eq!(app.schema_filter, "");
        let total = app
            .snapshot
            .schema
            .as_deref()
            .expect("mock schema")
            .tables
            .len();
        assert_eq!(app.schema_row_order.len(), total);

        // Query Lens.
        let mut app = App::new();
        app.active_tab = Tab::QueryLens;
        update(&mut app, press(KeyCode::Char('/')));
        type_str(&mut app, "pgbench_accounts");
        update(&mut app, press(KeyCode::Enter));
        assert_eq!(app.statements_filter, "pgbench_accounts");
        update(&mut app, press(KeyCode::Char('\\')));
        assert_eq!(app.statements_filter, "");
        let total = app
            .snapshot
            .statements
            .as_deref()
            .expect("mock statements")
            .statements
            .len();
        assert_eq!(app.statements_row_order.len(), total);
    }

    #[test]
    fn backslash_is_inert_with_no_active_filter_and_never_arms_quitting() {
        let mut app = App::new();
        app.active_tab = Tab::MicroLens;
        assert_eq!(app.filter, "");
        update(&mut app, press(KeyCode::Char('\\')));
        assert_eq!(app.filter, "");
        assert!(!app.should_quit);
        assert!(app.esc_quit_armed_until.is_none());
    }

    /// The key correctness point of v0.12's per-lens design: typing on one
    /// lens's filter must never leak into another's — clearing one must
    /// never touch the others either.
    #[test]
    fn filters_never_cross_contaminate_across_lenses() {
        let mut app = App::new();

        app.active_tab = Tab::MicroLens;
        update(&mut app, press(KeyCode::Char('/')));
        type_str(&mut app, "micro-term");
        update(&mut app, press(KeyCode::Enter));

        app.active_tab = Tab::SchemaLens;
        update(&mut app, press(KeyCode::Char('/')));
        type_str(&mut app, "schema-term");
        update(&mut app, press(KeyCode::Enter));

        app.active_tab = Tab::QueryLens;
        update(&mut app, press(KeyCode::Char('/')));
        type_str(&mut app, "query-term");
        update(&mut app, press(KeyCode::Enter));

        // Each lens kept exactly its own term.
        assert_eq!(app.filter, "micro-term");
        assert_eq!(app.schema_filter, "schema-term");
        assert_eq!(app.statements_filter, "query-term");

        // Clearing the Query Lens's filter leaves the other two untouched.
        update(&mut app, press(KeyCode::Char('\\')));
        assert_eq!(app.statements_filter, "");
        assert_eq!(app.filter, "micro-term");
        assert_eq!(app.schema_filter, "schema-term");

        // Switching tabs never rewrites another lens's saved term either.
        app.active_tab = Tab::MicroLens;
        assert_eq!(app.filter, "micro-term");
        app.active_tab = Tab::SchemaLens;
        assert_eq!(app.schema_filter, "schema-term");
    }

    #[test]
    fn navigation_saturates_at_both_ends() {
        let mut app = App::new();
        let last = app.snapshot.activity.len() - 1;
        assert_eq!(app.table_state.selected(), Some(0));

        // Up at the top stays at the top.
        update(&mut app, press(KeyCode::Char('k')));
        assert_eq!(app.table_state.selected(), Some(0));
        update(&mut app, press(KeyCode::Up));
        assert_eq!(app.table_state.selected(), Some(0));

        // Down walks to the last row and saturates there.
        for _ in 0..app.snapshot.activity.len() + 3 {
            update(&mut app, press(KeyCode::Char('j')));
        }
        assert_eq!(app.table_state.selected(), Some(last));
        update(&mut app, press(KeyCode::Down));
        assert_eq!(app.table_state.selected(), Some(last));

        // And back up one.
        update(&mut app, press(KeyCode::Up));
        assert_eq!(app.table_state.selected(), Some(last - 1));
    }

    /// `Home`/`g` jump to the first row, `End`/`G` to the last, from
    /// anywhere in the middle — on the Micro Lens's activity table.
    #[test]
    fn home_end_and_g_shift_g_jump_to_the_first_and_last_row() {
        let mut app = App::new();
        let last = app.row_order.len() - 1;
        assert!(last > 0, "mock must carry more than one activity row");

        update(&mut app, press(KeyCode::End));
        assert_eq!(app.table_state.selected(), Some(last));
        update(&mut app, press(KeyCode::Home));
        assert_eq!(app.table_state.selected(), Some(0));

        update(&mut app, press(KeyCode::Char('G')));
        assert_eq!(app.table_state.selected(), Some(last));
        update(&mut app, press(KeyCode::Char('g')));
        assert_eq!(app.table_state.selected(), Some(0));
    }

    #[test]
    fn page_up_and_page_down_move_by_a_page_and_clamp() {
        let mut app = App::new();
        let last = app.row_order.len() - 1;

        update(&mut app, press(KeyCode::PageDown));
        assert_eq!(app.table_state.selected(), Some((PAGE_SIZE as usize).min(last)));

        // From the top, PageUp clamps at 0 rather than underflowing.
        let mut app = App::new();
        update(&mut app, press(KeyCode::PageUp));
        assert_eq!(app.table_state.selected(), Some(0));

        // From the bottom, PageDown clamps at the last row.
        update(&mut app, press(KeyCode::End));
        update(&mut app, press(KeyCode::PageDown));
        assert_eq!(app.table_state.selected(), Some(last));
    }

    /// The fast-scroll keys route through the SAME per-lens (state, len)
    /// table as `j`/`k` — this proves it works on a lens other than the
    /// Micro Lens (the Schema Lens's own cursor), not just the default arm.
    #[test]
    fn fast_scroll_works_on_the_schema_lens_table_too() {
        let mut app = App::new();
        app.active_tab = Tab::SchemaLens;
        let last = app.schema_row_order.len() - 1;
        assert!(last > 0, "mock must carry more than one schema table row");

        update(&mut app, press(KeyCode::Char('G')));
        assert_eq!(app.schema_table_state.selected(), Some(last));
        update(&mut app, press(KeyCode::Char('g')));
        assert_eq!(app.schema_table_state.selected(), Some(0));
    }

    /// `g` must not hijack typing inside the filter editor.
    #[test]
    fn g_and_shift_g_are_inert_while_filter_editing() {
        let mut app = App::new();
        app.active_tab = Tab::MicroLens;
        update(&mut app, press(KeyCode::Char('/')));
        type_str(&mut app, "g");
        assert_eq!(app.filter, "g", "the letter must land in the filter text");
    }

    #[test]
    fn sort_cycles_and_reorders_rows() {
        let mut app = App::new();

        // Default: duration, longest first.
        assert_eq!(app.sort_mode, SortMode::Duration);
        let durations = displayed(&app, |r| r.duration_secs);
        assert!(durations.windows(2).all(|w| w[0] >= w[1]));

        // s → state (alphabetical).
        update(&mut app, press(KeyCode::Char('s')));
        assert_eq!(app.sort_mode, SortMode::State);
        let states = displayed(&app, |r| r.state.clone());
        assert!(states.windows(2).all(|w| w[0] <= w[1]));

        // s → pid (ascending).
        update(&mut app, press(KeyCode::Char('s')));
        assert_eq!(app.sort_mode, SortMode::Pid);
        let pids = displayed(&app, |r| r.pid);
        assert!(pids.windows(2).all(|w| w[0] < w[1]));

        // s → back to duration.
        update(&mut app, press(KeyCode::Char('s')));
        assert_eq!(app.sort_mode, SortMode::Duration);

        // Every mode shows every row exactly once.
        let mut seen = app.row_order.clone();
        seen.sort_unstable();
        assert_eq!(seen, (0..app.snapshot.activity.len()).collect::<Vec<_>>());
    }

    #[test]
    fn enter_opens_and_closes_detail_on_micro_lens_only() {
        let mut app = App::new();

        // Macro Lens: Enter is a no-op.
        update(&mut app, press(KeyCode::Enter));
        assert!(!app.detail_open);

        // Micro Lens with a selection: Enter opens, Enter closes.
        update(&mut app, press(KeyCode::Tab));
        update(&mut app, press(KeyCode::Enter));
        assert!(app.detail_open);
        update(&mut app, press(KeyCode::Enter));
        assert!(!app.detail_open);
    }

    #[test]
    fn esc_closes_detail_before_quitting() {
        let mut app = App::new();
        update(&mut app, press(KeyCode::Tab));
        update(&mut app, press(KeyCode::Enter));
        assert!(app.detail_open);

        // First Esc only closes the panel...
        update(&mut app, press(KeyCode::Esc));
        assert!(!app.detail_open);
        assert!(!app.should_quit);

        // ...the second ARMS the quit barrier (double-Esc rule), and only
        // the third — inside the window — actually quits.
        update(&mut app, press(KeyCode::Esc));
        assert!(!app.should_quit);
        update(&mut app, press(KeyCode::Esc));
        assert!(app.should_quit);
    }

    #[test]
    fn tab_and_navigation_behave_while_detail_is_open() {
        let mut app = App::new();
        update(&mut app, press(KeyCode::Tab)); // → Micro Lens
        update(&mut app, press(KeyCode::Enter));
        assert!(app.detail_open);

        // j moves the selection (the panel follows the cursor).
        let before = app.selected_row().expect("selection").pid;
        update(&mut app, press(KeyCode::Char('j')));
        assert!(app.detail_open);
        assert_ne!(app.selected_row().expect("selection").pid, before);

        // Tab closes the panel and switches lens.
        update(&mut app, press(KeyCode::Tab));
        assert!(!app.detail_open);
        assert_eq!(app.active_tab, Tab::ReplicationLens);
    }

    /// Rows of the Schema Lens in display order, projected by `field`.
    fn schema_displayed<'a, T>(
        app: &'a App,
        field: impl Fn(&'a pg_lens_core::TableStatRow) -> T,
    ) -> Vec<T> {
        let schema = app.snapshot.schema.as_deref().expect("mock has schema");
        app.schema_row_order
            .iter()
            .map(|&i| field(&schema.tables[i]))
            .collect()
    }

    #[test]
    fn schema_sort_cycles_and_reorders_rows() {
        let mut app = App::new();
        for _ in 0..3 {
            update(&mut app, press(KeyCode::Tab));
        }
        assert_eq!(app.active_tab, Tab::SchemaLens);

        // Default: total size descending (mock's biggest: pgbench_accounts).
        assert_eq!(app.schema_sort_mode, SchemaSortMode::TotalSize);
        let sizes = schema_displayed(&app, |t| t.total_bytes);
        assert!(sizes.windows(2).all(|w| w[0] >= w[1]));
        assert_eq!(
            schema_displayed(&app, |t| t.name.clone())[0],
            "pgbench_accounts"
        );

        // s → dead tuples descending (mock's bloated one: order_items).
        update(&mut app, press(KeyCode::Char('s')));
        assert_eq!(app.schema_sort_mode, SchemaSortMode::DeadTuples);
        let dead = schema_displayed(&app, |t| t.n_dead_tup);
        assert!(dead.windows(2).all(|w| w[0] >= w[1]));
        assert_eq!(schema_displayed(&app, |t| t.name.clone())[0], "order_items");

        // s → bloat% descending, tables without a usable estimate LAST
        // (mock: audit.raw_events is is_na; pgbench_branches has no row).
        update(&mut app, press(KeyCode::Char('s')));
        assert_eq!(app.schema_sort_mode, SchemaSortMode::BloatPct);
        let names = schema_displayed(&app, |t| t.name.clone());
        assert_eq!(names[0], "order_items", "highest estimated bloat first");
        let no_estimate_from = names
            .iter()
            .position(|n| n == "pgbench_branches" || n == "raw_events")
            .expect("estimate-less tables present");
        assert!(
            names[no_estimate_from..]
                .iter()
                .all(|n| n == "pgbench_branches" || n == "raw_events"),
            "None estimates must sort last: {names:?}"
        );

        // s → seq scans descending (mock's hot one: pgbench_branches).
        update(&mut app, press(KeyCode::Char('s')));
        assert_eq!(app.schema_sort_mode, SchemaSortMode::SeqScans);
        let seqs = schema_displayed(&app, |t| t.seq_scan);
        assert!(seqs.windows(2).all(|w| w[0] >= w[1]));

        // s → back to size; the Micro Lens sort was never touched.
        update(&mut app, press(KeyCode::Char('s')));
        assert_eq!(app.schema_sort_mode, SchemaSortMode::TotalSize);
        assert_eq!(app.sort_mode, SortMode::Duration);

        // Every mode shows every table exactly once.
        let mut seen = app.schema_row_order.clone();
        seen.sort_unstable();
        let table_count = app.snapshot.schema.as_deref().expect("schema").tables.len();
        assert_eq!(seen, (0..table_count).collect::<Vec<_>>());
    }

    #[test]
    fn schema_lens_has_its_own_selection_and_detail() {
        let mut app = App::new();
        for _ in 0..3 {
            update(&mut app, press(KeyCode::Tab));
        }
        assert_eq!(app.active_tab, Tab::SchemaLens);

        // j moves the SCHEMA selection, not the activity one.
        assert_eq!(app.schema_table_state.selected(), Some(0));
        update(&mut app, press(KeyCode::Char('j')));
        assert_eq!(app.schema_table_state.selected(), Some(1));
        assert_eq!(app.table_state.selected(), Some(0), "micro cursor untouched");

        // Enter opens the table detail; Enter closes it.
        let selected = app.selected_table().expect("selection").name.clone();
        update(&mut app, press(KeyCode::Enter));
        assert!(app.detail_open);
        assert_eq!(app.selected_table().expect("selection").name, selected);
        update(&mut app, press(KeyCode::Enter));
        assert!(!app.detail_open);

        // Esc closes the panel first, quits second (same as Micro).
        update(&mut app, press(KeyCode::Enter));
        update(&mut app, press(KeyCode::Esc));
        assert!(!app.detail_open);
        assert!(!app.should_quit);
    }

    #[test]
    fn uppercase_r_requests_schema_recollection_from_any_lens() {
        let mut app = App::new();
        assert_eq!(app.schema_refresh_requests, 0);

        // Macro Lens: R counts (documented decision: works from any lens).
        update(&mut app, press(KeyCode::Char('R')));
        assert_eq!(app.schema_refresh_requests, 1);

        // Schema Lens: R keeps counting; lowercase r does nothing.
        for _ in 0..3 {
            update(&mut app, press(KeyCode::Tab));
        }
        assert_eq!(app.active_tab, Tab::SchemaLens);
        update(&mut app, press(KeyCode::Char('R')));
        assert_eq!(app.schema_refresh_requests, 2);
        update(&mut app, press(KeyCode::Char('r')));
        assert_eq!(app.schema_refresh_requests, 2);
        assert!(!app.should_quit);
    }

    #[test]
    fn refresh_interval_adjusts_within_bounds() {
        let mut app = App::new();
        assert_eq!(app.refresh_interval, DEFAULT_REFRESH);

        update(&mut app, press(KeyCode::Char('+')));
        assert_eq!(app.refresh_interval, DEFAULT_REFRESH + REFRESH_STEP);

        // '-' repeatedly floors at REFRESH_MIN.
        for _ in 0..50 {
            update(&mut app, press(KeyCode::Char('-')));
        }
        assert_eq!(app.refresh_interval, REFRESH_MIN);

        // '+' repeatedly caps at REFRESH_MAX.
        for _ in 0..50 {
            update(&mut app, press(KeyCode::Char('+')));
        }
        assert_eq!(app.refresh_interval, REFRESH_MAX);
    }

    #[test]
    fn tick_advances_the_spinner_counter_and_nothing_else() {
        let mut app = App::new();
        assert_eq!(app.tick_count, 0);
        update(&mut app, Action::Tick);
        update(&mut app, Action::Tick);
        assert_eq!(app.tick_count, 2);
        assert!(!app.should_quit);
        assert!(app.first_data_at.is_none(), "ticks never count as data");
    }

    #[test]
    fn splash_shows_until_the_first_ok_snapshot_then_never_again() {
        let mut app = App::new();
        // App::new seeds an Ok mock snapshot, but pre-update state in real
        // mode is Connecting: simulate the real pipeline.
        update(
            &mut app,
            Action::Snapshot(Arc::new(DbSnapshot::connecting())),
        );
        assert!(app.show_splash(), "Connecting + no data ever = splash");

        // Error while still pre-first-data: stay on the splash (error box).
        let mut failed = DbSnapshot::connecting();
        failed.status = pg_lens_core::PollerStatus::Error("no pg_hba.conf entry".into());
        update(&mut app, Action::Snapshot(Arc::new(failed)));
        assert!(app.show_splash(), "pre-first-data errors stay on splash");
        assert!(app.first_data_at.is_none());

        // First Ok snapshot: dashboard, permanently.
        update(&mut app, Action::Snapshot(Arc::new(DbSnapshot::mock())));
        assert!(!app.show_splash());
        assert!(app.first_data_at.is_some());

        // A later disconnect does NOT bring the splash back (banner instead).
        let mut lost = DbSnapshot::mock();
        lost.status = pg_lens_core::PollerStatus::Error("connection refused".into());
        update(&mut app, Action::Snapshot(Arc::new(lost)));
        assert!(!app.show_splash(), "post-first-data errors use the banner");
    }

    // --- Query Lens (pg_stat_statements) --------------------------------------

    /// App on the Query Lens (five Tabs from Macro: Micro, Replication,
    /// Schema, Index, Query).
    fn query_lens_app() -> App {
        let mut app = App::new();
        for _ in 0..5 {
            update(&mut app, press(KeyCode::Tab));
        }
        assert_eq!(app.active_tab, Tab::QueryLens);
        app
    }

    /// Rows of the Query Lens in display order, projected by `field`.
    fn statements_displayed<'a, T>(
        app: &'a App,
        field: impl Fn(&'a pg_lens_core::StatementRow) -> T,
    ) -> Vec<T> {
        let statements = app.snapshot.statements.as_deref().expect("mock statements");
        app.statements_row_order
            .iter()
            .map(|&i| field(&statements.statements[i]))
            .collect()
    }

    #[test]
    fn statements_sort_cycles_and_reorders_rows() {
        let mut app = query_lens_app();

        // Default: total execution time descending.
        assert_eq!(app.statements_sort_mode, StatementsSortMode::TotalTime);
        let totals = statements_displayed(&app, |s| s.total_exec_ms);
        assert!(totals.windows(2).all(|w| w[0] >= w[1]));

        // s → calls descending.
        update(&mut app, press(KeyCode::Char('s')));
        assert_eq!(app.statements_sort_mode, StatementsSortMode::Calls);
        let calls = statements_displayed(&app, |s| s.calls);
        assert!(calls.windows(2).all(|w| w[0] >= w[1]));

        // s → mean descending (mock's slowest-per-call: pg_sleep).
        update(&mut app, press(KeyCode::Char('s')));
        assert_eq!(app.statements_sort_mode, StatementsSortMode::Mean);
        let means = statements_displayed(&app, |s| s.mean_exec_ms);
        assert!(means.windows(2).all(|w| w[0] >= w[1]));
        assert!(
            statements_displayed(&app, |s| s.query.clone())[0].contains("pg_sleep"),
            "pg_sleep has the highest mean in the mock"
        );

        // s → rows descending.
        update(&mut app, press(KeyCode::Char('s')));
        assert_eq!(app.statements_sort_mode, StatementsSortMode::Rows);
        let rows = statements_displayed(&app, |s| s.rows);
        assert!(rows.windows(2).all(|w| w[0] >= w[1]));

        // s → back to total; the other lenses' sorts were never touched.
        update(&mut app, press(KeyCode::Char('s')));
        assert_eq!(app.statements_sort_mode, StatementsSortMode::TotalTime);
        assert_eq!(app.sort_mode, SortMode::Duration);
        assert_eq!(app.schema_sort_mode, SchemaSortMode::TotalSize);

        // Every mode shows every statement exactly once.
        let mut seen = app.statements_row_order.clone();
        seen.sort_unstable();
        let n = app
            .snapshot
            .statements
            .as_deref()
            .expect("statements")
            .statements
            .len();
        assert_eq!(seen, (0..n).collect::<Vec<_>>());
    }

    #[test]
    fn query_lens_has_its_own_selection_and_detail() {
        let mut app = query_lens_app();

        // j moves the STATEMENTS selection only.
        assert_eq!(app.statements_table_state.selected(), Some(0));
        update(&mut app, press(KeyCode::Char('j')));
        assert_eq!(app.statements_table_state.selected(), Some(1));
        assert_eq!(app.table_state.selected(), Some(0), "micro cursor untouched");
        assert_eq!(
            app.schema_table_state.selected(),
            Some(0),
            "schema cursor untouched"
        );

        // Enter opens the statement detail; Enter closes it.
        let selected = app
            .selected_statement()
            .expect("selection")
            .query
            .clone();
        update(&mut app, press(KeyCode::Enter));
        assert!(app.detail_open);
        assert_eq!(app.selected_statement().expect("selection").query, selected);
        update(&mut app, press(KeyCode::Enter));
        assert!(!app.detail_open);

        // Esc closes the panel first, quits second (same as the others).
        update(&mut app, press(KeyCode::Enter));
        update(&mut app, press(KeyCode::Esc));
        assert!(!app.detail_open);
        assert!(!app.should_quit);
    }

    #[test]
    fn query_lens_without_statements_has_no_selection_or_detail() {
        let mut app = query_lens_app();
        let mut snap = app.snapshot.as_ref().clone();
        snap.statements = None;
        update(&mut app, Action::Snapshot(Arc::new(snap)));
        assert!(app.statements_row_order.is_empty());
        assert_eq!(app.statements_table_state.selected(), None);
        update(&mut app, press(KeyCode::Enter));
        assert!(!app.detail_open, "no data, nothing to detail");
        update(&mut app, press(KeyCode::Char('j'))); // must not panic
    }

    // --- startup service picker ---------------------------------------------

    fn picker_app() -> App {
        let mut app = App::new();
        app.picker = Some(PickerState::new(vec![
            PickerEntry {
                name: "prod".into(),
                detail: "svc@db.prod.internal".into(),
                service: Some("prod".into()),
            },
            PickerEntry {
                name: "staging".into(),
                detail: "postgres@db.staging.internal".into(),
                service: Some("staging".into()),
            },
            PickerEntry {
                name: "localhost".into(),
                detail: "(default)".into(),
                service: None,
            },
        ]));
        app
    }

    fn picker_selected(app: &App) -> usize {
        app.picker.as_ref().expect("picker open").selected
    }

    #[test]
    fn picker_navigation_saturates_at_both_ends() {
        let mut app = picker_app();
        assert_eq!(picker_selected(&app), 0);

        // Up at the top stays at the top.
        update(&mut app, press(KeyCode::Char('k')));
        assert_eq!(picker_selected(&app), 0);
        update(&mut app, press(KeyCode::Up));
        assert_eq!(picker_selected(&app), 0);

        // Down walks to the last entry and saturates there.
        for _ in 0..10 {
            update(&mut app, press(KeyCode::Char('j')));
        }
        assert_eq!(picker_selected(&app), 2);
        update(&mut app, press(KeyCode::Down));
        assert_eq!(picker_selected(&app), 2);
        update(&mut app, press(KeyCode::Up));
        assert_eq!(picker_selected(&app), 1);
        assert!(!app.should_quit);
        assert!(app.picked.is_none(), "navigation never picks");
    }

    #[test]
    fn picker_enter_picks_the_highlighted_entry_and_closes_the_picker() {
        let mut app = picker_app();
        update(&mut app, press(KeyCode::Char('j')));
        update(&mut app, press(KeyCode::Enter));
        assert!(app.picker.is_none(), "picker leaves the screen");
        let picked = app.picked.as_ref().expect("entry picked");
        assert_eq!(picked.name, "staging");
        assert_eq!(picked.service.as_deref(), Some("staging"));
        assert!(!app.should_quit);
    }

    #[test]
    fn picker_enter_on_the_default_entry_maps_to_no_service() {
        let mut app = picker_app();
        for _ in 0..5 {
            update(&mut app, press(KeyCode::Char('j')));
        }
        update(&mut app, press(KeyCode::Enter));
        let picked = app.picked.as_ref().expect("entry picked");
        assert_eq!(picked.name, "localhost");
        assert_eq!(picked.service, None, "default = plain resolution");
    }

    #[test]
    fn picker_q_and_esc_quit_without_picking() {
        for code in [KeyCode::Char('q'), KeyCode::Esc] {
            let mut app = picker_app();
            update(&mut app, press(code));
            assert!(app.should_quit);
            assert!(app.picked.is_none());
            assert!(app.picker.is_some(), "no entry was consumed");
        }
    }

    #[test]
    fn picker_ignores_lens_keybindings() {
        let mut app = picker_app();
        // Tab/s/R/+/- must be inert: no lens switch, no sort change, no
        // schema refresh request, no interval change — and no panic.
        for code in [
            KeyCode::Tab,
            KeyCode::Char('s'),
            KeyCode::Char('R'),
            KeyCode::Char('+'),
            KeyCode::Char('-'),
        ] {
            update(&mut app, press(code));
        }
        assert_eq!(app.active_tab, Tab::MacroLens);
        assert_eq!(app.sort_mode, SortMode::Duration);
        assert_eq!(app.schema_refresh_requests, 0);
        assert_eq!(app.refresh_interval, DEFAULT_REFRESH);
        assert!(app.picker.is_some());
        assert!(!app.should_quit);
    }

    // --- in-session database picker (U2) --------------------------------------

    #[test]
    fn d_opens_the_picker_starting_on_the_current_database() {
        let mut app = App::new();
        update(&mut app, press(KeyCode::Char('d')));
        let picker = app.db_picker.as_ref().expect("picker opened");
        assert!(picker.entries.iter().any(|e| e.name == "shop"), "mock entries");
        let current = &picker.entries[picker.selected];
        assert_eq!(current.name, app.snapshot.vitals.database);
    }

    /// `d` works from any lens, not just the Macro Lens.
    #[test]
    fn d_opens_the_picker_from_any_lens() {
        for tab_presses in 0..Tab::TITLES.len() {
            let mut app = App::new();
            for _ in 0..tab_presses {
                update(&mut app, press(KeyCode::Tab));
            }
            update(&mut app, press(KeyCode::Char('d')));
            assert!(app.db_picker.is_some(), "tab {tab_presses}: picker must open");
        }
    }

    /// No database list yet (pre-first-tick / collection failed): `d` is a
    /// harmless no-op, never an empty useless overlay.
    #[test]
    fn d_is_a_no_op_without_a_database_list() {
        let mut app = App::new();
        let mut snap = app.snapshot.as_ref().clone();
        snap.databases = None;
        update(&mut app, Action::Snapshot(std::sync::Arc::new(snap)));
        update(&mut app, press(KeyCode::Char('d')));
        assert!(app.db_picker.is_none());
    }

    #[test]
    fn picker_j_k_move_the_db_picker_selection_saturating() {
        let mut app = App::new();
        update(&mut app, press(KeyCode::Char('d')));
        update(&mut app, press(KeyCode::Up)); // already at 0 (or wherever): must not underflow
        let picker = app.db_picker.as_ref().expect("open");
        let len = picker.entries.len();
        assert!(len >= 2, "mock must carry at least 2 databases");
        for _ in 0..len + 3 {
            update(&mut app, press(KeyCode::Char('j')));
        }
        assert_eq!(app.db_picker.as_ref().unwrap().selected, len - 1);
        for _ in 0..len + 3 {
            update(&mut app, press(KeyCode::Char('k')));
        }
        assert_eq!(app.db_picker.as_ref().unwrap().selected, 0);
    }

    /// Esc closes the overlay WITHOUT arming the top-level quit barrier — an
    /// overlay dismissal, not a top-level Esc (mirrors the detail panel).
    #[test]
    fn esc_closes_the_db_picker_without_arming_quit() {
        let mut app = App::new();
        update(&mut app, press(KeyCode::Char('d')));
        update(&mut app, press(KeyCode::Esc));
        assert!(app.db_picker.is_none());
        assert!(!app.should_quit);
        assert!(app.esc_quit_armed_until.is_none());
    }

    /// `q` is inert while the picker is open (matches the confirm modal's
    /// convention) — it must not fall through and quit the app.
    #[test]
    fn q_is_inert_while_the_db_picker_is_open() {
        let mut app = App::new();
        update(&mut app, press(KeyCode::Char('d')));
        update(&mut app, press(KeyCode::Char('q')));
        assert!(app.db_picker.is_some());
        assert!(!app.should_quit);
    }

    /// Enter on a DIFFERENT database queues the switch and closes the
    /// picker; the current database is never among the events sent (there
    /// is nothing to switch to).
    #[test]
    fn enter_on_a_different_database_queues_the_switch() {
        let mut app = App::new();
        update(&mut app, press(KeyCode::Char('d')));
        update(&mut app, press(KeyCode::Char('j'))); // move off the current selection
        let target = app.db_picker.as_ref().unwrap().entries
            [app.db_picker.as_ref().unwrap().selected]
            .name
            .clone();
        assert_ne!(target, app.snapshot.vitals.database, "test needs a different pick");
        update(&mut app, press(KeyCode::Enter));
        assert!(app.db_picker.is_none(), "overlay closes on Enter");
        assert_eq!(app.pending_db_switch, Some(target));
    }

    /// Enter on the CURRENTLY connected database is a no-op — nothing to
    /// reconnect to.
    #[test]
    fn enter_on_the_current_database_does_not_queue_a_switch() {
        let mut app = App::new();
        update(&mut app, press(KeyCode::Char('d')));
        // The picker starts on the current database (see
        // `d_opens_the_picker_starting_on_the_current_database`).
        update(&mut app, press(KeyCode::Enter));
        assert!(app.db_picker.is_none());
        assert!(app.pending_db_switch.is_none());
    }

    /// `--mock` (`App::is_mock`): Enter on a different database shows the
    /// "not simulated" toast instead of queuing a switch no mock poller
    /// would ever act on.
    #[test]
    fn mock_mode_toasts_instead_of_queueing_a_switch() {
        let mut app = App::new();
        app.is_mock = true;
        update(&mut app, press(KeyCode::Char('d')));
        update(&mut app, press(KeyCode::Char('j')));
        update(&mut app, press(KeyCode::Enter));
        assert!(app.pending_db_switch.is_none(), "mock never queues a real switch");
        let feedback = app.admin_feedback.as_ref().expect("toast shown");
        assert!(feedback.text.contains("mock mode"), "{}", feedback.text);
        assert!(!feedback.error);
    }

    #[test]
    fn host_label_action_updates_the_header_host() {
        let mut app = App::new();
        update(
            &mut app,
            Action::HostLabel("svc@db.prod.internal".to_string()),
        );
        assert_eq!(app.host, "svc@db.prod.internal");
    }

    // --- `!`: psql shell request -----------------------------------------------

    /// The real path: `!` sets the flag `main.rs` watches, and does NOT
    /// touch `admin_feedback` itself — that only happens once `main.rs`
    /// reports back via `Action::PsqlResult`.
    #[test]
    fn bang_key_sets_the_launch_request_flag() {
        let mut app = App::new();
        update(&mut app, press(KeyCode::Char('!')));
        assert!(app.launch_psql_requested);
        assert!(app.admin_feedback.is_none());
    }

    /// `--mock` has no real connection: `!` short-circuits with the same
    /// calm toast style as the database picker's mock-mode toast, and
    /// `main.rs` never sees a launch request.
    #[test]
    fn bang_key_in_mock_mode_toasts_instead_of_requesting_a_launch() {
        let mut app = App::new();
        app.is_mock = true;
        update(&mut app, press(KeyCode::Char('!')));
        assert!(!app.launch_psql_requested);
        let feedback = app.admin_feedback.as_ref().expect("toast shown");
        assert!(feedback.text.contains("mock mode"), "{}", feedback.text);
        assert!(!feedback.error);
    }

    /// `Action::PsqlResult` is the only place the outcome reaches `App` —
    /// it reuses the existing `AdminFeedback` statusline (`c`/`K`'s
    /// mechanism), so both success and failure render exactly like an
    /// admin-action result.
    #[test]
    fn psql_result_action_surfaces_as_admin_feedback() {
        let mut app = App::new();
        update(
            &mut app,
            Action::PsqlResult {
                text: "psql not found on PATH".to_string(),
                error: true,
            },
        );
        let feedback = app.admin_feedback.as_ref().expect("feedback set");
        assert_eq!(feedback.text, "psql not found on PATH");
        assert!(feedback.error);

        update(
            &mut app,
            Action::PsqlResult {
                text: "psql session ended".to_string(),
                error: false,
            },
        );
        let feedback = app.admin_feedback.as_ref().expect("feedback set");
        assert_eq!(feedback.text, "psql session ended");
        assert!(!feedback.error);
    }

    // --- admin actions (cancel/terminate) -------------------------------------

    /// App on the Micro Lens with a selected row; returns its pid.
    fn micro_app() -> (App, i32) {
        let mut app = App::new();
        update(&mut app, press(KeyCode::Tab)); // → Micro Lens
        let pid = app.selected_row().expect("selection").pid;
        (app, pid)
    }

    #[test]
    fn c_opens_the_cancel_modal_only_on_the_micro_lens() {
        // Macro Lens: inert.
        let mut app = App::new();
        update(&mut app, press(KeyCode::Char('c')));
        assert!(app.confirm.is_none());

        // Schema Lens: inert too.
        update(&mut app, press(KeyCode::Tab));
        update(&mut app, press(KeyCode::Tab));
        update(&mut app, press(KeyCode::Char('c')));
        assert!(app.confirm.is_none());

        // Micro Lens: opens the modal for the selected row.
        let (mut app, pid) = micro_app();
        update(&mut app, press(KeyCode::Char('c')));
        let confirm = app.confirm.as_ref().expect("modal open");
        assert_eq!(confirm.command, AdminCommand::CancelBackend(pid));
        assert!(app.pending_admin.is_empty(), "nothing executes before y");
    }

    /// The real gate (see `open_confirm`): read-only refuses `c`/`K` BEFORE
    /// the confirmation modal opens — no `ConfirmState`, nothing queued in
    /// `pending_admin`, so `drain_admin` would forward zero `AdminCommand`s
    /// to the poller. Feedback still explains why.
    #[test]
    fn read_only_refuses_cancel_and_terminate_before_the_modal_opens() {
        let (mut app, _pid) = micro_app();
        app.read_only = true;

        update(&mut app, press(KeyCode::Char('c')));
        assert!(app.confirm.is_none(), "read-only must not open the cancel modal");
        assert!(app.pending_admin.is_empty(), "no AdminCommand queued");
        let feedback = app.admin_feedback.as_ref().expect("refusal feedback");
        assert!(feedback.text.contains("read-only"), "{}", feedback.text);
        assert!(feedback.error);

        app.admin_feedback = None;
        update(
            &mut app,
            Action::Key(KeyEvent::new(KeyCode::Char('K'), KeyModifiers::SHIFT)),
        );
        assert!(app.confirm.is_none(), "read-only must not open the terminate modal");
        assert!(app.pending_admin.is_empty(), "no AdminCommand queued");
        assert!(app.admin_feedback.is_some());
    }

    #[test]
    fn uppercase_k_opens_terminate_and_lowercase_k_still_navigates() {
        let (mut app, _) = micro_app();
        // Move down first so lowercase-k has room to move back up.
        update(&mut app, press(KeyCode::Char('j')));
        let selected = app.table_state.selected();

        // Lowercase k: navigation, no modal.
        update(&mut app, press(KeyCode::Char('k')));
        assert!(app.confirm.is_none(), "k must stay navigation");
        assert_ne!(app.table_state.selected(), selected);

        // Uppercase K: terminate modal for the selected row.
        let pid = app.selected_row().expect("selection").pid;
        update(
            &mut app,
            Action::Key(KeyEvent::new(KeyCode::Char('K'), KeyModifiers::SHIFT)),
        );
        let confirm = app.confirm.as_ref().expect("modal open");
        assert_eq!(confirm.command, AdminCommand::TerminateBackend(pid));
    }

    #[test]
    fn admin_keys_work_with_the_detail_panel_open() {
        let (mut app, pid) = micro_app();
        update(&mut app, press(KeyCode::Enter));
        assert!(app.detail_open);
        update(&mut app, press(KeyCode::Char('c')));
        let confirm = app.confirm.as_ref().expect("modal open over detail");
        assert_eq!(confirm.command, AdminCommand::CancelBackend(pid));
    }

    #[test]
    fn y_confirms_queueing_the_command_and_showing_sent_feedback() {
        let (mut app, pid) = micro_app();
        update(&mut app, press(KeyCode::Char('c')));
        update(&mut app, press(KeyCode::Char('y')));
        assert!(app.confirm.is_none(), "modal closed");
        assert_eq!(app.pending_admin, vec![AdminCommand::CancelBackend(pid)]);
        let feedback = app.admin_feedback.as_ref().expect("sent feedback");
        assert_eq!(feedback.text, format!("cancel sent to PID {pid}\u{2026}"));
        assert!(!feedback.error);
        assert!(!app.should_quit);
    }

    #[test]
    fn n_and_esc_abort_without_queueing() {
        for code in [KeyCode::Char('n'), KeyCode::Esc] {
            let (mut app, _) = micro_app();
            update(
                &mut app,
                Action::Key(KeyEvent::new(KeyCode::Char('K'), KeyModifiers::SHIFT)),
            );
            assert!(app.confirm.is_some());
            update(&mut app, press(code));
            assert!(app.confirm.is_none(), "modal aborted");
            assert!(app.pending_admin.is_empty(), "nothing queued");
            assert!(!app.should_quit, "Esc in the modal must not quit");
        }
    }

    #[test]
    fn every_other_key_is_inert_while_the_modal_is_open() {
        let (mut app, pid) = micro_app();
        update(&mut app, press(KeyCode::Char('c')));
        let sort_before = app.sort_mode;
        let selected_before = app.table_state.selected();
        for code in [
            KeyCode::Char('q'),
            KeyCode::Tab,
            KeyCode::Char('j'),
            KeyCode::Char('k'),
            KeyCode::Char('s'),
            KeyCode::Char('K'),
            KeyCode::Enter,
            KeyCode::Char('+'),
        ] {
            update(&mut app, press(code));
        }
        assert!(!app.should_quit, "q inert while modal open");
        assert_eq!(app.active_tab, Tab::MicroLens, "Tab inert");
        assert_eq!(app.table_state.selected(), selected_before, "j/k inert");
        assert_eq!(app.sort_mode, sort_before, "s inert");
        assert!(!app.detail_open, "Enter inert");
        assert_eq!(app.refresh_interval, DEFAULT_REFRESH, "+ inert");
        assert!(app.pending_admin.is_empty());
        // Still the same modal, unresolved.
        let confirm = app.confirm.as_ref().expect("modal still open");
        assert_eq!(confirm.command, AdminCommand::CancelBackend(pid));
    }

    /// Snapshot carrying a result → outcome feedback (fired once per
    /// at_epoch_ms even though the poller re-stamps it on every snapshot).
    #[test]
    fn snapshot_result_becomes_feedback_once_per_result() {
        use pg_lens_core::AdminActionResult;

        let mut app = App::new();
        let mut snap = DbSnapshot::mock();
        snap.last_admin_action = Some(AdminActionResult {
            kind: AdminKind::Cancel,
            pid: 4977,
            outcome: AdminOutcome::Signalled(true),
            at_epoch_ms: 111,
        });
        update(&mut app, Action::Snapshot(Arc::new(snap.clone())));
        let feedback = app.admin_feedback.clone().expect("outcome feedback");
        assert_eq!(feedback.text, "query cancelled (PID 4977)");
        assert!(!feedback.error);

        // The SAME result on the next snapshot must not re-announce (the
        // feedback would never fade otherwise).
        app.admin_feedback = None;
        update(&mut app, Action::Snapshot(Arc::new(snap)));
        assert!(app.admin_feedback.is_none(), "deduped by at_epoch_ms");

        // A NEW result (new stamp) announces again.
        let mut snap = DbSnapshot::mock();
        snap.last_admin_action = Some(AdminActionResult {
            kind: AdminKind::Terminate,
            pid: 4312,
            outcome: AdminOutcome::Signalled(true),
            at_epoch_ms: 222,
        });
        update(&mut app, Action::Snapshot(Arc::new(snap)));
        assert_eq!(
            app.admin_feedback.as_ref().expect("new feedback").text,
            "backend terminated (PID 4312)"
        );
    }

    #[test]
    fn returned_false_surfaces_the_privilege_hint() {
        use pg_lens_core::AdminActionResult;

        let mut app = App::new();
        let mut snap = DbSnapshot::mock();
        snap.last_admin_action = Some(AdminActionResult {
            kind: AdminKind::Cancel,
            pid: 999,
            outcome: AdminOutcome::Signalled(false),
            at_epoch_ms: 1,
        });
        update(&mut app, Action::Snapshot(Arc::new(snap)));
        let feedback = app.admin_feedback.as_ref().expect("feedback");
        assert!(feedback.error, "false return renders loud");
        assert!(feedback.text.contains("PID 999"));
        assert!(feedback.text.contains("gone or insufficient privilege"));
        assert!(feedback.text.contains("pg_signal_backend"));
    }

    #[test]
    fn error_outcome_surfaces_the_message() {
        use pg_lens_core::AdminActionResult;

        let mut app = App::new();
        let mut snap = DbSnapshot::mock();
        snap.last_admin_action = Some(AdminActionResult {
            kind: AdminKind::Terminate,
            pid: 7,
            outcome: AdminOutcome::Error("permission denied".to_string()),
            at_epoch_ms: 1,
        });
        update(&mut app, Action::Snapshot(Arc::new(snap)));
        let feedback = app.admin_feedback.as_ref().expect("feedback");
        assert!(feedback.error);
        // Permission errors (PG >= 16 raises instead of returning false)
        // carry the same actionable hint as the false-return case.
        assert_eq!(
            feedback.text,
            "terminate PID 7 failed: permission denied (needs same user or pg_signal_backend)"
        );

        let mut snap = DbSnapshot::mock();
        snap.last_admin_action = Some(AdminActionResult {
            kind: AdminKind::Cancel,
            pid: 8,
            outcome: AdminOutcome::Error("connection closed".to_string()),
            at_epoch_ms: 2,
        });
        update(&mut app, Action::Snapshot(Arc::new(snap)));
        assert_eq!(
            app.admin_feedback.as_ref().expect("feedback").text,
            "cancel PID 8 failed: connection closed",
            "non-permission errors get no privilege hint"
        );
    }

    #[test]
    fn admin_feedback_fades_after_the_tick_deadline() {
        let (mut app, _) = micro_app();
        update(&mut app, press(KeyCode::Char('c')));
        update(&mut app, press(KeyCode::Char('y')));
        assert!(app.admin_feedback.is_some());

        // One tick short of the deadline: still on screen.
        for _ in 0..ADMIN_FEEDBACK_TICKS - 1 {
            update(&mut app, Action::Tick);
        }
        assert!(app.admin_feedback.is_some(), "still visible at deadline-1");
        // The deadline tick clears it (≈10s at the 250ms tick cadence).
        update(&mut app, Action::Tick);
        assert!(app.admin_feedback.is_none(), "faded");
    }

    // --- pause / freeze (Space) ------------------------------------------------

    #[test]
    fn space_toggles_pause_in_every_lens() {
        let mut app = App::new();
        // Macro Lens.
        update(&mut app, press(KeyCode::Char(' ')));
        assert!(app.paused);
        update(&mut app, press(KeyCode::Char(' ')));
        assert!(!app.paused);

        // Micro and Schema Lens too.
        for _ in 0..2 {
            update(&mut app, press(KeyCode::Tab));
            update(&mut app, press(KeyCode::Char(' ')));
            assert!(app.paused, "space pauses on {:?}", app.active_tab);
            update(&mut app, press(KeyCode::Char(' ')));
            assert!(!app.paused);
        }
        assert!(!app.should_quit);
    }

    #[test]
    fn paused_snapshots_park_in_pending_last_wins_and_staleness_keeps_counting() {
        let mut app = App::new();
        update(&mut app, Action::Snapshot(Arc::new(DbSnapshot::mock())));
        let frozen = Arc::clone(&app.snapshot);
        let stamped_at = app.last_snapshot_at;

        update(&mut app, press(KeyCode::Char(' ')));
        assert!(app.paused);

        // Incoming snapshots do NOT replace the frozen one...
        let first = Arc::new(DbSnapshot::mock());
        update(&mut app, Action::Snapshot(Arc::clone(&first)));
        assert!(Arc::ptr_eq(&app.snapshot, &frozen), "display stays frozen");
        assert!(Arc::ptr_eq(
            app.pending_snapshot.as_ref().expect("parked"),
            &first
        ));
        // ...the freshness stamp stays put (staleness keeps growing)...
        assert_eq!(app.last_snapshot_at, stamped_at);

        // ...and a second arrival supersedes the first (last-wins).
        let second = Arc::new(DbSnapshot::mock());
        update(&mut app, Action::Snapshot(Arc::clone(&second)));
        assert!(Arc::ptr_eq(&app.snapshot, &frozen));
        assert!(Arc::ptr_eq(
            app.pending_snapshot.as_ref().expect("parked"),
            &second
        ));
    }

    #[test]
    fn resume_applies_the_pending_snapshot_and_clears_it() {
        let mut app = App::new();
        update(&mut app, Action::Snapshot(Arc::new(DbSnapshot::mock())));
        update(&mut app, press(KeyCode::Char(' ')));
        let parked = Arc::new(DbSnapshot::mock());
        update(&mut app, Action::Snapshot(Arc::clone(&parked)));

        update(&mut app, press(KeyCode::Char(' '))); // resume
        assert!(!app.paused);
        assert!(Arc::ptr_eq(&app.snapshot, &parked), "jumped to latest");
        assert!(app.pending_snapshot.is_none());
        assert!(app.last_snapshot_at.is_some());
        // The derived state was rebuilt for the applied snapshot.
        assert_eq!(app.row_order.len(), parked.activity.len());
    }

    #[test]
    fn resume_without_a_pending_snapshot_just_unfreezes() {
        let mut app = App::new();
        let frozen = Arc::clone(&app.snapshot);
        update(&mut app, press(KeyCode::Char(' ')));
        update(&mut app, press(KeyCode::Char(' ')));
        assert!(!app.paused);
        assert!(Arc::ptr_eq(&app.snapshot, &frozen), "nothing to apply");
    }

    #[test]
    fn navigation_sort_and_detail_keep_working_on_the_frozen_data() {
        let mut app = App::new();
        update(&mut app, press(KeyCode::Char(' ')));
        assert!(app.paused);

        // Tab still switches lens.
        update(&mut app, press(KeyCode::Tab)); // → Micro Lens
        assert_eq!(app.active_tab, Tab::MicroLens);

        // j/k still move the selection over the frozen rows.
        update(&mut app, press(KeyCode::Char('j')));
        assert_eq!(app.table_state.selected(), Some(1));

        // s still re-sorts the frozen snapshot.
        update(&mut app, press(KeyCode::Char('s')));
        assert_eq!(app.sort_mode, SortMode::State);
        let states = displayed(&app, |r| r.state.clone());
        assert!(states.windows(2).all(|w| w[0] <= w[1]));

        // Enter still opens the detail panel of a frozen row.
        update(&mut app, press(KeyCode::Enter));
        assert!(app.detail_open);

        // None of that thawed the freeze.
        assert!(app.paused);
    }

    #[test]
    fn space_is_inert_in_picker_mode() {
        let mut app = picker_app();
        update(&mut app, press(KeyCode::Char(' ')));
        assert!(!app.paused, "space must not pause from the picker");
        assert!(app.picker.is_some());
        assert!(!app.should_quit);
    }

    #[test]
    fn space_is_inert_while_the_confirm_modal_is_open() {
        let (mut app, pid) = micro_app();
        update(&mut app, press(KeyCode::Char('c')));
        update(&mut app, press(KeyCode::Char(' ')));
        assert!(!app.paused, "space inert like every non-y/n/Esc key");
        let confirm = app.confirm.as_ref().expect("modal still open");
        assert_eq!(confirm.command, AdminCommand::CancelBackend(pid));
    }

    #[test]
    fn space_is_inert_on_the_connection_splash() {
        let mut app = App::new();
        update(
            &mut app,
            Action::Snapshot(Arc::new(DbSnapshot::connecting())),
        );
        assert!(app.show_splash());
        update(&mut app, press(KeyCode::Char(' ')));
        assert!(!app.paused, "no data to freeze yet");
        // The first Ok snapshot must land normally afterwards.
        update(&mut app, Action::Snapshot(Arc::new(DbSnapshot::mock())));
        assert!(!app.show_splash());
    }

    #[test]
    fn space_works_with_the_detail_panel_open() {
        let (mut app, _) = micro_app();
        update(&mut app, press(KeyCode::Enter));
        assert!(app.detail_open);
        update(&mut app, press(KeyCode::Char(' ')));
        assert!(app.paused, "analysis time IS detail time");
        assert!(app.detail_open, "the panel stays open");
    }

    /// Design decision under test: confirming an admin action while paused
    /// auto-resumes, because the action's result arrives inside the (frozen)
    /// snapshot envelope — the outcome must be visible.
    #[test]
    fn confirming_an_admin_action_while_paused_auto_resumes() {
        let (mut app, pid) = micro_app();
        update(&mut app, press(KeyCode::Char(' ')));
        let parked = Arc::new(DbSnapshot::mock());
        update(&mut app, Action::Snapshot(Arc::clone(&parked)));

        update(&mut app, press(KeyCode::Char('c')));
        update(&mut app, press(KeyCode::Char('y')));
        assert!(!app.paused, "y while paused unfreezes");
        assert!(Arc::ptr_eq(&app.snapshot, &parked), "pending applied");
        assert!(app.pending_snapshot.is_none());
        assert_eq!(app.pending_admin, vec![AdminCommand::CancelBackend(pid)]);
        // Aborting (n/Esc) must NOT resume: only a confirmed action does.
        update(&mut app, press(KeyCode::Char(' ')));
        update(&mut app, press(KeyCode::Char('K')));
        update(&mut app, press(KeyCode::Esc));
        assert!(app.paused, "abort keeps the freeze");
    }

    #[test]
    fn r_still_counts_requests_while_paused() {
        let mut app = App::new();
        update(&mut app, press(KeyCode::Char(' ')));
        update(&mut app, press(KeyCode::Char('R')));
        assert_eq!(app.schema_refresh_requests, 1, "signal still goes out");
        assert!(app.paused, "data stays frozen regardless");
    }

    #[test]
    fn snapshot_action_replaces_data_and_marks_freshness() {
        let mut app = App::new();
        assert!(app.last_snapshot_at.is_none());

        let fresh = Arc::new(DbSnapshot::mock());
        update(&mut app, Action::Snapshot(Arc::clone(&fresh)));

        assert!(Arc::ptr_eq(&app.snapshot, &fresh));
        assert!(app.last_snapshot_at.is_some());
        // row_order re-derived for the new snapshot.
        assert_eq!(app.row_order.len(), fresh.activity.len());
        // Selection still valid.
        let selected = app.table_state.selected().expect("non-empty table");
        assert!(selected < fresh.activity.len());
    }
}
