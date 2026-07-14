//! View layer: pure, synchronous rendering functions. No I/O, ever.

mod macro_lens;
mod micro_lens;

use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Style, Stylize},
    text::Line,
    widgets::{Paragraph, Tabs},
};

use crate::app::{App, Tab};

/// Root layout: header / tabs / body / statusbar.
pub fn draw(app: &mut App, frame: &mut Frame) {
    let [header_area, tabs_area, body_area, statusbar_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    draw_header(app, frame, header_area);
    draw_tabs(app, frame, tabs_area);
    match app.active_tab {
        Tab::MacroLens => macro_lens::draw(app, frame, body_area),
        Tab::MicroLens => micro_lens::draw(app, frame, body_area),
    }
    draw_statusbar(frame, statusbar_area);
}

fn draw_header(app: &App, frame: &mut Frame, area: Rect) {
    let vitals = &app.snapshot.vitals;
    let header = Line::from(format!(
        " pg_lens v{} \u{2502} PG {} \u{2502} up {} \u{2502} {}/{} conns",
        env!("CARGO_PKG_VERSION"),
        vitals.server_version,
        format_uptime(vitals.uptime_secs),
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

fn draw_statusbar(frame: &mut Frame, area: Rect) {
    let keys = Line::from(" q/Esc: quit \u{2502} Tab: switch lens").dim();
    frame.render_widget(Paragraph::new(keys), area);
}

/// `3d 4h`, `4h 27m`, `27m`, `42s` — good enough until Fase 4's format.rs.
fn format_uptime(secs: u64) -> String {
    let (days, hours, mins) = (secs / 86_400, (secs % 86_400) / 3_600, (secs % 3_600) / 60);
    if days > 0 {
        format!("{days}d {hours}h")
    } else if hours > 0 {
        format!("{hours}h {mins}m")
    } else if mins > 0 {
        format!("{mins}m")
    } else {
        format!("{secs}s")
    }
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
    fn format_uptime_is_human() {
        assert_eq!(format_uptime(42), "42s");
        assert_eq!(format_uptime(27 * 60), "27m");
        assert_eq!(format_uptime(4 * 3_600 + 27 * 60), "4h 27m");
        assert_eq!(format_uptime(3 * 86_400 + 4 * 3_600), "3d 4h");
    }
}
