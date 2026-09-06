//! Progress Lens (v0.17.1): in-flight maintenance and DDL operations
//! tracked via `pg_stat_progress_*` (`CREATE INDEX`, `VACUUM`, `CLUSTER`,
//! `ANALYZE`, `REINDEX`).
//!
//! Provides real-time visibility into long-running operations with visual progress
//! gauges, step counters, correlated session details from `pg_stat_activity`,
//! and administrator controls (`c: cancel`, `K: terminate`).

use ratatui::{
    Frame,
    layout::{Constraint, Flex, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Cell, Clear, Paragraph, Row, Table},
};

use crate::app::App;
use crate::ui::{format, sql};

pub fn draw(app: &mut App, frame: &mut Frame, area: Rect) {
    if app.progress_row_order.is_empty() {
        draw_empty(app, frame, area);
    } else {
        draw_table(app, frame, area);
    }

    if app.detail_open {
        draw_detail(app, frame, area);
    }
}

fn progress_gauge_spans(pct: Option<f64>, bar_width: usize) -> Vec<Span<'static>> {
    let Some(p) = pct else {
        return vec![
            Span::styled("[", Style::new().dim()),
            Span::styled(
                "\u{2500}".repeat(bar_width),
                Style::new().fg(Color::DarkGray),
            ),
            Span::styled("] ", Style::new().dim()),
            Span::styled("in prog", Style::new().dim()),
        ];
    };

    let clamped = p.clamp(0.0, 100.0);
    let filled = ((clamped / 100.0) * (bar_width as f64)).round() as usize;
    let empty = bar_width.saturating_sub(filled);

    let color = if clamped >= 90.0 {
        Color::Green
    } else if clamped >= 40.0 {
        Color::Cyan
    } else {
        Color::Yellow
    };

    vec![
        Span::styled("[", Style::new().dim()),
        Span::styled("\u{2588}".repeat(filled), Style::new().fg(color)),
        Span::styled("\u{2591}".repeat(empty), Style::new().dim()),
        Span::styled("] ", Style::new().dim()),
        Span::styled(format!("{clamped:5.1}%"), Style::new().fg(color).bold()),
    ]
}

fn command_badge(command: &str) -> Span<'static> {
    let cmd_upper = command.to_uppercase();
    if cmd_upper.starts_with("VACUUM") {
        Span::styled(
            command.to_string(),
            Style::new().fg(Color::Green).bold(),
        )
    } else if cmd_upper.starts_with("CREATE INDEX") || cmd_upper.starts_with("REINDEX") {
        Span::styled(
            command.to_string(),
            Style::new().fg(Color::Cyan).bold(),
        )
    } else if cmd_upper.starts_with("CLUSTER") {
        Span::styled(
            command.to_string(),
            Style::new().fg(Color::Magenta).bold(),
        )
    } else if cmd_upper.starts_with("ANALYZE") {
        Span::styled(
            command.to_string(),
            Style::new().fg(Color::Blue).bold(),
        )
    } else {
        Span::styled(
            command.to_string(),
            Style::new().fg(Color::Yellow).bold(),
        )
    }
}

fn draw_table(app: &mut App, frame: &mut Frame, area: Rect) {
    let rows_data = app.snapshot.unified_progress();
    let filter_active = !app.progress_filter.is_empty();

    let title = if app.progress_filter_editing {
        format!(" Progress Lens [filter: {}_\u{2588}] ", app.progress_filter)
    } else if filter_active {
        format!(
            " Progress Lens [filter: \"{}\" \u{2014} {} matching] ",
            app.progress_filter,
            app.progress_row_order.len()
        )
    } else {
        format!(
            " Progress Lens ({} in-flight operations) ",
            app.progress_row_order.len()
        )
    };

    let block = Block::bordered()
        .title(title)
        .border_style(Style::new().fg(Color::Cyan));

    let header = Row::new(vec![
        Cell::from(Span::styled("PID", Style::new().bold().underlined())),
        Cell::from(Span::styled("COMMAND", Style::new().bold().underlined())),
        Cell::from(Span::styled("RELATION", Style::new().bold().underlined())),
        Cell::from(Span::styled("PHASE", Style::new().bold().underlined())),
        Cell::from(Span::styled("PROGRESS", Style::new().bold().underlined())),
        Cell::from(Span::styled("PROCESSED / TOTAL", Style::new().bold().underlined())),
        Cell::from(Span::styled("ELAPSED", Style::new().bold().underlined())),
        Cell::from(Span::styled("DETAILS", Style::new().bold().underlined())),
    ])
    .style(Style::new().fg(Color::White))
    .bottom_margin(1);

    let rows: Vec<Row> = app
        .progress_row_order
        .iter()
        .filter_map(|&idx| rows_data.get(idx))
        .map(|row| {
            let pid_span = Span::styled(row.pid.to_string(), Style::new().fg(Color::Yellow));
            let cmd_span = command_badge(&row.command);
            let rel_span = Span::styled(row.relation.clone(), Style::new().fg(Color::White).bold());
            let phase_span = Span::styled(row.phase.clone(), Style::new().dim());

            let progress_cell = Cell::from(Line::from(progress_gauge_spans(row.progress_pct, 10)));

            let steps_text = if row.total_step > 0 {
                format!("{} / {} {}", row.current_step, row.total_step, row.unit)
            } else if row.current_step > 0 {
                format!("{} {}", row.current_step, row.unit)
            } else {
                "\u{2014}".to_string()
            };
            let steps_span = Span::styled(steps_text, Style::new().dim());

            let duration_text = app
                .snapshot
                .activity
                .iter()
                .find(|a| a.pid == row.pid)
                .map(|a| format::human_duration(a.duration_secs))
                .unwrap_or_else(|| "\u{2014}".to_string());
            let dur_span = Span::styled(duration_text, Style::new().fg(Color::Magenta));

            let detail_span = Span::styled(row.detail.clone(), Style::new().dim());

            Row::new(vec![
                Cell::from(pid_span),
                Cell::from(cmd_span),
                Cell::from(rel_span),
                Cell::from(phase_span),
                progress_cell,
                Cell::from(steps_span),
                Cell::from(dur_span),
                Cell::from(detail_span),
            ])
        })
        .collect();

    let widths = [
        Constraint::Length(8),
        Constraint::Length(26),
        Constraint::Length(20),
        Constraint::Length(26),
        Constraint::Length(20),
        Constraint::Length(20),
        Constraint::Length(10),
        Constraint::Min(15),
    ];

    let table = Table::new(rows, widths)
        .header(header)
        .block(block)
        .row_highlight_style(
            Style::new()
                .bg(Color::Rgb(35, 45, 60))
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("\u{25b6} ");

    frame.render_stateful_widget(table, area, &mut app.progress_table_state);
}

fn draw_empty(app: &App, frame: &mut Frame, area: Rect) {
    let title = if app.progress_filter_editing {
        format!(" Progress Lens [filter: {}_\u{2588}] ", app.progress_filter)
    } else if !app.progress_filter.is_empty() {
        format!(" Progress Lens [filter: \"{}\"] ", app.progress_filter)
    } else {
        " Progress Lens ".to_string()
    };

    let block = Block::bordered()
        .title(title)
        .border_style(Style::new().dim());

    let inner = block.inner(area);
    frame.render_widget(block, area);

    if !app.progress_filter.is_empty() {
        let msg = Line::from(vec![
            Span::styled(" No operations match filter \"", Style::new().dim()),
            Span::styled(app.progress_filter.clone(), Style::new().fg(Color::Yellow)),
            Span::styled("\" (press \\ to clear)", Style::new().dim()),
        ]);
        frame.render_widget(Paragraph::new(msg), inner);
        return;
    }

    let lines = vec![
        Line::default(),
        Line::from(Span::styled(
            " No active maintenance or DDL operations in progress",
            Style::new().fg(Color::White).bold(),
        )),
        Line::default(),
        Line::from(Span::styled(
            " Monitoring real-time catalog progress views:",
            Style::new().fg(Color::Cyan),
        )),
        Line::from(Span::styled(
            "   \u{2022} pg_stat_progress_create_index (CREATE INDEX, REINDEX)",
            Style::new().dim(),
        )),
        Line::from(Span::styled(
            "   \u{2022} pg_stat_progress_vacuum       (VACUUM, autovacuum)",
            Style::new().dim(),
        )),
        Line::from(Span::styled(
            "   \u{2022} pg_stat_progress_cluster      (CLUSTER, VACUUM FULL)",
            Style::new().dim(),
        )),
        Line::from(Span::styled(
            "   \u{2022} pg_stat_progress_analyze      (ANALYZE)",
            Style::new().dim(),
        )),
        Line::default(),
        Line::from(Span::styled(
            " In-flight operations will appear here with live progress percentages,",
            Style::new().dim(),
        )),
        Line::from(Span::styled(
            " step counts, elapsed runtime, and administrator controls (c: cancel, K: terminate).",
            Style::new().dim(),
        )),
    ];

    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_detail(app: &App, frame: &mut Frame, area: Rect) {
    let Some(row) = app.selected_progress_row() else {
        return;
    };

    let [panel_area] = Layout::horizontal([Constraint::Length(84)])
        .flex(Flex::Center)
        .areas(area);
    let [panel_area] = Layout::vertical([Constraint::Length(22)])
        .flex(Flex::Center)
        .areas(panel_area);

    let session = app.snapshot.activity.iter().find(|a| a.pid == row.pid);

    let title = format!(
        " Progress Details \u{2014} PID {} ({} on {}) ",
        row.pid, row.command, row.relation
    );

    let mut lines = Vec::new();

    // Progress headline & gauge
    let pct_label = row
        .progress_pct
        .map(|p| format!("{p:.1}%"))
        .unwrap_or_else(|| "in progress".to_string());
    lines.push(Line::from(vec![
        Span::styled("Phase: ", Style::new().dim()),
        Span::styled(row.phase.clone(), Style::new().fg(Color::Yellow).bold()),
        Span::styled(" \u{2502} Progress: ", Style::new().dim()),
        Span::styled(pct_label, Style::new().fg(Color::Green).bold()),
    ]));

    lines.push(Line::from(progress_gauge_spans(row.progress_pct, 24)));

    if row.total_step > 0 {
        lines.push(Line::from(vec![
            Span::styled("Processed: ", Style::new().dim()),
            Span::styled(
                format!("{} / {} {}", row.current_step, row.total_step, row.unit),
                Style::new().fg(Color::White),
            ),
        ]));
    }

    if !row.detail.is_empty() {
        lines.push(Line::from(vec![
            Span::styled("Detail: ", Style::new().dim()),
            Span::styled(row.detail.clone(), Style::new().fg(Color::Cyan)),
        ]));
    }

    lines.push(Line::default());

    // Correlated session info
    if let Some(s) = session {
        lines.push(Line::from(vec![
            Span::styled("User: ", Style::new().dim()),
            Span::styled(s.username.clone(), Style::new().fg(Color::White)),
            Span::styled(" \u{2502} DB: ", Style::new().dim()),
            Span::styled(s.database.clone(), Style::new().fg(Color::White)),
            Span::styled(" \u{2502} Client: ", Style::new().dim()),
            Span::styled(s.client.clone(), Style::new().fg(Color::White)),
            Span::styled(" \u{2502} Elapsed: ", Style::new().dim()),
            Span::styled(
                format::human_duration(s.duration_secs),
                Style::new().fg(Color::Magenta).bold(),
            ),
        ]));

        if !s.application_name.is_empty() {
            lines.push(Line::from(vec![
                Span::styled("App: ", Style::new().dim()),
                Span::styled(s.application_name.clone(), Style::new().dim()),
                Span::styled(" \u{2502} State: ", Style::new().dim()),
                Span::styled(s.state.clone(), Style::new().dim()),
            ]));
        }

        lines.push(Line::default());
        lines.push(Line::from(Span::styled("Query:", Style::new().dim())));
        lines.extend(sql::highlight_lines(&s.query));
    } else {
        lines.push(Line::from(Span::styled(
            "Session not present in pg_stat_activity (backend may have completed or is a worker)",
            Style::new().dim().italic(),
        )));
    }

    lines.push(Line::default());
    lines.push(Line::from(vec![
        Span::styled("Shortcuts: ", Style::new().dim()),
        Span::styled("c", Style::new().fg(Color::Yellow).bold()),
        Span::styled(": cancel  ", Style::new().dim()),
        Span::styled("K", Style::new().fg(Color::Red).bold()),
        Span::styled(": terminate  ", Style::new().dim()),
        Span::styled("Enter/Esc", Style::new().fg(Color::White).bold()),
        Span::styled(": close", Style::new().dim()),
    ]));

    let panel = Paragraph::new(lines)
        .block(Block::bordered().title(title).border_style(Style::new().fg(Color::Yellow)));

    frame.render_widget(Clear, panel_area);
    frame.render_widget(panel, panel_area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    #[test]
    fn progress_lens_renders_table_from_mock() {
        let mut app = App::new();
        app.active_tab = crate::app::Tab::ProgressLens;
        // Make sure progress row order is populated from mock
        crate::app::update(&mut app, crate::app::Action::Tick);

        let backend = TestBackend::new(140, 40);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        terminal
            .draw(|frame| draw(&mut app, frame, frame.area()))
            .expect("draw");

        let screen: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();

        assert!(screen.contains("Progress Lens"), "{screen}");
        assert!(screen.contains("CREATE INDEX"), "{screen}");
        assert!(screen.contains("VACUUM"), "{screen}");
        assert!(screen.contains("4821"), "{screen}");
        assert!(screen.contains("4650"), "{screen}");
    }

    #[test]
    fn progress_lens_detail_panel_renders() {
        let mut app = App::new();
        app.active_tab = crate::app::Tab::ProgressLens;
        crate::app::update(&mut app, crate::app::Action::Tick);
        app.detail_open = true;
        app.progress_table_state.select(Some(0));

        let backend = TestBackend::new(140, 40);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        terminal
            .draw(|frame| draw(&mut app, frame, frame.area()))
            .expect("draw");

        let screen: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();

        assert!(screen.contains("Progress Details"), "{screen}");
        assert!(screen.contains("Shortcuts:"), "{screen}");
    }

    #[test]
    fn progress_lens_empty_state_renders() {
        let mut app = App::new();
        app.active_tab = crate::app::Tab::ProgressLens;
        let mut snap = (*app.snapshot).clone();
        snap.ddl_progress = Some(Vec::new());
        snap.vacuum_progress = Some(Vec::new());
        app.snapshot = std::sync::Arc::new(snap);
        app.progress_row_order.clear();

        let backend = TestBackend::new(140, 40);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        terminal
            .draw(|frame| draw(&mut app, frame, frame.area()))
            .expect("draw");

        let screen: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();

        assert!(screen.contains("No active maintenance"), "{screen}");
        assert!(screen.contains("pg_stat_progress_create_index"), "{screen}");
    }
}
