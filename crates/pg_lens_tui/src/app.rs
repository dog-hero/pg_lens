//! TEA-style Model + update for the TUI.
//!
//! `App` is pure state; [`update`] is the only place that mutates it. The
//! `Action` enum is internal to this crate — `pg_lens_core` never sees it.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use pg_lens_core::{
    AdminCommand, AdminKind, AdminOutcome, DbSnapshot, PollerStatus, TableDetailRequest,
};
use ratatui::widgets::TableState;

/// Active in-flight incident recording session (Flight Recorder).
#[derive(Debug)]
pub struct ActiveRecording {
    pub writer: pg_lens_core::recording::RecordingWriter,
    pub started_at: Instant,
}

/// Active in-memory state of an offline recording / snapshot replay.
#[derive(Clone, Debug)]
pub struct ReplayState {
    pub frames: Vec<Arc<DbSnapshot>>,
    pub current_idx: usize,
    pub is_paused: bool,
    pub speed: f64,
    pub loop_playback: bool,
    pub source_path: PathBuf,
    pub last_frame_time: Instant,
}

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
    /// Blocks & Locks Lens (v0.17): hierarchical wait-for tree + active locks.
    BlocksLens,
    /// The full replication view (U1): all senders/receiver + every slot as
    /// a scrollable table. The Macro Lens keeps its own compact, capped
    /// summary — this lens is where nothing clips.
    ReplicationLens,
    SchemaLens,
    /// The index advisor (U1), promoted out of the Schema Lens's old `i`
    /// toggle into its own full-height tab.
    IndexLens,
    QueryLens,
    /// In-flight maintenance and progress (v0.17.1): `pg_stat_progress_*`
    /// (CREATE INDEX, VACUUM, CLUSTER, ANALYZE, REINDEX).
    ProgressLens,
    /// Incident recordings & snapshot bookmarks (Tab 9).
    RecordsLens,
}

impl Tab {
    // v0.12: number-prefixed so the tab bar is self-documenting about the
    // `1`-`9` direct-jump keys (see `handle_key`'s digit arm). The prefix is
    // additive on top of the original title text (never replaces it) so
    // every pre-existing `screen.contains("Macro Lens")`-style assertion
    // keeps matching unchanged.
    pub const TITLES: [&'static str; 9] = [
        "1 Macro Lens",
        "2 Micro Lens",
        "3 Blocks & Locks Lens",
        "4 Replication Lens",
        "5 Schema Lens",
        "6 Indexes",
        "7 Query Lens",
        "8 Progress Lens",
        "9 Records Lens",
    ];

    pub fn index(self) -> usize {
        match self {
            Tab::MacroLens => 0,
            Tab::MicroLens => 1,
            Tab::BlocksLens => 2,
            Tab::ReplicationLens => 3,
            Tab::SchemaLens => 4,
            Tab::IndexLens => 5,
            Tab::QueryLens => 6,
            Tab::ProgressLens => 7,
            Tab::RecordsLens => 8,
        }
    }

    /// Inverse of [`Tab::index`] — used by the `1`-`9` direct-jump keys.
    /// `None` for anything outside `0..8`.
    pub fn from_index(index: usize) -> Option<Self> {
        match index {
            0 => Some(Tab::MacroLens),
            1 => Some(Tab::MicroLens),
            2 => Some(Tab::BlocksLens),
            3 => Some(Tab::ReplicationLens),
            4 => Some(Tab::SchemaLens),
            5 => Some(Tab::IndexLens),
            6 => Some(Tab::QueryLens),
            7 => Some(Tab::ProgressLens),
            8 => Some(Tab::RecordsLens),
            _ => None,
        }
    }

    pub fn next(self) -> Self {
        match self {
            Tab::MacroLens => Tab::MicroLens,
            Tab::MicroLens => Tab::BlocksLens,
            Tab::BlocksLens => Tab::ReplicationLens,
            Tab::ReplicationLens => Tab::SchemaLens,
            Tab::SchemaLens => Tab::IndexLens,
            Tab::IndexLens => Tab::QueryLens,
            Tab::QueryLens => Tab::ProgressLens,
            Tab::ProgressLens => Tab::RecordsLens,
            Tab::RecordsLens => Tab::MacroLens,
        }
    }

    /// Backward cycle (`BackTab` / Shift+Tab) — the exact inverse of
    /// [`Tab::next`].
    pub fn prev(self) -> Self {
        match self {
            Tab::MacroLens => Tab::RecordsLens,
            Tab::MicroLens => Tab::MacroLens,
            Tab::BlocksLens => Tab::MicroLens,
            Tab::ReplicationLens => Tab::BlocksLens,
            Tab::SchemaLens => Tab::ReplicationLens,
            Tab::IndexLens => Tab::SchemaLens,
            Tab::QueryLens => Tab::IndexLens,
            Tab::ProgressLens => Tab::QueryLens,
            Tab::RecordsLens => Tab::ProgressLens,
        }
    }
}

/// Sort modes for the Records Lens (Tab 9).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RecordsSortMode {
    #[default]
    StartedDesc,
    StartedAsc,
    SizeDesc,
    NameAsc,
}

impl RecordsSortMode {
    pub fn next(self) -> Self {
        match self {
            Self::StartedDesc => Self::StartedAsc,
            Self::StartedAsc => Self::SizeDesc,
            Self::SizeDesc => Self::NameAsc,
            Self::NameAsc => Self::StartedDesc,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::StartedDesc => "started (newest)",
            Self::StartedAsc => "started (oldest)",
            Self::SizeDesc => "size (largest)",
            Self::NameAsc => "name (A-Z)",
        }
    }
}

/// Active pane of the Blocks Lens (v0.17).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BlocksPane {
    /// Wait-for tree with root blockers.
    #[default]
    Tree,
    /// Active locks table (`pg_locks`).
    Locks,
}

impl BlocksPane {
    pub fn toggle(self) -> Self {
        match self {
            Self::Tree => Self::Locks,
            Self::Locks => Self::Tree,
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
    /// v0.14: largest `|Δ1h|` (absolute bytes) first — surfaces the fastest
    /// growers AND the fastest shrinkers at the top; tables with no growth
    /// reading yet (fresh session/table) sort last.
    Growth,
}

impl SchemaSortMode {
    pub fn next(self) -> Self {
        match self {
            SchemaSortMode::TotalSize => SchemaSortMode::DeadTuples,
            SchemaSortMode::DeadTuples => SchemaSortMode::BloatPct,
            SchemaSortMode::BloatPct => SchemaSortMode::SeqScans,
            SchemaSortMode::SeqScans => SchemaSortMode::Growth,
            SchemaSortMode::Growth => SchemaSortMode::TotalSize,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            SchemaSortMode::TotalSize => "size",
            SchemaSortMode::DeadTuples => "dead",
            SchemaSortMode::BloatPct => "bloat%",
            SchemaSortMode::SeqScans => "seq",
            SchemaSortMode::Growth => "\u{394}1h",
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
    /// Full-height: sequence headroom exhaustion watch (v0.19, `S` toggles).
    Sequences,
}

impl SchemaView {
    pub fn next(self) -> Self {
        match self {
            SchemaView::Tables => SchemaView::Vacuum,
            SchemaView::Vacuum => SchemaView::Tables,
            SchemaView::Sequences => SchemaView::Tables,
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
    /// Heaviest temp-file (work_mem) spillers first, by `temp_blks_written`
    /// (v0.14) — the #1 query-tuning signal this lens adds.
    Temp,
}

impl StatementsSortMode {
    pub fn next(self) -> Self {
        match self {
            StatementsSortMode::TotalTime => StatementsSortMode::Calls,
            StatementsSortMode::Calls => StatementsSortMode::Mean,
            StatementsSortMode::Mean => StatementsSortMode::Rows,
            StatementsSortMode::Rows => StatementsSortMode::Temp,
            StatementsSortMode::Temp => StatementsSortMode::TotalTime,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            StatementsSortMode::TotalTime => "total",
            StatementsSortMode::Calls => "calls",
            StatementsSortMode::Mean => "mean",
            StatementsSortMode::Rows => "rows",
            StatementsSortMode::Temp => "temp",
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
    /// v0.16 (`y`): `main.rs` emitted the OSC 52 copy escape sequence (the
    /// only place allowed to touch stdout directly — see `clipboard.rs`'s
    /// module doc) and reports the exact toast text back — same
    /// `AdminFeedback` mechanism `c`/`K`/`!` already use. `update()` stays
    /// the sole mutation point even though the actual terminal write
    /// happened in `main.rs`, mirroring `PsqlResult`'s same reasoning.
    ClipboardCopied { text: String },
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
    /// Sequences sub-view selection (v0.19, `S` toggles).
    pub sequences_table_state: TableState,
    pub sequences_row_order: Vec<usize>,
    /// Indices into `snapshot.schema.indexes` in severity-then-size display
    /// order (the Index Lens's twin of `schema_row_order`; no sort mode of
    /// its own — see [`index_finding_rank`]).
    pub index_row_order: Vec<usize>,
    /// Index Lens selection, independent from every other lens's cursor
    /// (U1: it used to share `SchemaView::Indexes`'s state before the
    /// promotion to its own tab; the field name is unchanged).
    pub index_table_state: TableState,
    /// Search filter for the Index Lens (v0.19, `/` edits, `\` clears).
    pub index_filter: String,
    pub index_filter_saved: String,
    pub index_filter_editing: bool,
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
    /// Blocks Lens tree selection (v0.17).
    pub blocks_tree_state: TableState,
    /// Blocks Lens locks table selection (v0.17).
    pub blocks_locks_state: TableState,
    /// Blocks Lens active pane (v0.17: Tree vs Locks).
    pub blocks_active_pane: BlocksPane,
    /// Progress Lens selection state (v0.17.1).
    pub progress_table_state: TableState,
    /// Filtered indices into `app.snapshot.unified_progress()` in display order (v0.17.1).
    pub progress_row_order: Vec<usize>,
    /// Search filter for the Progress Lens (v0.17.1, `/` edits, `\` clears).
    pub progress_filter: String,
    pub progress_filter_saved: String,
    pub progress_filter_editing: bool,
    /// Incident recordings and snapshot bookmarks (Tab 9: Records Lens).
    pub records: Vec<pg_lens_core::recording::RecordingEntry>,
    pub records_row_order: Vec<usize>,
    pub records_table_state: TableState,
    pub records_filter: String,
    pub records_filter_saved: String,
    pub records_filter_editing: bool,
    pub records_sort_mode: RecordsSortMode,
    pub record_max_bytes: usize,
    pub record_compress: bool,
    pub record_max_total_bytes: Option<u64>,
    pub record_retention_days: Option<u64>,
    pub delete_record_target: Option<pg_lens_core::recording::RecordingEntry>,
    pub live_mode_saved_snapshot: Option<Arc<DbSnapshot>>,
    /// Whether the detail panel is open (Micro Lens: full query of the
    /// selected session; Schema Lens: full vacuum/analyze stats + index
    /// bloat of the selected table). While open: `j`/`k` still move the
    /// selection (the panel follows it), `Enter`/`Esc` close the panel,
    /// `Tab` closes it and switches lens, `q` quits as always.
    pub detail_open: bool,
    /// v0.15: current scroll offset (in lines) of the Schema Lens table
    /// detail overlay's `\d` sections (columns/constraints/indexes) — the
    /// only detail overlay that scrolls independently of `j`/`k` moving the
    /// underlying row selection (see `handle_key`'s dedicated arms). Reset
    /// to 0 whenever a fresh [`TableDetailRequest`] is queued (opening the
    /// overlay, or moving the selection while it is open).
    pub table_detail_scroll: u16,
    /// v0.15: at most one pending on-demand table-detail request, queued by
    /// `update()` (Enter on a Schema Lens table, or moving the selection
    /// while its detail overlay is open) and drained by the main loop into
    /// the poller's `mpsc::Sender<TableDetailRequest>` — the exact mirror of
    /// `pending_db_switch`.
    pub table_detail_request: Option<TableDetailRequest>,
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
    /// v0.15's partition collapsing: `false` (default) hides
    /// `TableStatRow::is_partition` rows from `schema_row_order` in favor of
    /// their aggregated parent row; `p` toggles it. Tables view only (same
    /// scope as `schema_filter`) — persists across lens switches like
    /// `schema_sort_mode`, reset never happens automatically (an operator
    /// who expanded it stays expanded until they toggle back).
    pub schema_show_partitions: bool,
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
    /// v0.16: at most one pending copy-to-clipboard request, queued by `y`
    /// (see [`clipboard_text`]) and drained by `main.rs` into an OSC 52
    /// write on stdout — the exact mirror of `launch_psql_requested`/
    /// `table_detail_request`. Holds the FULL text to copy (already resolved
    /// from whatever the active lens has selected), never a lens/context
    /// enum: `update()` is the only place with enough state to resolve it,
    /// and `main.rs` should not need to reach back into `App` beyond taking
    /// this one field.
    pub clipboard_request: Option<String>,
    pub recording: Option<ActiveRecording>,
    pub replay_state: Option<ReplayState>,
    pub state_dir: Option<PathBuf>,
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
            sequences_table_state: TableState::default().with_selected(0),
            sequences_row_order: Vec::new(),
            index_row_order: Vec::new(),
            index_table_state: TableState::default().with_selected(0),
            index_filter: String::new(),
            index_filter_saved: String::new(),
            index_filter_editing: false,
            replication_row_order: Vec::new(),
            replication_table_state: TableState::default().with_selected(0),
            statements_row_order: Vec::new(),
            statements_sort_mode: StatementsSortMode::default(),
            statements_table_state: TableState::default().with_selected(0),
            blocks_tree_state: TableState::default().with_selected(0),
            blocks_locks_state: TableState::default().with_selected(0),
            blocks_active_pane: BlocksPane::default(),
            progress_table_state: TableState::default().with_selected(0),
            progress_row_order: Vec::new(),
            progress_filter: String::new(),
            progress_filter_saved: String::new(),
            progress_filter_editing: false,
            records: Vec::new(),
            records_row_order: Vec::new(),
            records_table_state: TableState::default().with_selected(0),
            records_filter: String::new(),
            records_filter_saved: String::new(),
            records_filter_editing: false,
            records_sort_mode: RecordsSortMode::default(),
            record_max_bytes: pg_lens_core::recording::DEFAULT_MAX_RECORD_BYTES,
            record_compress: false,
            record_max_total_bytes: None,
            record_retention_days: None,
            delete_record_target: None,
            live_mode_saved_snapshot: None,
            detail_open: false,
            table_detail_scroll: 0,
            table_detail_request: None,
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
            schema_show_partitions: false,
            statements_filter: String::new(),
            statements_filter_editing: false,
            statements_filter_saved: String::new(),
            esc_quit_armed_until: None,
            help_open: false,
            launch_psql_requested: false,
            clipboard_request: None,
            recording: None,
            replay_state: None,
            state_dir: None,
            should_quit: false,
        };
        resort(&mut app);
        resort_schema(&mut app);
        resort_sequences(&mut app);
        resort_indexes(&mut app);
        resort_replication(&mut app);
        resort_statements(&mut app);
        resort_progress(&mut app);
        app.refresh_records();
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

    /// The Sequence currently under the cursor, in display order.
    pub fn selected_sequence(&self) -> Option<&pg_lens_core::SequenceRow> {
        let schema = self.snapshot.schema.as_deref()?;
        let display_idx = self.sequences_table_state.selected()?;
        let snapshot_idx = *self.sequences_row_order.get(display_idx)?;
        schema.sequences.get(snapshot_idx)
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

    /// The Blocks Lens tree node currently under the cursor (v0.17).
    pub fn selected_block_node(&self) -> Option<&pg_lens_core::BlockTreeNode> {
        let tree = self.snapshot.blocking_tree.as_deref()?;
        let idx = self.blocks_tree_state.selected()?;
        fn find<'a>(nodes: &'a [pg_lens_core::BlockTreeNode], curr: &mut usize, target: usize) -> Option<&'a pg_lens_core::BlockTreeNode> {
            for node in nodes {
                if *curr == target {
                    return Some(node);
                }
                *curr += 1;
                if let Some(found) = find(&node.children, curr, target) {
                    return Some(found);
                }
            }
            None
        }
        let mut curr = 0;
        find(tree, &mut curr, idx)
    }

    /// The Blocks Lens active lock currently under the cursor (v0.17).
    pub fn selected_active_lock(&self) -> Option<&pg_lens_core::ActiveLockRow> {
        let locks = self.snapshot.active_locks.as_deref()?;
        let idx = self.blocks_locks_state.selected()?;
        locks.get(idx)
    }

    /// The Progress Lens row currently under the cursor (v0.17.1).
    pub fn selected_progress_row(&self) -> Option<pg_lens_core::ProgressUnifiedRow> {
        let display_idx = self.progress_table_state.selected()?;
        let unified_idx = *self.progress_row_order.get(display_idx)?;
        let items = self.snapshot.unified_progress();
        items.get(unified_idx).cloned()
    }

    /// The Records Lens recording or bookmark currently under the cursor (Tab 9).
    pub fn selected_recording(&self) -> Option<&pg_lens_core::recording::RecordingEntry> {
        let display_idx = self.records_table_state.selected()?;
        let record_idx = *self.records_row_order.get(display_idx)?;
        self.records.get(record_idx)
    }

    /// Prunes old recordings and snapshot bookmarks based on retention and quota settings.
    pub fn prune_records(&mut self) {
        if self.record_max_total_bytes.is_none() && self.record_retention_days.is_none() {
            return;
        }
        let recordings_dir = self
            .state_dir
            .as_ref()
            .map(|d| d.join("recordings"))
            .or_else(pg_lens_core::recording::recordings_dir)
            .unwrap_or_else(|| std::env::temp_dir().join("pg_lens").join("recordings"));
        let exports_dir = self
            .state_dir
            .as_ref()
            .map(|d| d.join("exports"))
            .or_else(pg_lens_core::recording::exports_dir)
            .unwrap_or_else(|| std::env::temp_dir().join("pg_lens").join("exports"));
        let active_path = self.recording.as_ref().map(|r| r.writer.path());
        let _ = pg_lens_core::recording::prune_recordings(
            &recordings_dir,
            &exports_dir,
            self.record_max_total_bytes,
            self.record_retention_days,
            active_path,
        );
    }

    /// Scans the state directories and refreshes available recordings and snapshot bookmarks.
    pub fn refresh_records(&mut self) {
        self.prune_records();
        let recordings_dir = self
            .state_dir
            .as_ref()
            .map(|d| d.join("recordings"))
            .or_else(pg_lens_core::recording::recordings_dir)
            .unwrap_or_else(|| std::env::temp_dir().join("pg_lens").join("recordings"));
        let exports_dir = self
            .state_dir
            .as_ref()
            .map(|d| d.join("exports"))
            .or_else(pg_lens_core::recording::exports_dir)
            .unwrap_or_else(|| std::env::temp_dir().join("pg_lens").join("exports"));
        let active_path = self.recording.as_ref().map(|r| r.writer.path());
        self.records = pg_lens_core::recording::list_recordings(&recordings_dir, &exports_dir, active_path);
        resort_records(self);
        clamp_selection(self);
    }

    /// Toggles incident recording mode (Flight Recorder).
    pub fn toggle_recording(&mut self) {
        if self.replay_state.is_some() {
            self.admin_feedback = Some(AdminFeedback {
                text: "recording is disabled during replay".to_string(),
                error: false,
                expires_at_tick: self.tick_count + ADMIN_FEEDBACK_TICKS,
            });
            return;
        }
        if let Some(rec) = self.recording.take() {
            match rec.writer.finish() {
                Ok((path, count, bytes)) => {
                    let path_str = path.display().to_string();
                    self.clipboard_request = Some(path_str.clone());
                    let kb = bytes as f64 / 1024.0;
                    self.admin_feedback = Some(AdminFeedback {
                        text: format!("Recording saved ({count} frames, {kb:.1} KB): {path_str}"),
                        error: false,
                        expires_at_tick: self.tick_count + ADMIN_FEEDBACK_TICKS * 2,
                    });
                    self.refresh_records();
                }
                Err(e) => {
                    self.admin_feedback = Some(AdminFeedback {
                        text: format!("Recording save failed: {e}"),
                        error: true,
                        expires_at_tick: self.tick_count + ADMIN_FEEDBACK_TICKS * 2,
                    });
                }
            }
        } else {
            let writer_res = if let Some(ref dir) = self.state_dir {
                pg_lens_core::recording::RecordingWriter::create_in_full(
                    &self.host,
                    &dir.join("recordings"),
                    self.record_max_bytes,
                    self.record_compress,
                )
            } else {
                pg_lens_core::recording::RecordingWriter::new_full(
                    &self.host,
                    self.record_max_bytes,
                    self.record_compress,
                )
            };
            match writer_res {
                Ok(mut writer) => {
                    let _ = writer.append(&self.snapshot);
                    self.recording = Some(ActiveRecording {
                        writer,
                        started_at: Instant::now(),
                    });
                    self.admin_feedback = Some(AdminFeedback {
                        text: "\u{25cf} Recording started (Shift+R to stop)".to_string(),
                        error: false,
                        expires_at_tick: self.tick_count + ADMIN_FEEDBACK_TICKS,
                    });
                    self.refresh_records();
                }
                Err(e) => {
                    self.admin_feedback = Some(AdminFeedback {
                        text: format!("Recording failed to start: {e}"),
                        error: true,
                        expires_at_tick: self.tick_count + ADMIN_FEEDBACK_TICKS * 2,
                    });
                }
            }
        }
    }

    /// Exports current snapshot bookmark to pretty JSON in state directory.
    pub fn export_snapshot(&mut self) {
        let export_res = if let Some(ref dir) = self.state_dir {
            pg_lens_core::recording::export_snapshot_to(&self.snapshot, &self.host, &dir.join("exports"))
        } else {
            pg_lens_core::recording::export_snapshot(&self.snapshot, &self.host)
        };
        match export_res {
            Ok(path) => {
                let path_str = path.display().to_string();
                self.clipboard_request = Some(path_str.clone());
                self.admin_feedback = Some(AdminFeedback {
                    text: format!("Exported snapshot to {path_str}"),
                    error: false,
                    expires_at_tick: self.tick_count + ADMIN_FEEDBACK_TICKS * 2,
                });
                self.refresh_records();
            }
            Err(e) => {
                self.admin_feedback = Some(AdminFeedback {
                    text: format!("Snapshot export failed: {e}"),
                    error: true,
                    expires_at_tick: self.tick_count + ADMIN_FEEDBACK_TICKS * 2,
                });
            }
        }
    }
}

/// v0.16 (`y`): resolves the text `y` should copy for whatever the active
/// lens currently has selected — `None` when there is nothing to copy (no
/// row selected, or the lens has no clipboard-worthy content at all, e.g.
/// the Replication Lens). Per the feature spec:
/// - Micro Lens (Activity view): the selected row's FULL query text (not
///   the truncated table cell — the same text the `Enter` detail panel
///   shows);
/// - Query Lens: the selected statement's full normalized text;
/// - Index Lens: the selected index's verbatim `CREATE INDEX` definition;
/// - Schema Lens (Tables view): the selected table's qualified name
///   (`schema.table`) — or, when the on-demand structure detail overlay is
///   open AND has resolved for THIS table, its column list instead (a more
///   useful paste target once the operator is already looking at columns).
pub fn clipboard_text(app: &App) -> Option<String> {
    match app.active_tab {
        Tab::MicroLens if app.micro_view == MicroView::Activity => {
            app.selected_row().map(|row| row.query.clone())
        }
        Tab::QueryLens => app.selected_statement().map(|row| row.query.clone()),
        Tab::IndexLens => app.selected_index().map(|idx| idx.indexdef.clone()),
        Tab::SchemaLens if app.schema_view == SchemaView::Tables => {
            let table = app.selected_table()?;
            if app.detail_open
                && let Some(detail) = app.snapshot.table_detail.as_deref()
                && detail.oid == table.oid
                && detail.error.is_none()
                && !detail.columns.is_empty()
            {
                return Some(schema_column_list(detail));
            }
            Some(format!("{}.{}", table.schema, table.name))
        }
        Tab::SchemaLens if app.schema_view == SchemaView::Sequences => {
            let seq = app.selected_sequence()?;
            Some(format!("{}.{}", seq.schema, seq.sequence_name))
        }
        Tab::BlocksLens => {
            if app.blocks_active_pane == BlocksPane::Tree {
                app.selected_block_node().map(|n| n.query.clone())
            } else {
                app.selected_active_lock().map(|l| l.query.clone())
            }
        }
        Tab::ProgressLens => {
            let row = app.selected_progress_row()?;
            if let Some(session) = app.snapshot.activity.iter().find(|a| a.pid == row.pid) {
                Some(session.query.clone())
            } else {
                Some(format!("{} on {}", row.command, row.relation))
            }
        }
        Tab::RecordsLens => app.selected_recording().map(|r| r.path.display().to_string()),
        _ => None,
    }
}

/// `name type [NOT NULL]`, one column per line — the copy payload for a
/// Schema Lens table whose structure detail is open. Deliberately terse
/// (no constraints/indexes/identity notes): the obvious, pasteable "what are
/// this table's columns" answer, not a reconstruction of the full `\d`.
fn schema_column_list(detail: &pg_lens_core::TableDetail) -> String {
    detail
        .columns
        .iter()
        .map(|c| {
            if c.not_null {
                format!("{} {} NOT NULL", c.name, c.data_type)
            } else {
                format!("{} {}", c.name, c.data_type)
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The single mutation point of the Model.
pub fn update(app: &mut App, action: Action) {
    match action {
        Action::Key(key) => handle_key(app, key),
        Action::Snapshot(snapshot) => {
            if let Some(ref mut rec) = app.recording {
                match rec.writer.append(&snapshot) {
                    Ok(Some(new_path)) => {
                        let path_str = new_path.display().to_string();
                        app.admin_feedback = Some(AdminFeedback {
                            text: format!("Recording rotated to {path_str}"),
                            error: false,
                            expires_at_tick: app.tick_count + ADMIN_FEEDBACK_TICKS * 2,
                        });
                        app.refresh_records();
                    }
                    Ok(None) => {}
                    Err(e) => {
                        app.admin_feedback = Some(AdminFeedback {
                            text: format!("Recording write failed: {e}"),
                            error: true,
                            expires_at_tick: app.tick_count + ADMIN_FEEDBACK_TICKS * 2,
                        });
                    }
                }
            }
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
        Action::ClipboardCopied { text } => {
            let message = if let Some(existing) = app.admin_feedback.take() {
                if existing.text.contains("Exported snapshot") || existing.text.contains("Recording saved") {
                    format!("{} ({})", existing.text, text)
                } else {
                    text
                }
            } else {
                text
            };
            app.admin_feedback = Some(AdminFeedback {
                text: message,
                error: false,
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
            let mut snap_to_apply = None;
            if let Some(ref mut replay) = app.replay_state {
                if !replay.is_paused && replay.frames.len() > 1 {
                    let step_millis = (1000.0 / replay.speed.max(0.1)) as u128;
                    if replay.last_frame_time.elapsed().as_millis() >= step_millis {
                        replay.last_frame_time = Instant::now();
                        let next_idx = replay.current_idx + 1;
                        if next_idx < replay.frames.len() {
                            replay.current_idx = next_idx;
                            snap_to_apply = Some(replay.frames[replay.current_idx].clone());
                        } else if replay.loop_playback {
                            replay.current_idx = 0;
                            snap_to_apply = Some(replay.frames[0].clone());
                        } else {
                            replay.is_paused = true;
                        }
                    }
                }
            }
            if let Some(snap) = snap_to_apply {
                apply_snapshot(app, snap);
            }
        }
        Action::Quit => {
            if let Some(rec) = app.recording.take() {
                let _ = rec.writer.finish();
            }
            app.should_quit = true;
        }
    }
}

/// Makes `snapshot` the one on screen: freshness stamp, splash gate,
/// admin-result feedback, re-sorts and selection clamps. Shared by the
/// live path (`Action::Snapshot` while not paused) and [`resume`] (which
/// applies the parked `pending_snapshot`).
pub fn apply_snapshot(app: &mut App, snapshot: Arc<DbSnapshot>) {
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
    resort_sequences(app);
    resort_indexes(app);
    resort_replication(app);
    resort_statements(app);
    resort_progress(app);
    if app.active_tab == Tab::RecordsLens && app.recording.is_some() {
        app.refresh_records();
    }
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
    // Delete recording confirmation modal
    if app.delete_record_target.is_some() {
        handle_delete_record_confirm_key(app, key);
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
    if active_filter_lens(app).is_some() {
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
            if let Some(snap) = app.live_mode_saved_snapshot.take() {
                app.replay_state = None;
                apply_snapshot(app, snap);
                app.admin_feedback = Some(AdminFeedback {
                    text: "Exited replay, returned to live view".to_string(),
                    error: false,
                    expires_at_tick: app.tick_count + ADMIN_FEEDBACK_TICKS,
                });
            } else if app.detail_open {
                app.detail_open = false;
            } else if app.waits_open {
                app.waits_open = false;
            } else if app.active_tab == Tab::SchemaLens && app.schema_view != SchemaView::Tables {
                // The `v` Vacuum sub-view is an overlay too: Esc returns to
                // The `v` Vacuum sub-view and `S` Sequences sub-view are overlay-like sub-views: Esc returns to
                // the Tables view, it does NOT arm quitting.
                app.schema_view = SchemaView::Tables;
            } else if app.active_tab == Tab::IndexLens
                && app.previous_tab == Some(Tab::SchemaLens)
                && !app.index_filter.is_empty()
            {
                // Esc returns to Schema Lens when jumped via `i`
                app.index_filter.clear();
                resort_indexes(app);
                app.active_tab = Tab::SchemaLens;
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
            } else if app.active_tab == Tab::RecordsLens {
                if let Some(entry) = app.selected_recording().cloned() {
                    match pg_lens_core::recording::RecordingReader::load(&entry.path) {
                        Ok(raw_frames) if !raw_frames.is_empty() => {
                            let frames: Vec<Arc<DbSnapshot>> =
                                raw_frames.into_iter().map(Arc::new).collect();
                            let initial = frames[0].clone();
                            app.live_mode_saved_snapshot = Some(app.snapshot.clone());
                            app.replay_state = Some(ReplayState {
                                frames,
                                current_idx: 0,
                                is_paused: false,
                                speed: 1.0,
                                loop_playback: false,
                                source_path: entry.path.clone(),
                                last_frame_time: Instant::now(),
                            });
                            apply_snapshot(app, initial);
                            app.active_tab = Tab::MacroLens;
                            app.admin_feedback = Some(AdminFeedback {
                                text: format!(
                                    "Replaying {} (Space: pause, \u{2190}/\u{2192}: step, Esc: exit replay)",
                                    entry.filename
                                ),
                                error: false,
                                expires_at_tick: app.tick_count + ADMIN_FEEDBACK_TICKS * 2,
                            });
                        }
                        Ok(_) => {
                            app.admin_feedback = Some(AdminFeedback {
                                text: format!("File {} contains no snapshots", entry.filename),
                                error: true,
                                expires_at_tick: app.tick_count + ADMIN_FEEDBACK_TICKS * 2,
                            });
                        }
                        Err(e) => {
                            app.admin_feedback = Some(AdminFeedback {
                                text: format!("Failed to read {}: {e}", entry.filename),
                                error: true,
                                expires_at_tick: app.tick_count + ADMIN_FEEDBACK_TICKS * 2,
                            });
                        }
                    }
                }
            } else if (app.active_tab == Tab::MicroLens
                && app.micro_view == MicroView::Activity
                && app.table_state.selected().is_some())
                || (app.active_tab == Tab::SchemaLens
                    && app.schema_view == SchemaView::Tables
                    && app.selected_table().is_some())
                || (app.active_tab == Tab::IndexLens && app.selected_index().is_some())
                || (app.active_tab == Tab::QueryLens && app.selected_statement().is_some())
                || (app.active_tab == Tab::BlocksLens
                    && (app.selected_block_node().is_some() || app.selected_active_lock().is_some()))
                || (app.active_tab == Tab::ProgressLens && app.selected_progress_row().is_some())
            {
                app.detail_open = true;
                // v0.15: fires the on-demand `\d` request (Schema Lens Tables
                // view only — a no-op on the other three lenses this arm
                // covers, whose detail panels need no extra fetch).
                sync_table_detail_request(app);
            }
        }
        KeyCode::Tab => {
            app.previous_tab = Some(app.active_tab);
            app.detail_open = false;
            app.waits_open = false;
            app.active_tab = app.active_tab.next();
            if app.active_tab == Tab::RecordsLens {
                app.refresh_records();
            }
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
            if app.active_tab == Tab::RecordsLens {
                app.refresh_records();
            }
        }
        // v0.12: direct tab jump, in `Tab::TITLES`/`Tab::index()` order (the
        // tab bar's number prefixes document this). Reached only when no
        // overlay/modal/picker/filter-edit owns input (they all return
        // earlier in this function), so it can never hijack a digit typed
        // into the filter editor or a confirm-modal keystroke. A no-op if
        // already on that tab (nothing to remember as "previous").
        KeyCode::Char(c @ '1'..='9') => {
            if let Some(tab) = Tab::from_index(c as usize - '1' as usize)
                && tab != app.active_tab
            {
                app.previous_tab = Some(app.active_tab);
                app.detail_open = false;
                app.waits_open = false;
                app.active_tab = tab;
                if tab == Tab::RecordsLens {
                    app.refresh_records();
                }
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
                if prev == Tab::RecordsLens {
                    app.refresh_records();
                }
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
            if app.active_tab == Tab::SchemaLens
                && (app.schema_view == SchemaView::Tables || app.schema_view == SchemaView::Sequences) =>
        {
            app.schema_filter_saved = app.schema_filter.clone();
            app.schema_filter_editing = true;
        }
        KeyCode::Char('/') if app.active_tab == Tab::IndexLens => {
            app.index_filter_saved = app.index_filter.clone();
            app.index_filter_editing = true;
        }
        KeyCode::Char('/') if app.active_tab == Tab::QueryLens => {
            app.statements_filter_saved = app.statements_filter.clone();
            app.statements_filter_editing = true;
        }
        KeyCode::Char('/') if app.active_tab == Tab::ProgressLens => {
            app.progress_filter_saved = app.progress_filter.clone();
            app.progress_filter_editing = true;
        }
        KeyCode::Char('/') if app.active_tab == Tab::RecordsLens => {
            app.records_filter_saved = app.records_filter.clone();
            app.records_filter_editing = true;
        }
        // v0.12: `\` clears the ACTIVE lens's committed filter in one key —
        // inert when there is nothing to clear (empty filter) or while
        // editing (Esc already reverts there). Chosen over `Esc` (already
        // overloaded: closes overlays, then arms the quit barrier — adding
        // a THIRD meaning would make a stray Esc unpredictable) and over a
        // digit/letter already claimed by v0.12's own navigation batch
        // (`1`-`8`, `g`/`G`, Backspace, BackTab) or by an existing lens key
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
                if (app.schema_view == SchemaView::Tables || app.schema_view == SchemaView::Sequences)
                    && !app.schema_filter.is_empty() =>
            {
                app.schema_filter.clear();
                resort_schema(app);
                resort_sequences(app);
                clamp_selection(app);
            }
            Tab::IndexLens if !app.index_filter.is_empty() => {
                app.index_filter.clear();
                resort_indexes(app);
                clamp_selection(app);
            }
            Tab::QueryLens if !app.statements_filter.is_empty() => {
                app.statements_filter.clear();
                resort_statements(app);
                clamp_selection(app);
            }
            Tab::ProgressLens if !app.progress_filter.is_empty() => {
                app.progress_filter.clear();
                resort_progress(app);
                clamp_selection(app);
            }
            Tab::RecordsLens if !app.records_filter.is_empty() => {
                app.records_filter.clear();
                resort_records(app);
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
        // `p` (v0.15, mnemonic "partitions"): toggles the Schema Lens Tables
        // view between the collapsed default (leaf partitions hidden behind
        // their aggregated parent row) and the expanded view (every leaf
        // shown too). Tables view only — the Vacuum sub-view has no
        // partition concept of its own. Re-sorts/re-clamps immediately so
        // the row count and cursor stay valid the same frame.
        KeyCode::Char('p')
            if app.active_tab == Tab::SchemaLens && app.schema_view == SchemaView::Tables =>
        {
            app.schema_show_partitions = !app.schema_show_partitions;
            resort_schema(app);
            clamp_selection(app);
        }
        // `p` or `o` (v0.17): toggles between Tree and Locks panes in Blocks Lens.
        KeyCode::Char('p') | KeyCode::Char('o') if app.active_tab == Tab::BlocksLens => {
            app.blocks_active_pane = app.blocks_active_pane.toggle();
        }
        // `b` (v0.17, mnemonic "blocks"): jumps directly to Blocks & Locks Lens.
        KeyCode::Char('b') if app.active_tab != Tab::BlocksLens => {
            app.previous_tab = Some(app.active_tab);
            app.detail_open = false;
            app.waits_open = false;
            app.active_tab = Tab::BlocksLens;
        }
        // `x` (v0.15, mnemonic "cross-reference"): jumps from the selected
        // Schema Lens table to the Query Lens with `statements_filter`
        // seeded to the table's bare name (substring match — imperfect,
        // documented on `App::statements_filter`'s call site below and in
        // the title rendering the v0.12 filter machinery already provides).
        // Tables view only, and only with a row actually selected (mirrors
        // the statusbar hint's own gate in `ui/mod.rs`). Works on partition
        // parents too — `selected_table()` returns the synthesized parent
        // row exactly like any other, and its `name` is the parent's own
        // bare name.
        KeyCode::Char('x')
            if app.active_tab == Tab::SchemaLens && app.schema_view == SchemaView::Tables =>
        {
            if let Some(table) = app.selected_table() {
                let name = table.name.clone();
                app.previous_tab = Some(app.active_tab);
                app.detail_open = false;
                app.waits_open = false;
                app.active_tab = Tab::QueryLens;
                app.statements_filter = name;
                app.statements_filter_saved = app.statements_filter.clone();
                app.statements_filter_editing = false;
                resort_statements(app);
                app.statements_table_state.select(Some(0));
                clamp_selection(app);
            }
        }
        // `i` (v0.19, cross-lens jump from Schema Lens to Index Lens):
        KeyCode::Char('i')
            if app.active_tab == Tab::SchemaLens && app.schema_view == SchemaView::Tables =>
        {
            if let Some(table) = app.selected_table() {
                let name = table.name.clone();
                app.previous_tab = Some(app.active_tab);
                app.detail_open = false;
                app.waits_open = false;
                app.active_tab = Tab::IndexLens;
                app.index_filter = name;
                app.index_filter_saved = app.index_filter.clone();
                app.index_filter_editing = false;
                resort_indexes(app);
                app.index_table_state.select(Some(0));
                clamp_selection(app);
            }
        }
        // `S` (v0.19, Sequences sub-view): toggles between Tables and Sequences view in Schema Lens.
        KeyCode::Char('S') if app.active_tab == Tab::SchemaLens => {
            app.schema_view = match app.schema_view {
                SchemaView::Sequences => SchemaView::Tables,
                _ => SchemaView::Sequences,
            };
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
        // `y` (v0.16, vim yank): copies whatever the active lens has
        // selected to the clipboard via OSC 52 — see [`clipboard_text`] for
        // exactly what that is per lens. Free at this top level: `y` only
        // ever means "confirm" INSIDE the admin modal (`handle_confirm_key`,
        // reached via its own early return above `handle_key`'s big match),
        // so the two never collide. `update()` only QUEUES the request
        // (pure state, per `CLAUDE.md`'s no-`.await`-in-ui'/no-I/O-outside-
        // main.rs discipline) — `main.rs` performs the actual terminal write
        // and reports back via `Action::ClipboardCopied`.
        KeyCode::Char('y') => {
            if let Some(text) = clipboard_text(app) {
                app.clipboard_request = Some(text);
            } else {
                app.admin_feedback = Some(AdminFeedback {
                    text: "nothing to copy here".to_string(),
                    error: false,
                    expires_at_tick: app.tick_count + ADMIN_FEEDBACK_TICKS,
                });
            }
        }
        // On Records Lens: `x` or `Delete` prompts deletion of selected recording.
        // On all lenses: `d` opens the database picker.
        KeyCode::Char('x') | KeyCode::Delete if app.active_tab == Tab::RecordsLens => {
            if let Some(entry) = app.selected_recording() {
                if entry.is_active {
                    app.admin_feedback = Some(AdminFeedback {
                        text: "Cannot delete active in-flight recording (stop recording first)".to_string(),
                        error: true,
                        expires_at_tick: app.tick_count + ADMIN_FEEDBACK_TICKS,
                    });
                } else {
                    app.delete_record_target = Some(entry.clone());
                }
            }
        }
        KeyCode::Char('b') | KeyCode::Char('B') if app.active_tab == Tab::RecordsLens => {
            app.refresh_records();
            app.admin_feedback = Some(AdminFeedback {
                text: "Rescanned recordings directory".to_string(),
                error: false,
                expires_at_tick: app.tick_count + ADMIN_FEEDBACK_TICKS,
            });
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
            if app.replay_state.is_some() {
                app.admin_feedback = Some(AdminFeedback {
                    text: "replay mode: no live connection for psql".to_string(),
                    error: false,
                    expires_at_tick: app.tick_count + ADMIN_FEEDBACK_TICKS,
                });
            } else if app.is_mock {
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
        // v0.15: while the Schema Lens table detail overlay is open, `j`/`k`
        // (and the arrow keys) scroll its `\d` sections instead of moving
        // the underlying row selection — columns + constraints + indexes
        // routinely overflow the panel. Every OTHER detail overlay (Micro/
        // Index/Query Lens) keeps the pre-existing "selection moves, panel
        // follows" behavior untouched (these arms only fire on the Schema
        // Lens's Tables view, and are checked before the generic movement
        // arms below).
        KeyCode::Up | KeyCode::Char('k')
            if app.detail_open
                && app.active_tab == Tab::SchemaLens
                && app.schema_view == SchemaView::Tables =>
        {
            app.table_detail_scroll = app.table_detail_scroll.saturating_sub(1);
        }
        KeyCode::Down | KeyCode::Char('j')
            if app.detail_open
                && app.active_tab == Tab::SchemaLens
                && app.schema_view == SchemaView::Tables =>
        {
            // Soft cap, not a precise content-length clamp: `ui/` (the only
            // place that knows the overlay's real line count and visible
            // height) clamps the RENDERED offset already, so overshooting
            // here just means a few extra `j` presses do nothing visible —
            // never a panic, never unbounded growth.
            app.table_detail_scroll = (app.table_detail_scroll + 1).min(500);
        }
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
        KeyCode::Home | KeyCode::Char('g') => {
            if let Some(ref mut replay) = app.replay_state {
                replay.is_paused = true;
                replay.current_idx = 0;
                if let Some(snap) = replay.frames.first().cloned() {
                    apply_snapshot(app, snap);
                }
            } else {
                move_selection_to(app, 0);
            }
        }
        KeyCode::End | KeyCode::Char('G') => {
            if let Some(ref mut replay) = app.replay_state {
                replay.is_paused = true;
                if !replay.frames.is_empty() {
                    replay.current_idx = replay.frames.len() - 1;
                    if let Some(snap) = replay.frames.last().cloned() {
                        apply_snapshot(app, snap);
                    }
                }
            } else {
                move_selection_to(app, i64::MAX);
            }
        }
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
            Tab::RecordsLens => {
                app.records_sort_mode = app.records_sort_mode.next();
                resort_records(app);
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
        // Space: pause/resume the display refresh (UI-side freeze; the
        // poller keeps its cadence — see `App::paused`). In replay mode:
        // toggles playback play/pause.
        KeyCode::Char(' ') => {
            let mut snap_to_apply = None;
            if let Some(ref mut replay) = app.replay_state {
                if replay.is_paused {
                    // If we're at the very end of the recording and user hits Play (Space),
                    // rewind back to frame 0 and restart playback from the beginning.
                    if replay.current_idx + 1 >= replay.frames.len() && !replay.frames.is_empty() {
                        replay.current_idx = 0;
                        snap_to_apply = Some(replay.frames[0].clone());
                    }
                    replay.is_paused = false;
                    replay.last_frame_time = Instant::now();
                } else {
                    replay.is_paused = true;
                }
            } else if !app.show_splash() {
                toggle_pause(app);
            }
            if let Some(snap) = snap_to_apply {
                apply_snapshot(app, snap);
            }
        }
        // `B` (uppercase, Shift+B): request an immediate schema & bloat re-collection.
        // Allowed from any lens — fresh data is ready when the user tabs in.
        KeyCode::Char('B') => {
            if app.replay_state.is_none() {
                app.schema_refresh_requests += 1;
            }
        }
        // `Shift+R` (`R`) or `Ctrl+R`: toggle incident recording mode (Flight Recorder).
        KeyCode::Char('R') => {
            app.toggle_recording();
        }
        KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.toggle_recording();
        }
        // `E`: export point-in-time snapshot to JSON bookmark.
        KeyCode::Char('E') => {
            app.export_snapshot();
        }
        // Replay stepping: Left/Right arrows step frame backward/forward
        KeyCode::Left => {
            let mut snap_to_apply = None;
            if let Some(ref mut replay) = app.replay_state {
                replay.is_paused = true;
                if replay.current_idx > 0 {
                    replay.current_idx -= 1;
                    snap_to_apply = Some(replay.frames[replay.current_idx].clone());
                }
            }
            if let Some(snap) = snap_to_apply {
                apply_snapshot(app, snap);
            }
        }
        KeyCode::Right => {
            let mut snap_to_apply = None;
            if let Some(ref mut replay) = app.replay_state {
                replay.is_paused = true;
                if replay.current_idx + 1 < replay.frames.len() {
                    replay.current_idx += 1;
                    snap_to_apply = Some(replay.frames[replay.current_idx].clone());
                }
            }
            if let Some(snap) = snap_to_apply {
                apply_snapshot(app, snap);
            }
        }
        KeyCode::Char('l') | KeyCode::Char('L') => {
            if let Some(ref mut replay) = app.replay_state {
                replay.loop_playback = !replay.loop_playback;
                let status = if replay.loop_playback { "enabled" } else { "disabled" };
                app.admin_feedback = Some(AdminFeedback {
                    text: format!("Replay loop {status}"),
                    error: false,
                    expires_at_tick: app.tick_count + ADMIN_FEEDBACK_TICKS,
                });
            }
        }
        // Replay speed adjustments: [ slows down, ] speeds up
        KeyCode::Char('[') => {
            if let Some(ref mut replay) = app.replay_state {
                replay.speed = (replay.speed / 2.0).max(0.25);
                app.admin_feedback = Some(AdminFeedback {
                    text: format!("Playback speed: {:.2}x", replay.speed),
                    error: false,
                    expires_at_tick: app.tick_count + ADMIN_FEEDBACK_TICKS,
                });
            }
        }
        KeyCode::Char(']') => {
            if let Some(ref mut replay) = app.replay_state {
                replay.speed = (replay.speed * 2.0).min(16.0);
                app.admin_feedback = Some(AdminFeedback {
                    text: format!("Playback speed: {:.2}x", replay.speed),
                    error: false,
                    expires_at_tick: app.tick_count + ADMIN_FEEDBACK_TICKS,
                });
            }
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
    if app.replay_state.is_some() {
        return;
    }
    let (pid, username, database) = if app.active_tab == Tab::MicroLens && app.micro_view == MicroView::Activity {
        let Some(row) = app.selected_row() else { return; };
        (row.pid, row.username.clone(), row.database.clone())
    } else if app.active_tab == Tab::BlocksLens {
        if app.blocks_active_pane == BlocksPane::Tree {
            let Some(node) = app.selected_block_node() else { return; };
            (node.pid, node.usename.clone(), app.snapshot.vitals.database.clone())
        } else {
            let Some(lock) = app.selected_active_lock() else { return; };
            (lock.pid, lock.usename.clone(), app.snapshot.vitals.database.clone())
        }
    } else if app.active_tab == Tab::ProgressLens {
        let Some(progress) = app.selected_progress_row() else { return; };
        let (username, database) = app
            .snapshot
            .activity
            .iter()
            .find(|a| a.pid == progress.pid)
            .map(|a| (a.username.clone(), a.database.clone()))
            .unwrap_or_else(|| ("postgres".to_string(), app.snapshot.vitals.database.clone()));
        (progress.pid, username, database)
    } else {
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
        AdminCommand::TerminateBackend(pid)
    } else {
        AdminCommand::CancelBackend(pid)
    };
    app.confirm = Some(ConfirmState {
        command,
        username,
        database,
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

/// Keymap while confirming deletion of an incident recording file.
fn handle_delete_record_confirm_key(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Char('y') | KeyCode::Char('Y') => {
            if let Some(target) = app.delete_record_target.take() {
                match pg_lens_core::recording::delete_recording(&target.path) {
                    Ok(()) => {
                        app.admin_feedback = Some(AdminFeedback {
                            text: format!("Deleted recording: {}", target.filename),
                            error: false,
                            expires_at_tick: app.tick_count + ADMIN_FEEDBACK_TICKS * 2,
                        });
                        app.refresh_records();
                    }
                    Err(e) => {
                        app.admin_feedback = Some(AdminFeedback {
                            text: format!("Failed to delete {}: {e}", target.filename),
                            error: true,
                            expires_at_tick: app.tick_count + ADMIN_FEEDBACK_TICKS * 2,
                        });
                    }
                }
            }
        }
        KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
            app.delete_record_target = None;
        }
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.should_quit = true;
        }
        _ => {}
    }
}

/// Opens the in-session database picker (`d`). A no-op when the poller has
/// not yet collected the database list (`snapshot.databases` is `None`
/// before the first successful fast tick, or on any collection failure) —
/// nothing to pick from yet; the key simply does nothing rather than open an
/// empty, useless overlay.
fn open_db_picker(app: &mut App) {
    if app.replay_state.is_some() {
        app.admin_feedback = Some(AdminFeedback {
            text: "replay mode: cannot switch database".to_string(),
            error: false,
            expires_at_tick: app.tick_count + ADMIN_FEEDBACK_TICKS,
        });
        return;
    }
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
    Progress,
    Records,
    Index,
}

/// `None` when no filter is being edited — defensive; `handle_key` only
/// routes into [`handle_filter_key`] when at least one `*_filter_editing`
/// flag is set, and the flags are mutually exclusive by construction
/// (only one `/` arm can fire per keypress, each setting exactly one).
fn active_filter_lens(app: &App) -> Option<FilterLens> {
    if app.filter_editing {
        Some(FilterLens::Micro)
    } else if app.schema_filter_editing {
        Some(FilterLens::Schema)
    } else if app.statements_filter_editing {
        Some(FilterLens::Query)
    } else if app.progress_filter_editing {
        Some(FilterLens::Progress)
    } else if app.records_filter_editing {
        Some(FilterLens::Records)
    } else if app.index_filter_editing {
        Some(FilterLens::Index)
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
        FilterLens::Schema => {
            resort_schema(app);
            resort_sequences(app);
        }
        FilterLens::Query => resort_statements(app),
        FilterLens::Progress => resort_progress(app),
        FilterLens::Records => resort_records(app),
        FilterLens::Index => resort_indexes(app),
    }
}

/// Keymap while editing ANY lens's filter (`app.filter_editing` /
/// `schema_filter_editing` / `statements_filter_editing` / `progress_filter_editing` — exactly one is
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
            FilterLens::Progress => app.progress_filter_editing = false,
            FilterLens::Records => app.records_filter_editing = false,
            FilterLens::Index => app.index_filter_editing = false,
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
                FilterLens::Progress => {
                    app.progress_filter = std::mem::take(&mut app.progress_filter_saved);
                    app.progress_filter_editing = false;
                }
                FilterLens::Records => {
                    app.records_filter = std::mem::take(&mut app.records_filter_saved);
                    app.records_filter_editing = false;
                }
                FilterLens::Index => {
                    app.index_filter = std::mem::take(&mut app.index_filter_saved);
                    app.index_filter_editing = false;
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
                FilterLens::Progress => {
                    app.progress_filter.pop();
                }
                FilterLens::Records => {
                    app.records_filter.pop();
                }
                FilterLens::Index => {
                    app.index_filter.pop();
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
                FilterLens::Progress => app.progress_filter.push(c),
                FilterLens::Records => app.records_filter.push(c),
                FilterLens::Index => app.index_filter.push(c),
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
    sync_table_detail_request(app);
}

/// v0.15: queues a fresh [`TableDetailRequest`] for whatever table the
/// Schema Lens's Tables-view cursor currently points at, and resets
/// [`App::table_detail_scroll`] to 0 — called on every event that changes
/// what the detail overlay should show while it is open (opening it via
/// Enter, or moving the selection while it stays open, since the panel
/// "follows" the cursor — see `App::detail_open`'s doc comment). A no-op on
/// every other lens/overlay state.
fn sync_table_detail_request(app: &mut App) {
    if !app.detail_open || app.active_tab != Tab::SchemaLens || app.schema_view != SchemaView::Tables
    {
        return;
    }
    app.table_detail_scroll = 0;
    if let Some(table) = app.selected_table() {
        app.table_detail_request = Some(TableDetailRequest::Fetch {
            oid: table.oid,
            schema: table.schema.clone(),
            name: table.name.clone(),
        });
    }
}

/// Jumps the active lens's table selection to an absolute position, clamped
/// into range: `target <= 0` goes to the first row, `target >=
/// len.saturating_sub(1)` goes to the last (so `i64::MAX` is the idiomatic
/// "last row" — see the `Home`/`End`/`g`/`G` arms in `handle_key`). Shares
/// the exact same per-lens (state, len) routing as [`move_selection`], so it
/// works on every lens that has a selectable table with no per-lens code.
fn move_selection_to(app: &mut App, target: i64) {
    move_selection_to_inner(app, target);
    sync_table_detail_request(app);
}

fn move_selection_to_inner(app: &mut App, target: i64) {
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
        Tab::BlocksLens if app.blocks_active_pane == BlocksPane::Tree => {
            let len = app.snapshot.blocking_tree.as_deref().map_or(0, |tree| {
                fn count(nodes: &[pg_lens_core::BlockTreeNode]) -> usize {
                    nodes.iter().map(|n| 1 + count(&n.children)).sum()
                }
                count(tree)
            });
            (&mut app.blocks_tree_state, len)
        }
        Tab::BlocksLens => {
            let len = app.snapshot.active_locks.as_deref().map_or(0, |l| l.len());
            (&mut app.blocks_locks_state, len)
        }
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
        Tab::SchemaLens if app.schema_view == SchemaView::Sequences => (
            &mut app.sequences_table_state,
            app.sequences_row_order.len(),
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
        Tab::ProgressLens => (
            &mut app.progress_table_state,
            app.progress_row_order.len(),
        ),
        Tab::RecordsLens => (
            &mut app.records_table_state,
            app.records_row_order.len(),
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

    // Sequences sub-view (v0.19): no detail panel to close, just a cursor to keep valid.
    let sequences_len = app.sequences_row_order.len();
    if sequences_len == 0 {
        app.sequences_table_state.select(None);
    } else {
        let clamped = app
            .sequences_table_state
            .selected()
            .unwrap_or(0)
            .min(sequences_len - 1);
        app.sequences_table_state.select(Some(clamped));
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

    // Progress Lens (v0.17.1): clamps progress rows.
    let progress_len = app.progress_row_order.len();
    if progress_len == 0 {
        app.progress_table_state.select(None);
        if app.active_tab == Tab::ProgressLens {
            app.detail_open = false;
        }
    } else {
        let clamped = app
            .progress_table_state
            .selected()
            .unwrap_or(0)
            .min(progress_len - 1);
        app.progress_table_state.select(Some(clamped));
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

    // Blocks Lens (v0.17): clamps tree selection and active locks selection.
    let blocks_tree_len = app.snapshot.blocking_tree.as_deref().map_or(0, |tree| {
        fn count(nodes: &[pg_lens_core::BlockTreeNode]) -> usize {
            nodes.iter().map(|n| 1 + count(&n.children)).sum()
        }
        count(tree)
    });
    if blocks_tree_len == 0 {
        app.blocks_tree_state.select(None);
    } else {
        let clamped = app
            .blocks_tree_state
            .selected()
            .unwrap_or(0)
            .min(blocks_tree_len - 1);
        app.blocks_tree_state.select(Some(clamped));
    }

    let blocks_locks_len = app.snapshot.active_locks.as_deref().map_or(0, |l| l.len());
    if blocks_locks_len == 0 {
        app.blocks_locks_state.select(None);
    } else {
        let clamped = app
            .blocks_locks_state
            .selected()
            .unwrap_or(0)
            .min(blocks_locks_len - 1);
        app.blocks_locks_state.select(Some(clamped));
    }

    // Records Lens (Tab 9): clamps records rows.
    let records_len = app.records_row_order.len();
    if records_len == 0 {
        app.records_table_state.select(None);
    } else {
        let clamped = app
            .records_table_state
            .selected()
            .unwrap_or(0)
            .min(records_len - 1);
        app.records_table_state.select(Some(clamped));
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
///
/// v0.15's partition collapsing composes with the filter: leaf partitions
/// (`is_partition`) are dropped from the display order UNLESS
/// `schema_show_partitions` is on — but the filter itself still matches
/// against every row (parents AND, when expanded, leaves), never just the
/// visible subset, so toggling `p` after filtering does not need a
/// re-search.
fn resort_schema(app: &mut App) {
    let Some(schema) = app.snapshot.schema.as_deref() else {
        app.schema_row_order = Vec::new();
        return;
    };
    let rows = &schema.tables;
    let needle = app.schema_filter.to_lowercase();
    let show_partitions = app.schema_show_partitions;
    let mut order: Vec<usize> = (0..rows.len())
        .filter(|&i| show_partitions || !rows[i].is_partition)
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
        SchemaSortMode::Growth => {
            // Descending by |Δ1h bytes|; unknown growth (`None`) sorts
            // last, keyed as -1 (a valid |delta| is always >= 0).
            let abs_growth = |i: usize| rows[i].growth_1h_bytes.map_or(-1, i64::abs);
            order.sort_by(|&a, &b| {
                abs_growth(b)
                    .cmp(&abs_growth(a))
                    .then_with(|| by_size_then_name(a, b))
            });
        }
    }
    app.schema_row_order = order;
}

/// Recomputes `sequences_row_order` from the current snapshot (v0.19).
/// Headroom exhaustion order: percent_used DESC, then remaining_count ASC;
/// ties break by schema/sequence ascending so the order is deterministic.
fn resort_sequences(app: &mut App) {
    let Some(schema) = app.snapshot.schema.as_deref() else {
        app.sequences_row_order = Vec::new();
        return;
    };
    let rows = &schema.sequences;
    let needle = app.schema_filter.trim().to_lowercase();
    let mut order: Vec<usize> = (0..rows.len())
        .filter(|&i| {
            needle.is_empty()
                || rows[i].schema.to_lowercase().contains(&needle)
                || rows[i].sequence_name.to_lowercase().contains(&needle)
                || rows[i].table_name.to_lowercase().contains(&needle)
                || rows[i].column_name.to_lowercase().contains(&needle)
        })
        .collect();
    order.sort_by(|&a, &b| {
        rows[b]
            .percent_used
            .total_cmp(&rows[a].percent_used)
            .then_with(|| rows[a].remaining_count.cmp(&rows[b].remaining_count))
            .then_with(|| {
                (&rows[a].schema, &rows[a].sequence_name)
                    .cmp(&(&rows[b].schema, &rows[b].sequence_name))
            })
    });
    app.sequences_row_order = order;
}

/// Recomputes `index_row_order` from the current snapshot (the Index Lens's
/// twin of [`resort_schema`]). Fixed severity-then-size order (no
/// user-chosen sort — see [`index_finding_rank`]); ties break by
/// schema/table/name ascending so the order is deterministic.
/// twin of [`resort_schema`]). Filtered by `index_filter`.
/// Fixed severity-then-size order (no user-chosen sort — see [`index_finding_rank`]);
/// ties break by schema/table/name ascending so the order is deterministic.
fn resort_indexes(app: &mut App) {
    let Some(schema) = app.snapshot.schema.as_deref() else {
        app.index_row_order = Vec::new();
        return;
    };
    let rows = &schema.indexes;
    let needle = app.index_filter.trim().to_lowercase();
    let mut order: Vec<usize> = (0..rows.len())
        .filter(|&i| {
            needle.is_empty()
                || rows[i].schema.to_lowercase().contains(&needle)
                || rows[i].table.to_lowercase().contains(&needle)
                || rows[i].name.to_lowercase().contains(&needle)
        })
        .collect();
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
        StatementsSortMode::Temp => order.sort_by(|&a, &b| {
            rows[b]
                .temp_blks_written
                .cmp(&rows[a].temp_blks_written)
                .then_with(|| tiebreak(a, b))
        }),
    }
    app.statements_row_order = order;
}

fn progress_row_matches(row: &pg_lens_core::ProgressUnifiedRow, needle: &str) -> bool {
    row.pid.to_string().contains(needle)
        || row.command.to_lowercase().contains(needle)
        || row.relation.to_lowercase().contains(needle)
        || row.phase.to_lowercase().contains(needle)
        || row.detail.to_lowercase().contains(needle)
}

/// Recomputes `progress_row_order` from current snapshot's unified progress + filter (v0.17.1).
fn resort_progress(app: &mut App) {
    let rows = app.snapshot.unified_progress();
    let needle = app.progress_filter.to_lowercase();
    let order: Vec<usize> = (0..rows.len())
        .filter(|&i| needle.is_empty() || progress_row_matches(&rows[i], &needle))
        .collect();
    app.progress_row_order = order;
}

fn records_row_matches(entry: &pg_lens_core::recording::RecordingEntry, needle: &str) -> bool {
    entry.filename.to_lowercase().contains(needle)
        || entry.target.to_lowercase().contains(needle)
        || match entry.kind {
            pg_lens_core::recording::RecordingKind::Recording => "recording rec",
            pg_lens_core::recording::RecordingKind::Bookmark => "bookmark snapshot point",
        }
        .contains(needle)
        || entry
            .started_at
            .as_deref()
            .is_some_and(|s: &str| s.to_lowercase().contains(needle))
        || entry
            .ended_at
            .as_deref()
            .is_some_and(|s: &str| s.to_lowercase().contains(needle))
}

/// Recomputes `records_row_order` from available disk records + filter + sort mode.
pub fn resort_records(app: &mut App) {
    let rows = &app.records;
    let needle = app.records_filter.to_lowercase();
    let mut order: Vec<usize> = (0..rows.len())
        .filter(|&i| needle.is_empty() || records_row_matches(&rows[i], &needle))
        .collect();

    match app.records_sort_mode {
        RecordsSortMode::StartedDesc => order.sort_by(|&a, &b| {
            rows[b]
                .started_at_secs
                .cmp(&rows[a].started_at_secs)
                .then_with(|| rows[a].filename.cmp(&rows[b].filename))
        }),
        RecordsSortMode::StartedAsc => order.sort_by(|&a, &b| {
            rows[a]
                .started_at_secs
                .cmp(&rows[b].started_at_secs)
                .then_with(|| rows[a].filename.cmp(&rows[b].filename))
        }),
        RecordsSortMode::SizeDesc => order.sort_by(|&a, &b| {
            rows[b]
                .size_bytes
                .cmp(&rows[a].size_bytes)
                .then_with(|| rows[a].filename.cmp(&rows[b].filename))
        }),
        RecordsSortMode::NameAsc => order.sort_by(|&a, &b| {
            rows[a].filename.cmp(&rows[b].filename)
        }),
    }

    app.records_row_order = order;
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

    // --- v0.15: on-demand `\d` table detail (Enter on the Tables view) ----

    /// Enter on a Schema Lens table opens the overlay AND queues a
    /// `TableDetailRequest::Fetch` for the selected table, resetting the
    /// scroll offset.
    #[test]
    fn enter_on_a_schema_table_queues_a_detail_fetch_request() {
        let mut app = App::new();
        app.active_tab = Tab::SchemaLens;
        app.table_detail_scroll = 7;
        let selected = app.selected_table().expect("mock has tables").clone();
        update(&mut app, press(KeyCode::Enter));
        assert!(app.detail_open);
        assert_eq!(app.table_detail_scroll, 0);
        match app.table_detail_request.take().expect("request queued") {
            pg_lens_core::TableDetailRequest::Fetch { oid, schema, name } => {
                assert_eq!(oid, selected.oid);
                assert_eq!(schema, selected.schema);
                assert_eq!(name, selected.name);
            }
            other => panic!("expected Fetch, got {other:?}"),
        }
    }

    /// Moving the selection while the detail overlay is open (e.g. via
    /// `End`, since plain `j`/`k` are claimed by the overlay's own scroll —
    /// see `jk_scroll_the_detail_overlay_instead_of_moving_selection`)
    /// re-queues a fresh request for the newly selected table (the panel
    /// "follows" the cursor) and resets the scroll offset again.
    #[test]
    fn moving_selection_with_detail_open_requeues_the_request() {
        let mut app = App::new();
        app.active_tab = Tab::SchemaLens;
        update(&mut app, press(KeyCode::Enter));
        app.table_detail_request.take(); // drain the initial Enter request
        app.table_detail_scroll = 3;
        update(&mut app, press(KeyCode::End));
        assert_eq!(app.table_detail_scroll, 0);
        assert!(app.table_detail_request.is_some());
    }

    /// `j`/`k` while the overlay is open scroll the panel, NOT the
    /// underlying row selection — columns/constraints/indexes routinely
    /// overflow the visible area.
    #[test]
    fn jk_scroll_the_detail_overlay_instead_of_moving_selection() {
        let mut app = App::new();
        app.active_tab = Tab::SchemaLens;
        update(&mut app, press(KeyCode::Enter));
        let selection_before = app.schema_table_state.selected();
        update(&mut app, press(KeyCode::Char('j')));
        assert_eq!(app.table_detail_scroll, 1);
        assert_eq!(
            app.schema_table_state.selected(),
            selection_before,
            "row selection must not move while the detail overlay is open"
        );
        update(&mut app, press(KeyCode::Char('j')));
        assert_eq!(app.table_detail_scroll, 2);
        update(&mut app, press(KeyCode::Char('k')));
        assert_eq!(app.table_detail_scroll, 1);
    }

    /// The scroll offset never underflows past 0.
    #[test]
    fn detail_scroll_clamps_at_zero() {
        let mut app = App::new();
        app.active_tab = Tab::SchemaLens;
        update(&mut app, press(KeyCode::Enter));
        assert_eq!(app.table_detail_scroll, 0);
        update(&mut app, press(KeyCode::Char('k')));
        assert_eq!(app.table_detail_scroll, 0, "must not underflow");
    }

    /// Every other detail overlay (e.g. the Micro Lens's activity detail)
    /// keeps the pre-existing "j/k moves selection, panel follows" contract
    /// untouched — the dedicated scroll arms only fire on the Schema Lens's
    /// Tables view.
    #[test]
    fn jk_still_moves_selection_on_other_lens_detail_overlays() {
        let mut app = App::new();
        app.active_tab = Tab::MicroLens;
        update(&mut app, press(KeyCode::Enter));
        assert!(app.detail_open);
        let before = app.table_state.selected();
        update(&mut app, press(KeyCode::Char('j')));
        assert_ne!(app.table_state.selected(), before, "selection should move");
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
    fn tab_cycles_the_nine_lenses() {
        let mut app = App::new();
        assert_eq!(app.active_tab, Tab::MacroLens);
        update(&mut app, press(KeyCode::Tab));
        assert_eq!(app.active_tab, Tab::MicroLens);
        update(&mut app, press(KeyCode::Tab));
        assert_eq!(app.active_tab, Tab::BlocksLens);
        update(&mut app, press(KeyCode::Tab));
        assert_eq!(app.active_tab, Tab::ReplicationLens);
        update(&mut app, press(KeyCode::Tab));
        assert_eq!(app.active_tab, Tab::SchemaLens);
        update(&mut app, press(KeyCode::Tab));
        assert_eq!(app.active_tab, Tab::IndexLens);
        update(&mut app, press(KeyCode::Tab));
        assert_eq!(app.active_tab, Tab::QueryLens);
        update(&mut app, press(KeyCode::Tab));
        assert_eq!(app.active_tab, Tab::ProgressLens);
        update(&mut app, press(KeyCode::Tab));
        assert_eq!(app.active_tab, Tab::RecordsLens);
        update(&mut app, press(KeyCode::Tab));
        assert_eq!(app.active_tab, Tab::MacroLens);
        assert!(!app.should_quit);
    }

    // --- v0.12: navigation & scroll polish ----------------------------------

    #[test]
    fn back_tab_cycles_the_nine_lenses_backward() {
        let mut app = App::new();
        assert_eq!(app.active_tab, Tab::MacroLens);
        update(&mut app, press(KeyCode::BackTab));
        assert_eq!(app.active_tab, Tab::RecordsLens);
        update(&mut app, press(KeyCode::BackTab));
        assert_eq!(app.active_tab, Tab::ProgressLens);
        update(&mut app, press(KeyCode::BackTab));
        assert_eq!(app.active_tab, Tab::QueryLens);
        update(&mut app, press(KeyCode::BackTab));
        assert_eq!(app.active_tab, Tab::IndexLens);
        update(&mut app, press(KeyCode::BackTab));
        assert_eq!(app.active_tab, Tab::SchemaLens);
        update(&mut app, press(KeyCode::BackTab));
        assert_eq!(app.active_tab, Tab::ReplicationLens);
        update(&mut app, press(KeyCode::BackTab));
        assert_eq!(app.active_tab, Tab::BlocksLens);
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
            ('3', Tab::BlocksLens),
            ('4', Tab::ReplicationLens),
            ('5', Tab::SchemaLens),
            ('6', Tab::IndexLens),
            ('7', Tab::QueryLens),
            ('8', Tab::ProgressLens),
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
        assert_eq!(app.active_tab, Tab::ReplicationLens);
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

        update(&mut app, press(KeyCode::Char('6'))); // → Index Lens
        assert_eq!(app.active_tab, Tab::IndexLens);
        update(&mut app, press(KeyCode::Backspace)); // → back to Macro Lens
        assert_eq!(app.active_tab, Tab::MacroLens);
        update(&mut app, press(KeyCode::Backspace)); // toggles right back
        assert_eq!(app.active_tab, Tab::IndexLens);
    }

    // v0.15's cross-lens jump (`x`): Schema Lens table → Query Lens, seeding
    // `statements_filter` to the table's bare name.

    #[test]
    fn x_jumps_to_query_lens_seeded_with_the_selected_table_name() {
        let mut app = App::new();
        app.active_tab = Tab::SchemaLens;
        app.schema_table_state.select(Some(0));
        let table_name = app.selected_table().expect("mock has tables").name.clone();

        update(&mut app, press(KeyCode::Char('x')));

        assert_eq!(app.active_tab, Tab::QueryLens);
        assert_eq!(app.previous_tab, Some(Tab::SchemaLens));
        assert_eq!(app.statements_filter, table_name);
        assert_eq!(app.statements_filter_saved, table_name);
        assert!(!app.statements_filter_editing);
        assert_eq!(app.statements_table_state.selected(), Some(0));
        // The title rendering (v0.12) makes the seeded filter visible; the
        // filter machinery itself narrows the row order to matches.
        assert!(
            app.statements_row_order
                .iter()
                .all(|&i| app.snapshot.statements.as_ref().unwrap().statements[i]
                    .query
                    .to_lowercase()
                    .contains(&table_name.to_lowercase()))
        );
    }

    #[test]
    fn x_backspace_returns_to_schema_lens() {
        let mut app = App::new();
        app.active_tab = Tab::SchemaLens;
        app.schema_table_state.select(Some(0));
        update(&mut app, press(KeyCode::Char('x')));
        assert_eq!(app.active_tab, Tab::QueryLens);

        update(&mut app, press(KeyCode::Backspace));
        assert_eq!(app.active_tab, Tab::SchemaLens);
    }

    #[test]
    fn x_never_touches_the_schema_lens_filter() {
        let mut app = App::new();
        app.active_tab = Tab::SchemaLens;
        update(&mut app, press(KeyCode::Char('/')));
        type_str(&mut app, "public");
        update(&mut app, press(KeyCode::Enter));
        assert_eq!(app.schema_filter, "public");
        app.schema_table_state.select(Some(0));

        update(&mut app, press(KeyCode::Char('x')));

        assert_eq!(app.active_tab, Tab::QueryLens);
        assert_eq!(app.schema_filter, "public", "Schema Lens filter untouched");
    }

    /// Works on partition parents too — `selected_table()` returns the
    /// synthesized parent row exactly like any other table, and `x` seeds
    /// its own bare name.
    #[test]
    fn x_works_on_a_partition_parent_row() {
        let mut app = App::new();
        app.active_tab = Tab::SchemaLens;
        let schema = app.snapshot.schema.clone().expect("mock has schema");
        let idx = app
            .schema_row_order
            .iter()
            .position(|&i| schema.tables[i].partition_count.is_some())
            .expect("mock has a partition parent row");
        app.schema_table_state.select(Some(idx));
        let parent_name = app.selected_table().expect("selected").name.clone();

        update(&mut app, press(KeyCode::Char('x')));

        assert_eq!(app.active_tab, Tab::QueryLens);
        assert_eq!(app.statements_filter, parent_name);
    }

    /// Inert off the Tables view (e.g. the Vacuum sub-view) and with no row
    /// selected — the gate mirrors `handle_key`'s own `if` condition.
    #[test]
    fn x_is_inert_off_the_tables_view_and_with_no_selection() {
        let mut app = App::new();
        app.active_tab = Tab::SchemaLens;
        app.schema_view = SchemaView::Vacuum;
        app.schema_table_state.select(Some(0));
        update(&mut app, press(KeyCode::Char('x')));
        assert_eq!(app.active_tab, Tab::SchemaLens, "Vacuum sub-view: no jump");

        app.schema_view = SchemaView::Tables;
        app.schema_table_state.select(None);
        update(&mut app, press(KeyCode::Char('x')));
        assert_eq!(app.active_tab, Tab::SchemaLens, "no selection: no jump");
    }

    /// `x` is scoped to the Schema Lens — pressing it elsewhere (e.g. the
    /// Micro Lens) must not be misread as some other lens's binding.
    #[test]
    fn x_is_inert_on_other_lenses() {
        let mut app = App::new();
        app.active_tab = Tab::MicroLens;
        update(&mut app, press(KeyCode::Char('x')));
        assert_eq!(app.active_tab, Tab::MicroLens);
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
        let schema = app.snapshot.schema.as_deref().expect("mock schema");
        // v0.15: leaf partitions stay collapsed by default (`p` not
        // pressed), so the cleared filter's row order is every table
        // EXCEPT the mock's 3 hidden leaves, not the raw table count.
        let hidden_leaves = schema.tables.iter().filter(|t| t.is_partition).count();
        assert_eq!(app.schema_row_order.len(), schema.tables.len() - hidden_leaves);

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
        assert_eq!(app.active_tab, Tab::BlocksLens);
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
        for _ in 0..4 {
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

        // s → |Δ1h| descending (mock's biggest grower: order_items), unknown
        // growth (raw_events, no ring sample yet in mock) sorts last.
        update(&mut app, press(KeyCode::Char('s')));
        assert_eq!(app.schema_sort_mode, SchemaSortMode::Growth);
        let names = schema_displayed(&app, |t| t.name.clone());
        assert_eq!(names[0], "order_items", "largest |growth| first");
        assert_eq!(names[names.len() - 1], "raw_events", "unknown growth sorts last");

        // s → back to size; the Micro Lens sort was never touched.
        update(&mut app, press(KeyCode::Char('s')));
        assert_eq!(app.schema_sort_mode, SchemaSortMode::TotalSize);
        assert_eq!(app.sort_mode, SortMode::Duration);

        // Every mode shows every NON-LEAF table exactly once (v0.15: leaf
        // partitions stay collapsed by default).
        let mut seen = app.schema_row_order.clone();
        seen.sort_unstable();
        let schema = app.snapshot.schema.as_deref().expect("schema");
        let mut expected: Vec<usize> = (0..schema.tables.len())
            .filter(|&i| !schema.tables[i].is_partition)
            .collect();
        expected.sort_unstable();
        assert_eq!(seen, expected);
    }

    /// v0.15's partition collapsing: leaves hidden by default, `p` reveals
    /// them, `p` again re-collapses — and the parent's aggregate row is
    /// ALWAYS visible either way (it is not itself a leaf).
    #[test]
    fn partition_leaves_are_collapsed_by_default_and_p_toggles_them() {
        let mut app = App::new();
        app.active_tab = Tab::SchemaLens;

        let schema = app.snapshot.schema.as_deref().expect("mock schema");
        let leaf_count = schema.tables.iter().filter(|t| t.is_partition).count();
        assert!(leaf_count > 0, "mock must fixture at least one leaf");
        let parent_oid = schema
            .tables
            .iter()
            .find(|t| t.partition_count.is_some())
            .expect("mock must fixture a partition parent")
            .oid;

        // Collapsed by default: no leaf is reachable via the display order.
        assert!(!app.schema_show_partitions);
        let visible_leaves = |app: &App| {
            let schema = app.snapshot.schema.as_deref().expect("schema");
            app.schema_row_order
                .iter()
                .filter(|&&i| schema.tables[i].is_partition)
                .count()
        };
        assert_eq!(visible_leaves(&app), 0);
        // The parent itself IS visible (it is not a leaf).
        assert!(
            app.schema_row_order.iter().any(|&i| schema.tables[i].oid == parent_oid),
            "the parent's aggregate row must stay visible while collapsed"
        );

        update(&mut app, press(KeyCode::Char('p')));
        assert!(app.schema_show_partitions);
        assert_eq!(visible_leaves(&app), leaf_count);

        update(&mut app, press(KeyCode::Char('p')));
        assert!(!app.schema_show_partitions);
        assert_eq!(visible_leaves(&app), 0);
    }

    /// `p` is inert outside the Schema Lens Tables view (Vacuum sub-view,
    /// or any other lens) — same "scoped to the right sub-view" contract
    /// `/` and `s` already follow.
    #[test]
    fn p_is_inert_outside_schema_tables_view() {
        let mut app = App::new();
        app.active_tab = Tab::MicroLens;
        update(&mut app, press(KeyCode::Char('p')));
        assert!(!app.schema_show_partitions);

        app.active_tab = Tab::SchemaLens;
        update(&mut app, press(KeyCode::Char('v')));
        assert_eq!(app.schema_view, SchemaView::Vacuum);
        update(&mut app, press(KeyCode::Char('p')));
        assert!(!app.schema_show_partitions, "Vacuum sub-view has no partitions toggle");
    }

    /// v0.15: filtering by name matches the parent AND, once expanded, its
    /// leaves — never just the currently visible subset.
    #[test]
    fn schema_filter_matches_parent_and_leaves_when_expanded() {
        let mut app = App::new();
        app.active_tab = Tab::SchemaLens;
        update(&mut app, press(KeyCode::Char('/')));
        type_str(&mut app, "events_by_month");
        update(&mut app, press(KeyCode::Enter));

        let schema = app.snapshot.schema.as_deref().expect("schema").clone();
        // Collapsed: only the parent row matches (leaves are filtered out
        // regardless of whether their name matches).
        assert_eq!(app.schema_row_order.len(), 1);
        assert_eq!(schema.tables[app.schema_row_order[0]].partition_count, Some(3));

        // Expand: every leaf whose name matches the same needle joins the
        // parent in the display order.
        update(&mut app, press(KeyCode::Char('p')));
        let matched_leaves = app
            .schema_row_order
            .iter()
            .filter(|&&i| schema.tables[i].is_partition)
            .count();
        assert!(matched_leaves > 0, "expanded view must surface matching leaves too");
    }

    #[test]
    fn schema_lens_has_its_own_selection_and_detail() {
        let mut app = App::new();
        for _ in 0..4 {
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
    fn uppercase_b_requests_schema_recollection_from_any_lens() {
        let mut app = App::new();
        assert_eq!(app.schema_refresh_requests, 0);

        // Macro Lens: B counts (documented decision: works from any lens).
        update(&mut app, press(KeyCode::Char('B')));
        assert_eq!(app.schema_refresh_requests, 1);

        // Schema Lens: B keeps counting; lowercase b jumps to BlocksLens.
        for _ in 0..4 {
            update(&mut app, press(KeyCode::Tab));
        }
        assert_eq!(app.active_tab, Tab::SchemaLens);
        update(&mut app, press(KeyCode::Char('B')));
        assert_eq!(app.schema_refresh_requests, 2);
        assert!(!app.should_quit);
    }

    #[test]
    fn shift_r_and_ctrl_r_toggle_recording() {
        let mut app = App::new();
        let test_dir = std::env::current_dir().unwrap().join("target/test_state_rec");
        app.state_dir = Some(test_dir);
        assert!(app.recording.is_none());

        // Shift+R starts recording
        update(&mut app, press(KeyCode::Char('R')));
        assert!(app.recording.is_some());
        assert_eq!(app.recording.as_ref().unwrap().writer.frame_count(), 1);

        // Incoming snapshot appends to recording
        let next_snap = Arc::new(DbSnapshot::mock());
        update(&mut app, Action::Snapshot(next_snap));
        assert_eq!(app.recording.as_ref().unwrap().writer.frame_count(), 2);

        // Shift+R stops recording and queues path to clipboard
        update(&mut app, press(KeyCode::Char('R')));
        assert!(app.recording.is_none());
        assert!(app.clipboard_request.is_some());

        // Ctrl+R starts recording again
        update(
            &mut app,
            Action::Key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL)),
        );
        assert!(app.recording.is_some());
        // Clean up: stop recording
        update(&mut app, press(KeyCode::Char('R')));
        assert!(app.recording.is_none());
    }

    #[test]
    fn e_exports_snapshot_bookmark() {
        let mut app = App::new();
        let test_dir = std::env::current_dir().unwrap().join("target/test_state_exp");
        app.state_dir = Some(test_dir);
        assert!(app.clipboard_request.is_none());

        update(&mut app, press(KeyCode::Char('E')));
        assert!(app.clipboard_request.is_some());
        let path = app.clipboard_request.unwrap();
        assert!(path.ends_with(".json"));
        assert!(path.contains("snapshot-"));
    }

    #[test]
    fn replay_mode_navigation_and_playback_controls() {
        let mut app = App::new();
        let frame1 = Arc::new(DbSnapshot::mock());
        let frame2 = Arc::new(DbSnapshot::mock());
        let frame3 = Arc::new(DbSnapshot::mock());
        let frames = vec![frame1, frame2, frame3];

        app.replay_state = Some(ReplayState {
            frames: frames.clone(),
            current_idx: 0,
            is_paused: false,
            speed: 1.0,
            loop_playback: false,
            source_path: PathBuf::from("test.jsonl"),
            last_frame_time: Instant::now(),
        });

        // Space pauses playback
        update(&mut app, press(KeyCode::Char(' ')));
        assert!(app.replay_state.as_ref().unwrap().is_paused);

        // Right arrow steps forward
        update(&mut app, press(KeyCode::Right));
        assert_eq!(app.replay_state.as_ref().unwrap().current_idx, 1);

        // Right arrow steps forward again
        update(&mut app, press(KeyCode::Right));
        assert_eq!(app.replay_state.as_ref().unwrap().current_idx, 2);

        // Right arrow at end stays at end
        update(&mut app, press(KeyCode::Right));
        assert_eq!(app.replay_state.as_ref().unwrap().current_idx, 2);

        // Left arrow steps backward
        update(&mut app, press(KeyCode::Left));
        assert_eq!(app.replay_state.as_ref().unwrap().current_idx, 1);

        // Left arrow steps backward to start
        update(&mut app, press(KeyCode::Left));
        assert_eq!(app.replay_state.as_ref().unwrap().current_idx, 0);

        // Left arrow at start stays at 0
        update(&mut app, press(KeyCode::Left));
        assert_eq!(app.replay_state.as_ref().unwrap().current_idx, 0);

        // Advance to end (frame 2 of 3)
        update(&mut app, press(KeyCode::Right));
        update(&mut app, press(KeyCode::Right));
        assert_eq!(app.replay_state.as_ref().unwrap().current_idx, 2);
        assert!(app.replay_state.as_ref().unwrap().is_paused);

        // Space at the end restarts from frame 0 and unpauses
        update(&mut app, press(KeyCode::Char(' ')));
        assert_eq!(app.replay_state.as_ref().unwrap().current_idx, 0);
        assert!(!app.replay_state.as_ref().unwrap().is_paused);

        // Space in the middle pauses without rewinding
        update(&mut app, press(KeyCode::Char(' ')));
        assert!(app.replay_state.as_ref().unwrap().is_paused);
        assert_eq!(app.replay_state.as_ref().unwrap().current_idx, 0);

        // ] increases speed
        update(&mut app, press(KeyCode::Char(']')));
        assert!((app.replay_state.as_ref().unwrap().speed - 2.0).abs() < f64::EPSILON);

        // [ decreases speed
        update(&mut app, press(KeyCode::Char('[')));
        assert!((app.replay_state.as_ref().unwrap().speed - 1.0).abs() < f64::EPSILON);

        // In replay mode, B does not bump schema_refresh_requests
        update(&mut app, press(KeyCode::Char('B')));
        assert_eq!(app.schema_refresh_requests, 0);

        // In replay mode, recording is disabled
        update(&mut app, press(KeyCode::Char('R')));
        assert!(app.recording.is_none());
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

    /// App on the Query Lens (six Tabs from Macro: Micro, Blocks,
    /// Replication, Schema, Index, Query).
    fn query_lens_app() -> App {
        let mut app = App::new();
        for _ in 0..6 {
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

        // s → temp-spill descending (v0.14: the heaviest spiller first).
        update(&mut app, press(KeyCode::Char('s')));
        assert_eq!(app.statements_sort_mode, StatementsSortMode::Temp);
        let temp = statements_displayed(&app, |s| s.temp_blks_written);
        assert!(temp.windows(2).all(|w| w[0] >= w[1]));
        assert!(temp[0] > 0, "the mock's heavy spiller must sort first");

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
            KeyCode::Char('B'),
            KeyCode::Char('R'),
            KeyCode::Char('E'),
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

        // Replication Lens: inert too.
        app.active_tab = Tab::ReplicationLens;
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
    fn b_still_counts_requests_while_paused() {
        let mut app = App::new();
        update(&mut app, press(KeyCode::Char(' ')));
        update(&mut app, press(KeyCode::Char('B')));
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

    // --- v0.16: `y` copy-to-clipboard --------------------------------------

    #[test]
    fn y_queues_the_selected_micro_lens_row_query_and_is_free_at_top_level() {
        let mut app = App::new();
        app.active_tab = Tab::MicroLens;
        let expected = app.selected_row().expect("mock has rows").query.clone();
        update(&mut app, press(KeyCode::Char('y')));
        assert_eq!(app.clipboard_request, Some(expected));
        // `y` only queues — it never quits, never opens/closes anything.
        assert!(!app.should_quit);
    }

    #[test]
    fn y_queues_the_full_query_lens_statement_text() {
        let mut app = App::new();
        app.active_tab = Tab::QueryLens;
        let expected = app.selected_statement().expect("mock has statements").query.clone();
        update(&mut app, press(KeyCode::Char('y')));
        assert_eq!(app.clipboard_request, Some(expected));
    }

    #[test]
    fn y_queues_the_index_definition_on_the_index_lens() {
        let mut app = App::new();
        app.active_tab = Tab::IndexLens;
        let expected = app.selected_index().expect("mock has indexes").indexdef.clone();
        update(&mut app, press(KeyCode::Char('y')));
        assert_eq!(app.clipboard_request, Some(expected));
    }

    #[test]
    fn y_queues_the_qualified_table_name_on_the_schema_lens() {
        let mut app = App::new();
        app.active_tab = Tab::SchemaLens;
        let table = app.selected_table().expect("mock has tables");
        let expected = format!("{}.{}", table.schema, table.name);
        update(&mut app, press(KeyCode::Char('y')));
        assert_eq!(app.clipboard_request, Some(expected));
    }

    /// With the Schema Lens structure detail open on the selected table, `y`
    /// copies the column list instead of the bare qualified name.
    #[test]
    fn y_queues_the_column_list_when_the_schema_structure_detail_is_open() {
        let mut app = App::new();
        app.active_tab = Tab::SchemaLens;
        // Select the table `TableDetail::mock()` describes (order_items) so
        // the oid actually matches.
        let idx = app
            .schema_row_order
            .iter()
            .position(|&i| app.snapshot.schema.as_ref().unwrap().tables[i].name == "order_items")
            .expect("mock has order_items");
        app.schema_table_state.select(Some(idx));
        app.detail_open = true;
        update(&mut app, press(KeyCode::Char('y')));
        let text = app.clipboard_request.expect("queued");
        assert!(text.contains("id bigint NOT NULL"), "{text}");
        assert!(text.lines().count() > 1, "one line per column: {text}");
    }

    #[test]
    fn y_shows_a_calm_toast_when_there_is_nothing_to_copy() {
        let mut app = App::new();
        app.active_tab = Tab::ReplicationLens; // no clipboard content defined
        update(&mut app, press(KeyCode::Char('y')));
        assert!(app.clipboard_request.is_none());
        let feedback = app.admin_feedback.as_ref().expect("toast shown");
        assert!(feedback.text.contains("nothing to copy"), "{}", feedback.text);
        assert!(!feedback.error);
    }

    /// `y` inside the admin confirm modal still means "confirm", never
    /// "copy" — the modal's own keymap intercepts it before `handle_key`'s
    /// top-level match is ever reached.
    #[test]
    fn y_inside_the_confirm_modal_still_confirms_not_copies() {
        let mut app = App::new();
        app.active_tab = Tab::MicroLens;
        update(&mut app, press(KeyCode::Char('c')));
        assert!(app.confirm.is_some(), "modal open");
        update(&mut app, press(KeyCode::Char('y')));
        assert!(app.confirm.is_none(), "confirmed and closed");
        assert!(!app.pending_admin.is_empty(), "the cancel command was queued");
        assert!(app.clipboard_request.is_none(), "not a copy request");
    }

    #[test]
    fn clipboard_copied_action_sets_admin_feedback() {
        let mut app = App::new();
        update(
            &mut app,
            Action::ClipboardCopied {
                text: "sent to clipboard (OSC 52) \u{2014} copied 8 chars".to_string(),
            },
        );
        let feedback = app.admin_feedback.as_ref().expect("feedback set");
        assert!(feedback.text.contains("copied 8 chars"));
        assert!(!feedback.error);
    }

    #[test]
    fn tab_9_jumps_to_records_lens_and_cycles() {
        let mut app = App::new();
        assert_eq!(app.active_tab, Tab::MacroLens);
        update(&mut app, press(KeyCode::Char('9')));
        assert_eq!(app.active_tab, Tab::RecordsLens);
        assert_eq!(app.active_tab.index(), 8);

        // Next tab cycles back to MacroLens
        assert_eq!(app.active_tab.next(), Tab::MacroLens);
        // Prev tab goes to ProgressLens
        assert_eq!(app.active_tab.prev(), Tab::ProgressLens);
    }

    #[test]
    fn records_sort_and_filter_test() {
        let mut app = App::new();
        app.records = vec![
            pg_lens_core::recording::RecordingEntry {
                path: PathBuf::from("/tmp/rec-prod-20260907_100000.jsonl"),
                filename: "rec-prod-20260907_100000.jsonl".to_string(),
                target: "prod".to_string(),
                kind: pg_lens_core::recording::RecordingKind::Recording,
                size_bytes: 5_000,
                started_at_secs: Some(100),
                ended_at_secs: None,
                started_at: Some("2026-09-07 10:00:00".to_string()),
                ended_at: None,
                duration_secs: None,
                frame_count: Some(50),
                is_active: false,
            },
            pg_lens_core::recording::RecordingEntry {
                path: PathBuf::from("/tmp/snapshot-staging-20260907_120000.json"),
                filename: "snapshot-staging-20260907_120000.json".to_string(),
                target: "staging".to_string(),
                kind: pg_lens_core::recording::RecordingKind::Bookmark,
                size_bytes: 20_000,
                started_at_secs: Some(200),
                ended_at_secs: Some(200),
                started_at: Some("2026-09-07 12:00:00".to_string()),
                ended_at: Some("2026-09-07 12:00:00".to_string()),
                duration_secs: Some(0),
                frame_count: Some(1),
                is_active: false,
            },
            pg_lens_core::recording::RecordingEntry {
                path: PathBuf::from("/tmp/rec-analytics-20260907_080000.jsonl"),
                filename: "rec-analytics-20260907_080000.jsonl".to_string(),
                target: "analytics".to_string(),
                kind: pg_lens_core::recording::RecordingKind::Recording,
                size_bytes: 1_000,
                started_at_secs: Some(50),
                ended_at_secs: None,
                started_at: Some("2026-09-07 08:00:00".to_string()),
                ended_at: None,
                duration_secs: None,
                frame_count: Some(10),
                is_active: false,
            },
        ];

        // Default sort: StartedDesc (200, 100, 50) => indices 1, 0, 2
        app.records_sort_mode = RecordsSortMode::StartedDesc;
        resort_records(&mut app);
        assert_eq!(app.records_row_order, vec![1, 0, 2]);

        // StartedAsc => 2, 0, 1
        app.records_sort_mode = RecordsSortMode::StartedAsc;
        resort_records(&mut app);
        assert_eq!(app.records_row_order, vec![2, 0, 1]);

        // SizeDesc (20000, 5000, 1000) => 1, 0, 2
        app.records_sort_mode = RecordsSortMode::SizeDesc;
        resort_records(&mut app);
        assert_eq!(app.records_row_order, vec![1, 0, 2]);

        // NameAsc ("rec-analytics", "rec-prod", "snapshot-staging") => 2, 0, 1
        app.records_sort_mode = RecordsSortMode::NameAsc;
        resort_records(&mut app);
        assert_eq!(app.records_row_order, vec![2, 0, 1]);

        // Filter for "analytics"
        app.records_filter = "analytics".to_string();
        resort_records(&mut app);
        assert_eq!(app.records_row_order, vec![2]);

        // Filter for bookmark
        app.records_filter = "bookmark".to_string();
        resort_records(&mut app);
        assert_eq!(app.records_row_order, vec![1]);
    }

    #[test]
    fn records_clipboard_and_delete_confirm() {
        let mut app = App::new();
        app.active_tab = Tab::RecordsLens;
        app.records = vec![pg_lens_core::recording::RecordingEntry {
            path: PathBuf::from("/tmp/rec-prod-20260907_100000.jsonl"),
            filename: "rec-prod-20260907_100000.jsonl".to_string(),
            target: "prod".to_string(),
            kind: pg_lens_core::recording::RecordingKind::Recording,
            size_bytes: 5_000,
            started_at_secs: Some(100),
            ended_at_secs: None,
            started_at: Some("2026-09-07 10:00:00".to_string()),
            ended_at: None,
            duration_secs: None,
            frame_count: Some(50),
            is_active: false,
        }];
        app.records_row_order = vec![0];
        app.records_table_state.select(Some(0));

        // 'y' copies the file path
        update(&mut app, press(KeyCode::Char('y')));
        assert_eq!(
            app.clipboard_request,
            Some("/tmp/rec-prod-20260907_100000.jsonl".to_string())
        );

        // 'Delete' prompts delete confirmation
        update(&mut app, press(KeyCode::Delete));
        assert!(app.delete_record_target.is_some());
        assert_eq!(
            app.delete_record_target.as_ref().unwrap().filename,
            "rec-prod-20260907_100000.jsonl"
        );

        // 'n' cancels delete confirmation
        update(&mut app, press(KeyCode::Char('n')));
        assert!(app.delete_record_target.is_none());

        // 'x' also prompts delete confirmation
        update(&mut app, press(KeyCode::Char('x')));
        assert!(app.delete_record_target.is_some());

        // Esc cancels delete confirmation
        update(&mut app, press(KeyCode::Esc));
        assert!(app.delete_record_target.is_none());
    }

    #[test]
    fn schema_sequences_subview_toggle_and_filter() {
        let mut app = App::new();
        app.active_tab = Tab::SchemaLens;
        assert_eq!(app.schema_view, SchemaView::Tables);

        // Pressing 'S' switches to Sequences view
        update(&mut app, press(KeyCode::Char('S')));
        assert_eq!(app.schema_view, SchemaView::Sequences);

        // Mock has sequences; sequences_row_order should be populated and sorted by % used desc
        assert!(!app.sequences_row_order.is_empty());

        // Pressing 'S' again toggles back to Tables
        update(&mut app, press(KeyCode::Char('S')));
        assert_eq!(app.schema_view, SchemaView::Tables);

        // Pressing 'S' to go back to Sequences
        update(&mut app, press(KeyCode::Char('S')));
        assert_eq!(app.schema_view, SchemaView::Sequences);

        // Pressing Esc in Sequences view returns to Tables view
        update(&mut app, press(KeyCode::Esc));
        assert_eq!(app.schema_view, SchemaView::Tables);

        // Switch back to Sequences and filter with '/'
        update(&mut app, press(KeyCode::Char('S')));
        assert_eq!(app.schema_view, SchemaView::Sequences);

        update(&mut app, press(KeyCode::Char('/')));
        assert!(app.schema_filter_editing);
        update(&mut app, press(KeyCode::Char('o')));
        update(&mut app, press(KeyCode::Char('r')));
        update(&mut app, press(KeyCode::Char('d')));
        update(&mut app, press(KeyCode::Enter));
        assert!(!app.schema_filter_editing);
        assert_eq!(app.schema_filter, "ord");

        // Clear filter with '\'
        update(&mut app, press(KeyCode::Char('\\')));
        assert!(app.schema_filter.is_empty());
    }

    #[test]
    fn cross_lens_jump_from_table_to_index_lens() {
        let mut app = App::new();
        app.active_tab = Tab::SchemaLens;
        assert_eq!(app.schema_view, SchemaView::Tables);

        let selected = app.selected_table().cloned().expect("selected table");
        let table_name = selected.name.clone();

        // Pressing 'i' jumps from selected table to IndexLens
        update(&mut app, press(KeyCode::Char('i')));
        assert_eq!(app.active_tab, Tab::IndexLens);
        assert_eq!(app.previous_tab, Some(Tab::SchemaLens));
        assert_eq!(app.index_filter, table_name);

        // Filtered indexes should match table name
        if let Some(schema) = app.snapshot.schema.as_deref() {
            for &idx_i in &app.index_row_order {
                assert!(
                    schema.indexes[idx_i]
                        .table
                        .to_lowercase()
                        .contains(&table_name.to_lowercase())
                        || schema.indexes[idx_i]
                            .name
                            .to_lowercase()
                            .contains(&table_name.to_lowercase())
                );
            }
        }

        // Backspace returns to SchemaLens
        update(&mut app, press(KeyCode::Backspace));
        assert_eq!(app.active_tab, Tab::SchemaLens);

        // Jumping again and using Esc to return
        update(&mut app, press(KeyCode::Char('i')));
        assert_eq!(app.active_tab, Tab::IndexLens);
        assert_eq!(app.index_filter, table_name);

        update(&mut app, press(KeyCode::Esc));
        assert_eq!(app.active_tab, Tab::SchemaLens);
        assert!(app.index_filter.is_empty());
    }

    #[test]
    fn index_lens_filter_and_clear() {
        let mut app = App::new();
        app.active_tab = Tab::IndexLens;
        assert!(app.index_filter.is_empty());

        // '/' begins editing index filter
        update(&mut app, press(KeyCode::Char('/')));
        assert!(app.index_filter_editing);

        update(&mut app, press(KeyCode::Char('p')));
        update(&mut app, press(KeyCode::Char('k')));
        assert_eq!(app.index_filter, "pk");

        // Esc cancels and reverts
        update(&mut app, press(KeyCode::Esc));
        assert!(!app.index_filter_editing);
        assert!(app.index_filter.is_empty());

        // Type and commit with Enter
        update(&mut app, press(KeyCode::Char('/')));
        update(&mut app, press(KeyCode::Char('p')));
        update(&mut app, press(KeyCode::Char('k')));
        update(&mut app, press(KeyCode::Enter));
        assert!(!app.index_filter_editing);
        assert_eq!(app.index_filter, "pk");

        // '\' clears committed filter
        update(&mut app, press(KeyCode::Char('\\')));
        assert!(app.index_filter.is_empty());
    }
}
