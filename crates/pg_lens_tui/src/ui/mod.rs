//! View layer: pure, synchronous rendering functions. No I/O, ever.

pub mod format;
mod macro_lens;
mod micro_lens;
mod schema_lens;

use pg_lens_core::PollerStatus;
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Style, Stylize},
    text::Line,
    widgets::{Paragraph, Tabs},
};

use crate::app::{App, Tab};

/// Root layout: header / tabs / [status banner] / body / statusbar. The
/// banner row collapses to zero height while the poller is healthy.
pub fn draw(app: &mut App, frame: &mut Frame) {
    let banner_height = match app.snapshot.status {
        PollerStatus::Ok => 0,
        PollerStatus::Connecting | PollerStatus::Error(_) => 1,
    };
    let [header_area, tabs_area, banner_area, body_area, statusbar_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(banner_height),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    draw_header(app, frame, header_area);
    draw_tabs(app, frame, tabs_area);
    if banner_height > 0 {
        draw_status_banner(app, frame, banner_area);
    }
    match app.active_tab {
        Tab::MacroLens => macro_lens::draw(app, frame, body_area),
        Tab::MicroLens => micro_lens::draw(app, frame, body_area),
        Tab::SchemaLens => schema_lens::draw(app, frame, body_area),
    }
    draw_statusbar(app, frame, statusbar_area);
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
    frame.render_widget(Paragraph::new(header), area);
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
    let (selected, len, sort_label, extra) = match app.active_tab {
        Tab::SchemaLens => (
            app.schema_table_state.selected(),
            app.snapshot.schema.as_deref().map_or(0, |s| s.tables.len()),
            app.schema_sort_mode.label(),
            " \u{2502} R: recollect",
        ),
        _ => (
            app.table_state.selected(),
            app.snapshot.activity.len(),
            app.sort_mode.label(),
            "",
        ),
    };
    let row = match (selected, len) {
        (Some(i), len) if len > 0 => format!("{}/{len}", i + 1),
        _ => "-".to_string(),
    };
    let keys = Line::from(format!(
        " q/Esc: quit \u{2502} Tab: switch lens \u{2502} j/k: row {row} \u{2502} Enter: \
         detail \u{2502} s: sort={sort_label}{extra} \u{2502} +/-: refresh={:.1}s \u{2502} \
         data: {staleness}",
        app.refresh_interval.as_secs_f64(),
    ))
    .dim();
    frame.render_widget(Paragraph::new(keys), area);
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
        let mut snap = app.snapshot.as_ref().clone();
        snap.status = PollerStatus::Error("connection refused".to_string());
        app.snapshot = Arc::new(snap);

        let screen = render(&mut app);
        assert!(screen.contains("DB error: connection refused"));
        assert!(screen.contains("showing last known data"));
        // Last data still rendered underneath the banner.
        assert!(screen.contains("Connections"));
    }

    #[test]
    fn ok_status_renders_no_banner() {
        let mut app = App::new();
        let screen = render(&mut app);
        assert!(!screen.contains("DB error"));
        assert!(!screen.contains("connecting to PostgreSQL"));
    }

}
