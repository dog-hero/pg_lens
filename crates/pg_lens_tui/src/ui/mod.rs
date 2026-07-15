//! View layer: pure, synchronous rendering functions. No I/O, ever.

pub mod format;
mod confirm;
mod macro_lens;
mod micro_lens;
mod picker;
mod schema_lens;
mod splash;
mod sql;
mod style;

use pg_lens_core::PollerStatus;
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Style, Stylize},
    text::{Line, Span},
    widgets::{Paragraph, Tabs},
};

use crate::app::{App, Tab};

/// Root layout: header / tabs / [status banner] / body / statusbar. The
/// banner row collapses to zero height while the poller is healthy.
///
/// Exception: while no real data has EVER arrived (`App::show_splash`), a
/// full-screen connection splash renders instead of the dashboard — empty
/// chrome with zeroed vitals looks broken. After the first Ok snapshot the
/// dashboard takes over permanently (disconnects show the banner, as ever).
pub fn draw(app: &mut App, frame: &mut Frame) {
    // Startup service picker: a pre-poller mode of its own — neither the
    // splash nor the dashboard makes sense before a connection is chosen.
    if app.picker.is_some() {
        picker::draw(app, frame);
        return;
    }
    if app.show_splash() {
        splash::draw(app, frame);
        return;
    }
    let banner_height = match app.snapshot.status {
        PollerStatus::Ok => 0,
        PollerStatus::Connecting | PollerStatus::Error(_) => 1,
    };
    // Transient admin-action feedback ("cancel sent…" / outcome): its own
    // one-line row under the poller banner, collapsing to zero when absent.
    let feedback_height = u16::from(app.admin_feedback.is_some());
    let [header_area, tabs_area, banner_area, feedback_area, body_area, statusbar_area] =
        Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(banner_height),
            Constraint::Length(feedback_height),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .areas(frame.area());

    draw_header(app, frame, header_area);
    draw_tabs(app, frame, tabs_area);
    if banner_height > 0 {
        draw_status_banner(app, frame, banner_area);
    }
    if feedback_height > 0 {
        draw_admin_feedback(app, frame, feedback_area);
    }
    match app.active_tab {
        Tab::MacroLens => macro_lens::draw(app, frame, body_area),
        Tab::MicroLens => micro_lens::draw(app, frame, body_area),
        Tab::SchemaLens => schema_lens::draw(app, frame, body_area),
    }
    draw_statusbar(app, frame, statusbar_area);
    // The admin confirmation modal draws over everything else, last.
    if app.confirm.is_some() {
        confirm::draw(app, frame);
    }
}

/// One line of admin-action feedback; loud red for failures (including the
/// returned-false privilege case), green for successes/acks. Fades on its
/// own: `update()` clears it ~10s after it was set (tick-based).
fn draw_admin_feedback(app: &App, frame: &mut Frame, area: Rect) {
    let Some(feedback) = app.admin_feedback.as_ref() else {
        return;
    };
    let style = if feedback.error {
        Style::new().fg(Color::White).bg(Color::Red).bold()
    } else {
        Style::new().fg(Color::Green).bold()
    };
    let line = Line::from(format!(" {}", feedback.text)).style(style);
    frame.render_widget(Paragraph::new(line), area);
}

/// Poller health banner: loud on error (last good data stays on screen
/// underneath it), quiet while the first connection is still in flight.
fn draw_status_banner(app: &App, frame: &mut Frame, area: Rect) {
    let line = match &app.snapshot.status {
        PollerStatus::Ok => return,
        PollerStatus::Connecting => Line::from(" connecting to PostgreSQL\u{2026}").dim(),
        PollerStatus::Error(msg) => {
            Line::from(format!(" DB error: {msg} \u{2014} showing last known data"))
                .style(Style::new().fg(Color::White).bg(Color::Red).bold())
        }
    };
    frame.render_widget(Paragraph::new(line), area);
}

fn draw_header(app: &App, frame: &mut Frame, area: Rect) {
    let vitals = &app.snapshot.vitals;
    let header = Line::from(format!(
        " pg_lens v{} \u{2502} PG {} @ {} \u{2502} up {} \u{2502} {}/{} conns",
        env!("CARGO_PKG_VERSION"),
        vitals.server_version,
        app.host,
        format::human_uptime(vitals.uptime_secs),
        vitals.connections_total,
        vitals.max_connections,
    ))
    .bold();
    // Right side: the pause control. The hint lives HERE, not in the
    // statusbar — that bar was fought down to a ~4-column margin at 120
    // cols and cannot take another 15 characters on any lens. While frozen
    // the hint grows into the loud PAUSED indicator (yellow: the staleness
    // that follows is deliberate, not a fault).
    let pause = pause_indicator(app);
    let pause_width = pause.width() as u16;
    let [left_area, pause_area] =
        Layout::horizontal([Constraint::Min(0), Constraint::Length(pause_width)]).areas(area);
    frame.render_widget(Paragraph::new(header), left_area);
    frame.render_widget(Paragraph::new(pause), pause_area);
}

/// The header's right-side pause control: `Space: pause` while live,
/// `▮▮ PAUSED · Space: resume` (yellow, loud) while frozen.
fn pause_indicator(app: &App) -> Line<'static> {
    let mut spans: Vec<Span> = Vec::new();
    if app.paused {
        spans.push(Span::styled(
            "\u{25ae}\u{25ae} PAUSED",
            Style::new().fg(Color::Yellow).bold(),
        ));
        spans.push(Span::styled(" \u{b7} ", style::label_style()));
        let [k, d] = style::hint("Space", ": resume");
        spans.push(k);
        spans.push(d);
    } else {
        let [k, d] = style::hint("Space", ": pause");
        spans.push(k);
        spans.push(d);
    }
    spans.push(Span::raw(" "));
    Line::from(spans)
}

fn draw_tabs(app: &App, frame: &mut Frame, area: Rect) {
    let tabs = Tabs::new(Tab::TITLES)
        .select(app.active_tab.index())
        .style(Style::new().dim())
        .highlight_style(Style::new().not_dim().bold().underlined());
    frame.render_widget(tabs, area);
}

fn draw_statusbar(app: &App, frame: &mut Frame, area: Rect) {
    // Staleness: seconds since the last Action::Snapshot reached update().
    // Rendered every tick, so it counts up between snapshots.
    let staleness = match app.last_snapshot_at {
        Some(at) => format!("{}s ago", at.elapsed().as_secs()),
        None => "waiting".to_string(),
    };
    // Row counter and sort label follow the active lens (the Schema Lens
    // keeps its own selection and sort mode); `R: recollect` is a Schema
    // Lens hint (the key itself works from any lens).
    let (selected, len, sort_label, schema_extra) = match app.active_tab {
        Tab::SchemaLens => (
            app.schema_table_state.selected(),
            app.snapshot.schema.as_deref().map_or(0, |s| s.tables.len()),
            app.schema_sort_mode.label(),
            true,
        ),
        _ => (
            app.table_state.selected(),
            app.snapshot.activity.len(),
            app.sort_mode.label(),
            false,
        ),
    };
    let row = match (selected, len) {
        (Some(i), len) if len > 0 => format!("{}/{len}", i + 1),
        _ => "-".to_string(),
    };
    // Keybinding letters in accent, descriptions dim (style::hint), the
    // separators dim — same text as ever, only the styling changed.
    let sep = Span::styled(" \u{2502} ", style::label_style());
    let mut spans: Vec<Span> = vec![Span::raw(" ")];
    let push_hint = |spans: &mut Vec<Span<'static>>, key: &str, desc: String, lead: bool| {
        if lead {
            spans.push(sep.clone());
        }
        let [k, d] = style::hint(key, desc);
        spans.push(k);
        spans.push(d);
    };
    push_hint(&mut spans, "q/Esc", ": quit".into(), false);
    push_hint(&mut spans, "Tab", ": lens".into(), true);
    push_hint(&mut spans, "j/k", format!(": row {row}"), true);
    // The Micro Lens trades the Enter hint for the admin keys (the open
    // panel titles itself "Enter/Esc: close") — the bar must fit 120 cols.
    if app.active_tab == Tab::MicroLens {
        push_hint(&mut spans, "c", ": cancel".into(), true);
        spans.push(Span::styled(" \u{b7} ", style::label_style()));
        let [k, d] = style::hint("K", ": kill");
        spans.push(k);
        spans.push(d);
    } else {
        push_hint(&mut spans, "Enter", ": detail".into(), true);
    }
    push_hint(&mut spans, "s", format!(": sort={sort_label}"), true);
    if schema_extra {
        push_hint(&mut spans, "R", ": recollect".into(), true);
    }
    push_hint(
        &mut spans,
        "+/-",
        format!(": refresh={:.1}s", app.refresh_interval.as_secs_f64()),
        true,
    );
    spans.push(sep.clone());
    // While paused the staleness turns yellow: it grows on purpose (the
    // freeze holds `last_snapshot_at` still) and doubles as the "how old is
    // this frozen picture" readout next to the header's PAUSED indicator.
    let staleness_style = if app.paused {
        Style::new().fg(Color::Yellow).bold()
    } else {
        style::label_style()
    };
    spans.push(Span::styled(format!("data: {staleness}"), staleness_style));
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use ratatui::{Terminal, backend::TestBackend};

    fn render(app: &mut App) -> String {
        let backend = TestBackend::new(120, 36);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        terminal.draw(|frame| draw(app, frame)).expect("draw");
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    #[test]
    fn macro_lens_renders_header_tabs_and_widgets() {
        let mut app = App::new();
        let screen = render(&mut app);
        assert!(screen.contains("pg_lens v"));
        assert!(screen.contains("Macro Lens"));
        assert!(screen.contains("Micro Lens"));
        assert!(screen.contains("Connections"));
        assert!(screen.contains("TPS"));
        assert!(screen.contains("q/Esc: quit"));
    }

    #[test]
    fn micro_lens_renders_activity_table() {
        let mut app = App::new();
        app.active_tab = Tab::MicroLens;
        let screen = render(&mut app);
        assert!(screen.contains("PID"));
        assert!(screen.contains("Wait"));
        assert!(screen.contains("Duration"));
        assert!(screen.contains("pgbench"));
    }

    #[test]
    fn schema_lens_renders_table_footer_and_markers() {
        let mut app = App::new();
        app.active_tab = Tab::SchemaLens;
        let screen = render(&mut app);
        // Columns of the S0-decision-3 spec.
        for header in ["Table", "Size", "Live", "Dead", "Bloat%", "Bloat", "Last AV", "Seq/Idx"] {
            assert!(screen.contains(header), "missing column {header}: {screen}");
        }
        // Mock rows, joined bloat, is_na marker, footer.
        assert!(screen.contains("public.order_items"));
        assert!(screen.contains("54.0%"), "red-tier bloat pct: {screen}");
        assert!(screen.contains("!!"), "red severity marker: {screen}");
        assert!(screen.contains("~?"), "is_na renders ~?, never a number");
        assert!(screen.contains("db: shop"), "footer names the database");
        assert!(screen.contains("ESTIMATED"), "estimate label is mandatory");
        assert!(screen.contains("R: recollect"));
    }

    #[test]
    fn schema_lens_without_collection_shows_placeholder() {
        use std::sync::Arc;

        let mut app = App::new();
        app.active_tab = Tab::SchemaLens;
        let mut snap = app.snapshot.as_ref().clone();
        snap.schema = None;
        crate::app::update(&mut app, crate::app::Action::Snapshot(Arc::new(snap)));
        let screen = render(&mut app);
        assert!(screen.contains("collecting schema stats"));
    }

    #[test]
    fn schema_lens_error_status_renders_inline_banner() {
        use std::sync::Arc;

        let mut app = App::new();
        app.active_tab = Tab::SchemaLens;
        let mut snap = app.snapshot.as_ref().clone();
        let mut schema = snap.schema.as_deref().expect("mock schema").clone();
        schema.status = pg_lens_core::SchemaStatus::Error("permission denied".to_string());
        snap.schema = Some(Arc::new(schema));
        crate::app::update(&mut app, crate::app::Action::Snapshot(Arc::new(snap)));
        let screen = render(&mut app);
        assert!(screen.contains("schema: permission denied"));
        assert!(screen.contains("showing last collection"));
        // Last data still rendered underneath.
        assert!(screen.contains("public.order_items"));
    }

    #[test]
    fn schema_detail_lists_the_tables_indexes() {
        let mut app = App::new();
        app.active_tab = Tab::SchemaLens;
        // Sort by dead tuples puts order_items (which owns the mock's only
        // index-bloat row) under the cursor at display index 0.
        crate::app::update(
            &mut app,
            crate::app::Action::Key(crossterm::event::KeyEvent::new(
                crossterm::event::KeyCode::Char('s'),
                crossterm::event::KeyModifiers::NONE,
            )),
        );
        app.detail_open = true;
        let screen = render(&mut app);
        assert!(screen.contains("Table \u{2014} public.order_items"));
        assert!(screen.contains("mod since analyze"));
        assert!(screen.contains("order_items_pkey"), "index bloat listed: {screen}");
        assert!(screen.contains("35.0%"), "index bloat pct shown");
    }

    #[test]
    fn error_status_renders_banner_and_keeps_data() {
        use std::sync::Arc;

        let mut app = App::new();
        // Real data arrived earlier (splash-splash gate: post-first-data
        // errors must render the banner over the dashboard, never the
        // splash) — simulate it through the real update path.
        crate::app::update(
            &mut app,
            crate::app::Action::Snapshot(Arc::new(pg_lens_core::DbSnapshot::mock())),
        );
        let mut snap = app.snapshot.as_ref().clone();
        snap.status = PollerStatus::Error("connection refused".to_string());
        app.snapshot = Arc::new(snap);

        let screen = render(&mut app);
        assert!(screen.contains("DB error: connection refused"));
        assert!(screen.contains("showing last known data"));
        // Last data still rendered underneath the banner.
        assert!(screen.contains("Connections"));
    }

    /// Pre-first-data Connecting: the splash replaces the dashboard.
    #[test]
    fn splash_renders_while_connecting_before_any_data() {
        use std::sync::Arc;

        let mut app = App::new();
        crate::app::update(
            &mut app,
            crate::app::Action::Snapshot(Arc::new(pg_lens_core::DbSnapshot::connecting())),
        );
        app.host = "postgres@db.internal:5432".to_string();
        let screen = render(&mut app);
        assert!(screen.contains("p g _ l e n s"), "wordmark: {screen}");
        assert!(screen.contains("connecting to"));
        assert!(screen.contains("postgres@db.internal:5432"));
        assert!(screen.contains("waiting for the first snapshot"));
        // Dashboard chrome must NOT render underneath.
        assert!(!screen.contains("Connections"));
        assert!(!screen.contains("Macro Lens"));
    }

    /// Pre-first-data Error: splash stays (poller retries with backoff),
    /// with the wrapped error text in a box plus the retry hint.
    #[test]
    fn splash_shows_wrapped_error_and_retry_hint_before_any_data() {
        use std::sync::Arc;

        let mut app = App::new();
        let mut snap = pg_lens_core::DbSnapshot::connecting();
        // Long password_cmd-style stderr: must word-wrap inside the box.
        let msg = "password_cmd failed: `op read op://infra/pg/password` exited with status 1: \
                   [ERROR] 2026/07/14 could not resolve item op://infra/pg/password in vault";
        snap.status = PollerStatus::Error(msg.to_string());
        crate::app::update(&mut app, crate::app::Action::Snapshot(Arc::new(snap)));

        let screen = render(&mut app);
        assert!(screen.contains("connection error"));
        assert!(screen.contains("password_cmd failed"));
        // The tail survived the wrap (nothing overflowed off-screen).
        assert!(screen.contains("in vault"), "wrapped tail visible: {screen}");
        assert!(screen.contains("connection failed"));
        assert!(screen.contains("retrying automatically \u{b7} q/Esc: quit"));
        assert!(!screen.contains("Macro Lens"), "no dashboard underneath");
    }

    /// The spinner glyph changes between ticks (animation proof at the
    /// TestBackend level; the PTY run proves it end-to-end).
    #[test]
    fn splash_spinner_advances_on_tick() {
        use std::sync::Arc;

        let mut app = App::new();
        crate::app::update(
            &mut app,
            crate::app::Action::Snapshot(Arc::new(pg_lens_core::DbSnapshot::connecting())),
        );
        let before = render(&mut app);
        crate::app::update(&mut app, crate::app::Action::Tick);
        let after = render(&mut app);
        assert_ne!(before, after, "tick must advance the spinner frame");
    }

    /// 80x24: splash renders without panicking, wordmark + hint intact.
    #[test]
    fn splash_fits_a_small_terminal() {
        use std::sync::Arc;

        let mut app = App::new();
        let mut snap = pg_lens_core::DbSnapshot::connecting();
        snap.status = PollerStatus::Error("connection refused".to_string());
        crate::app::update(&mut app, crate::app::Action::Snapshot(Arc::new(snap)));

        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        terminal.draw(|frame| draw(&mut app, frame)).expect("draw");
        let screen: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(screen.contains("p g _ l e n s"));
        assert!(screen.contains("connection refused"));
    }

    // --- admin actions -------------------------------------------------------

    fn press(app: &mut App, code: crossterm::event::KeyCode) {
        crate::app::update(
            app,
            crate::app::Action::Key(crossterm::event::KeyEvent::new(
                code,
                crossterm::event::KeyModifiers::NONE,
            )),
        );
    }

    #[test]
    fn micro_lens_statusbar_shows_the_admin_hints() {
        let mut app = App::new();
        press(&mut app, crossterm::event::KeyCode::Tab); // → Micro Lens
        let screen = render(&mut app);
        assert!(screen.contains("c: cancel"), "cancel hint: {screen}");
        assert!(screen.contains("K: kill"), "kill hint: {screen}");

        // Macro Lens: no admin hints (the keys are inert there).
        let mut app = App::new();
        let screen = render(&mut app);
        assert!(!screen.contains("c: cancel"));
        assert!(!screen.contains("K: kill"));
    }

    #[test]
    fn cancel_modal_renders_pid_target_and_key_hints() {
        let mut app = App::new();
        press(&mut app, crossterm::event::KeyCode::Tab);
        let row = app.selected_row().expect("selection");
        let (pid, user, db) = (row.pid, row.username.clone(), row.database.clone());
        press(&mut app, crossterm::event::KeyCode::Char('c'));
        let screen = render(&mut app);
        assert!(screen.contains("Cancel query"), "title: {screen}");
        assert!(screen.contains(&format!("Cancel query on PID {pid} ({user}@{db})?")));
        assert!(screen.contains("y: confirm"));
        assert!(screen.contains("n/Esc: abort"));
        assert!(!screen.contains("connection will be killed"));
    }

    #[test]
    fn terminate_modal_renders_the_kill_warning() {
        let mut app = App::new();
        press(&mut app, crossterm::event::KeyCode::Tab);
        let pid = app.selected_row().expect("selection").pid;
        press(&mut app, crossterm::event::KeyCode::Char('K'));
        let screen = render(&mut app);
        assert!(screen.contains("Terminate backend"), "title: {screen}");
        assert!(screen.contains(&format!("Terminate backend PID {pid}")));
        assert!(screen.contains("The connection will be killed."));
        assert!(screen.contains("y: confirm"));
    }

    #[test]
    fn admin_feedback_line_renders_and_fades() {
        let mut app = App::new();
        press(&mut app, crossterm::event::KeyCode::Tab);
        let pid = app.selected_row().expect("selection").pid;
        press(&mut app, crossterm::event::KeyCode::Char('c'));
        press(&mut app, crossterm::event::KeyCode::Char('y'));
        let screen = render(&mut app);
        assert!(!screen.contains("Cancel query on PID"), "modal closed");
        assert!(
            screen.contains(&format!("cancel sent to PID {pid}")),
            "sent feedback: {screen}"
        );

        // ~10s of ticks later the line is gone.
        for _ in 0..crate::app::ADMIN_FEEDBACK_TICKS {
            crate::app::update(&mut app, crate::app::Action::Tick);
        }
        let screen = render(&mut app);
        assert!(!screen.contains("cancel sent to PID"), "faded: {screen}");
    }

    // --- pause / freeze (Space) -----------------------------------------------

    #[test]
    fn header_hint_switches_between_pause_and_resume() {
        let mut app = App::new();
        let screen = render(&mut app);
        assert!(screen.contains("Space: pause"), "live hint: {screen}");
        assert!(!screen.contains("PAUSED"));

        press(&mut app, crossterm::event::KeyCode::Char(' '));
        let screen = render(&mut app);
        assert!(
            screen.contains("\u{25ae}\u{25ae} PAUSED"),
            "indicator: {screen}"
        );
        assert!(screen.contains("Space: resume"), "resume hint: {screen}");
        assert!(!screen.contains("Space: pause"));
        // The header's left side survived the split.
        assert!(screen.contains("pg_lens v"));
    }

    /// Frozen render proof at the TestBackend level: a new snapshot arriving
    /// while paused must not change a single cell except the tick-driven
    /// staleness counter (held constant here by not ticking).
    #[test]
    fn paused_screen_ignores_incoming_snapshots_until_resume() {
        use std::sync::Arc;

        let mut app = App::new();
        app.active_tab = Tab::MicroLens;
        press(&mut app, crossterm::event::KeyCode::Char(' '));
        let frozen = render(&mut app);

        // Distinguishable new snapshot: one activity row dropped.
        let mut snap = app.snapshot.as_ref().clone();
        snap.activity.truncate(2);
        crate::app::update(
            &mut app,
            crate::app::Action::Snapshot(Arc::new(snap)),
        );
        assert_eq!(render(&mut app), frozen, "display frozen while paused");

        // Resume: the parked snapshot applies (row counter now 6 → 2).
        press(&mut app, crossterm::event::KeyCode::Char(' '));
        let live = render(&mut app);
        assert_ne!(live, frozen);
        assert!(live.contains("row 1/2"), "pending applied: {live}");
        assert!(!live.contains("PAUSED"));
    }

    /// 80x24: the PAUSED indicator renders without panicking and without
    /// breaking the layout (dashboard chrome still present).
    #[test]
    fn paused_indicator_fits_a_small_terminal() {
        let mut app = App::new();
        press(&mut app, crossterm::event::KeyCode::Char(' '));

        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        terminal.draw(|frame| draw(&mut app, frame)).expect("draw");
        let screen: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(screen.contains("\u{25ae}\u{25ae} PAUSED"), "indicator: {screen}");
        assert!(screen.contains("pg_lens v"), "header left intact");
        assert!(screen.contains("Connections"), "body intact");
        assert!(screen.contains("q/Esc: quit"), "statusbar intact");
    }

    #[test]
    fn ok_status_renders_no_banner() {
        let mut app = App::new();
        let screen = render(&mut app);
        assert!(!screen.contains("DB error"));
        assert!(!screen.contains("connecting to PostgreSQL"));
    }

}
