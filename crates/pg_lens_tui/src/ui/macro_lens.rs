//! Macro Lens: server-wide vitals dashboard (mock data in Fase 1).

use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Style},
    text::Line,
    widgets::{Block, Gauge, Paragraph, Sparkline},
};

use crate::app::App;

pub fn draw(app: &App, frame: &mut Frame, area: Rect) {
    let vitals = &app.snapshot.vitals;

    let [gauge_area, sparkline_area, vitals_area] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Length(6),
        Constraint::Min(0),
    ])
    .areas(area);

    let ratio = if vitals.max_connections > 0 {
        f64::from(vitals.connections_total) / f64::from(vitals.max_connections)
    } else {
        0.0
    };
    let gauge = Gauge::default()
        .block(Block::bordered().title("Connections"))
        .gauge_style(Style::new().fg(Color::Cyan))
        .ratio(ratio.clamp(0.0, 1.0))
        .label(format!(
            "{}/{}",
            vitals.connections_total, vitals.max_connections
        ));
    frame.render_widget(gauge, gauge_area);

    let sparkline = Sparkline::default()
        .block(Block::bordered().title(format!("TPS (now: {:.0})", vitals.tps)))
        .style(Style::new().fg(Color::Green))
        .data(&vitals.tps_history);
    frame.render_widget(sparkline, sparkline_area);

    let lines = vec![
        Line::from(format!("Cache hit ratio : {:.1}%", vitals.cache_hit_ratio * 100.0)),
        Line::from(format!("Active          : {}", vitals.active)),
        Line::from(format!("Idle            : {}", vitals.idle)),
        Line::from(format!("Idle in tx      : {}", vitals.idle_in_transaction)),
        Line::from(format!("Waiting         : {}", vitals.waiting)),
    ];
    let paragraph = Paragraph::new(lines).block(Block::bordered().title("Vitals"));
    frame.render_widget(paragraph, vitals_area);
}
