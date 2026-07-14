//! TEA-style Model + update for the TUI.
//!
//! `App` is pure state; [`update`] is the only place that mutates it. The
//! `Action` enum is internal to this crate — `pg_lens_core` never sees it.

use std::sync::Arc;
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use pg_lens_core::DbSnapshot;
use ratatui::widgets::TableState;

/// Default poll interval; `+`/`-` move it in [`REFRESH_STEP`] steps.
pub const DEFAULT_REFRESH: Duration = Duration::from_secs(2);
const REFRESH_STEP: Duration = Duration::from_millis(500);
const REFRESH_MIN: Duration = Duration::from_millis(500);
const REFRESH_MAX: Duration = Duration::from_secs(10);

/// Which lens (tab) is on screen.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tab {
    #[default]
    MacroLens,
    MicroLens,
}

impl Tab {
    pub const TITLES: [&'static str; 2] = ["Macro Lens", "Micro Lens"];

    pub fn index(self) -> usize {
        match self {
            Tab::MacroLens => 0,
            Tab::MicroLens => 1,
        }
    }

    pub fn next(self) -> Self {
        match self {
            Tab::MacroLens => Tab::MicroLens,
            Tab::MicroLens => Tab::MacroLens,
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
    /// Desired poll interval. Fase 2: state + statusbar only; wired into the
    /// live poller in Fase 3.
    pub refresh_interval: Duration,
    /// When the last `Action::Snapshot` arrived — drives the staleness
    /// indicator in the statusbar. `None` until the first snapshot.
    pub last_snapshot_at: Option<Instant>,
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
            refresh_interval: DEFAULT_REFRESH,
            last_snapshot_at: None,
            should_quit: false,
        };
        resort(&mut app);
        app
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

/// The single mutation point of the Model.
pub fn update(app: &mut App, action: Action) {
    match action {
        Action::Key(key) => handle_key(app, key),
        Action::Snapshot(snapshot) => {
            app.snapshot = snapshot;
            app.last_snapshot_at = Some(Instant::now());
            resort(app);
            clamp_selection(app);
        }
        // The next draw reads the new terminal size from the frame itself.
        Action::Resize => {}
        Action::Tick => {}
        Action::Quit => app.should_quit = true,
    }
}

fn handle_key(app: &mut App, key: KeyEvent) {
    if key.kind != KeyEventKind::Press {
        return;
    }
    match key.code {
        KeyCode::Char('q') | KeyCode::Esc => app.should_quit = true,
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.should_quit = true;
        }
        KeyCode::Tab => app.active_tab = app.active_tab.next(),
        KeyCode::Up | KeyCode::Char('k') => move_selection(app, -1),
        KeyCode::Down | KeyCode::Char('j') => move_selection(app, 1),
        KeyCode::Char('s') => {
            app.sort_mode = app.sort_mode.next();
            resort(app);
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

/// Moves the table selection by `delta`, saturating at both ends (no wrap).
fn move_selection(app: &mut App, delta: i64) {
    let len = app.snapshot.activity.len();
    if len == 0 {
        app.table_state.select(None);
        return;
    }
    let current = app.table_state.selected().unwrap_or(0).min(len - 1);
    let next = if delta < 0 {
        current.saturating_sub(delta.unsigned_abs() as usize)
    } else {
        (current + delta as usize).min(len - 1)
    };
    app.table_state.select(Some(next));
}

/// Keeps the selection valid after the row set changes size.
fn clamp_selection(app: &mut App) {
    let len = app.snapshot.activity.len();
    if len == 0 {
        app.table_state.select(None);
    } else {
        let clamped = app.table_state.selected().unwrap_or(0).min(len - 1);
        app.table_state.select(Some(clamped));
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
    fn tab_cycles_lenses() {
        let mut app = App::new();
        assert_eq!(app.active_tab, Tab::MacroLens);
        update(&mut app, press(KeyCode::Tab));
        assert_eq!(app.active_tab, Tab::MicroLens);
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
