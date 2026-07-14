//! TEA-style Model + update for the TUI.
//!
//! `App` is pure state; [`update`] is the only place that mutates it. The
//! `Action` enum is internal to this crate — `pg_lens_core` never sees it.

use std::sync::Arc;
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use pg_lens_core::{DbSnapshot, PollerStatus};
use ratatui::widgets::TableState;

/// Default poll interval; `+`/`-` move it in [`REFRESH_STEP`] steps.
pub const DEFAULT_REFRESH: Duration = Duration::from_secs(2);
const REFRESH_STEP: Duration = Duration::from_millis(500);
const REFRESH_MIN: Duration = Duration::from_millis(500);
const REFRESH_MAX: Duration = Duration::from_secs(10);

/// Which lens (tab) is on screen.
// The "Lens" postfix is the product vocabulary (Macro/Micro/Schema Lens),
// not naming noise — keep it despite clippy's shared-postfix lint.
#[allow(clippy::enum_variant_names)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tab {
    #[default]
    MacroLens,
    MicroLens,
    SchemaLens,
}

impl Tab {
    pub const TITLES: [&'static str; 3] = ["Macro Lens", "Micro Lens", "Schema Lens"];

    pub fn index(self) -> usize {
        match self {
            Tab::MacroLens => 0,
            Tab::MicroLens => 1,
            Tab::SchemaLens => 2,
        }
    }

    pub fn next(self) -> Self {
        match self {
            Tab::MacroLens => Tab::MicroLens,
            Tab::MicroLens => Tab::SchemaLens,
            Tab::SchemaLens => Tab::MacroLens,
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

/// Everything that can happen, funneled through one mpsc channel.
#[derive(Clone, Debug)]
pub enum Action {
    Key(KeyEvent),
    /// Terminal was resized. Carries no dimensions on purpose: the next
    /// `Terminal::draw` reads the real size from the frame; the action only
    /// exists to wake the loop up for an immediate redraw.
    Resize,
    Snapshot(Arc<DbSnapshot>),
    Tick,
    Quit,
}

/// The Model: pure state, no I/O.
#[derive(Debug)]
pub struct App {
    pub active_tab: Tab,
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
    /// Whether the detail panel is open (Micro Lens: full query of the
    /// selected session; Schema Lens: full vacuum/analyze stats + index
    /// bloat of the selected table). While open: `j`/`k` still move the
    /// selection (the panel follows it), `Enter`/`Esc` close the panel,
    /// `Tab` closes it and switches lens, `q` quits as always.
    pub detail_open: bool,
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
    pub should_quit: bool,
}

impl App {
    pub fn new() -> Self {
        let mut app = Self {
            active_tab: Tab::default(),
            snapshot: Arc::new(DbSnapshot::mock()),
            row_order: Vec::new(),
            sort_mode: SortMode::default(),
            table_state: TableState::default().with_selected(0),
            schema_row_order: Vec::new(),
            schema_sort_mode: SchemaSortMode::default(),
            schema_table_state: TableState::default().with_selected(0),
            detail_open: false,
            schema_refresh_requests: 0,
            host: "localhost".to_string(),
            refresh_interval: DEFAULT_REFRESH,
            last_snapshot_at: None,
            first_data_at: None,
            tick_count: 0,
            should_quit: false,
        };
        resort(&mut app);
        resort_schema(&mut app);
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
}

/// The single mutation point of the Model.
pub fn update(app: &mut App, action: Action) {
    match action {
        Action::Key(key) => handle_key(app, key),
        Action::Snapshot(snapshot) => {
            app.snapshot = snapshot;
            app.last_snapshot_at = Some(Instant::now());
            // First Ok snapshot ever: leave the splash for the dashboard,
            // permanently (see `App::show_splash`).
            if app.first_data_at.is_none() && matches!(app.snapshot.status, PollerStatus::Ok) {
                app.first_data_at = Some(Instant::now());
            }
            resort(app);
            resort_schema(app);
            clamp_selection(app);
        }
        // The next draw reads the new terminal size from the frame itself.
        Action::Resize => {}
        // Only effect: advance the splash spinner (and force a redraw).
        Action::Tick => app.tick_count = app.tick_count.wrapping_add(1),
        Action::Quit => app.should_quit = true,
    }
}

fn handle_key(app: &mut App, key: KeyEvent) {
    if key.kind != KeyEventKind::Press {
        return;
    }
    match key.code {
        KeyCode::Char('q') => app.should_quit = true,
        // Esc closes the detail panel when it is open; quits otherwise.
        KeyCode::Esc => {
            if app.detail_open {
                app.detail_open = false;
            } else {
                app.should_quit = true;
            }
        }
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.should_quit = true;
        }
        // Enter toggles the detail panel of the active lens's selected row
        // (Micro: session query; Schema: table stats + index bloat).
        KeyCode::Enter => {
            if app.detail_open {
                app.detail_open = false;
            } else if (app.active_tab == Tab::MicroLens && app.table_state.selected().is_some())
                || (app.active_tab == Tab::SchemaLens && app.selected_table().is_some())
            {
                app.detail_open = true;
            }
        }
        KeyCode::Tab => {
            app.detail_open = false;
            app.active_tab = app.active_tab.next();
        }
        KeyCode::Up | KeyCode::Char('k') => move_selection(app, -1),
        KeyCode::Down | KeyCode::Char('j') => move_selection(app, 1),
        // `s` cycles the sort of whichever lens is active (each keeps its
        // own mode, so tabbing away and back never loses the choice).
        KeyCode::Char('s') => {
            if app.active_tab == Tab::SchemaLens {
                app.schema_sort_mode = app.schema_sort_mode.next();
                resort_schema(app);
            } else {
                app.sort_mode = app.sort_mode.next();
                resort(app);
            }
        }
        // `R` (uppercase, deliberately distinct from the lowercase keys):
        // request an immediate schema re-collection. Allowed from any lens —
        // it is harmless, and the fresh data is ready when the user tabs in.
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

/// Moves the active lens's table selection by `delta`, saturating at both
/// ends (no wrap). The Macro Lens has no table; j/k default to the Micro
/// Lens cursor there (harmless, matches the pre-S3 behavior).
fn move_selection(app: &mut App, delta: i64) {
    let (state, len) = match app.active_tab {
        Tab::SchemaLens => (
            &mut app.schema_table_state,
            app.snapshot.schema.as_deref().map_or(0, |s| s.tables.len()),
        ),
        _ => (&mut app.table_state, app.snapshot.activity.len()),
    };
    move_state(state, len, delta);
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
    let len = app.snapshot.activity.len();
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

    let schema_len = app.snapshot.schema.as_deref().map_or(0, |s| s.tables.len());
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
}

/// Recomputes `row_order` from the current snapshot + sort mode. The view
/// renders rows in this order; the snapshot itself is never mutated.
fn resort(app: &mut App) {
    let rows = &app.snapshot.activity;
    let mut order: Vec<usize> = (0..rows.len()).collect();
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

/// Recomputes `schema_row_order` from the current snapshot + schema sort
/// mode (the Schema Lens twin of [`resort`]). Ties break by total size
/// descending, then schema.name ascending, so the order is deterministic.
fn resort_schema(app: &mut App) {
    let Some(schema) = app.snapshot.schema.as_deref() else {
        app.schema_row_order = Vec::new();
        return;
    };
    let rows = &schema.tables;
    let mut order: Vec<usize> = (0..rows.len()).collect();
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

    #[test]
    fn esc_quits() {
        let mut app = App::new();
        update(&mut app, press(KeyCode::Esc));
        assert!(app.should_quit);
    }

    #[test]
    fn tab_cycles_the_three_lenses() {
        let mut app = App::new();
        assert_eq!(app.active_tab, Tab::MacroLens);
        update(&mut app, press(KeyCode::Tab));
        assert_eq!(app.active_tab, Tab::MicroLens);
        update(&mut app, press(KeyCode::Tab));
        assert_eq!(app.active_tab, Tab::SchemaLens);
        update(&mut app, press(KeyCode::Tab));
        assert_eq!(app.active_tab, Tab::MacroLens);
        assert!(!app.should_quit);
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

        // ...the second one quits.
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
        assert_eq!(app.active_tab, Tab::SchemaLens);
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
        update(&mut app, press(KeyCode::Tab));
        update(&mut app, press(KeyCode::Tab)); // → Schema Lens

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
        update(&mut app, press(KeyCode::Tab));
        update(&mut app, press(KeyCode::Tab)); // → Schema Lens

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
        update(&mut app, press(KeyCode::Tab));
        update(&mut app, press(KeyCode::Tab));
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
