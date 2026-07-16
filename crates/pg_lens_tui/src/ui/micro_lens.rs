//! Micro Lens: per-session activity table + detail panel (Fase 4).
//!
//! Row semantics:
//! - status column `S`: `B` = blocked (pid present in `DbSnapshot::locks`),
//!   `W` = waiting on a non-null `wait_event`, ` ` otherwise — so captures
//!   prove the state without relying on color;
//! - row style mirrors it: red for blocked (wins), yellow for waiting;
//! - the query cell is truncated to the column width with an explicit `…`;
//! - `Enter` opens a detail panel with the full query (wrapped); while it is
//!   open `j`/`k` keep moving the selection (the panel follows),
//!   `Enter`/`Esc` close it (see `crate::app::handle_key`).

use std::collections::HashSet;

use pg_lens_core::waits::{WaitSummary, top_waits};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Cell, Clear, Paragraph, Row, Table, Wrap},
};

use crate::app::App;
use crate::ui::{format, sql, style};

/// (width, spacing-follows) of every fixed column, in order. The last,
/// flexible column (Query) takes whatever is left.
const FIXED_WIDTHS: [u16; 7] = [6, 10, 12, 12, 11, 22, 8];
const STATUS_WIDTH: u16 = 1;
const COLUMN_SPACING: u16 = 1;
/// Highlight symbol "▶ " rendered left of the selected row.
const HIGHLIGHT_WIDTH: u16 = 2;

/// Below this width the waits strip hides: the entries would truncate into
/// noise, and the activity table needs every column it can get.
const WAITS_MIN_WIDTH: u16 = 80;
/// Minimum body height to spend a line on the strip — the table (border +
/// header + a few rows) always wins the space fight.
const WAITS_MIN_HEIGHT: u16 = 8;
/// At most this many ranked waits render in the strip.
const WAITS_TOP_N: usize = 5;

pub fn draw(app: &mut App, frame: &mut Frame, area: Rect) {
    // Top-waits strip: aggregated over the FULL activity set, never the
    // filtered `row_order` — it answers "what is the *server* stuck on",
    // and a display filter must not change that answer. One line, hidden
    // when nothing waits or the terminal is too narrow/short to afford it.
    let waits = top_waits(&app.snapshot.activity);
    let table_area = if !waits.is_empty()
        && area.width >= WAITS_MIN_WIDTH
        && area.height >= WAITS_MIN_HEIGHT
    {
        let [strip_area, rest] =
            Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(area);
        frame.render_widget(Paragraph::new(waits_strip(&waits)), strip_area);
        rest
    } else {
        area
    };
    draw_table(app, frame, table_area);
    // Empty state: the header/border still render (so the filter term and
    // count stay visible), but the body gets a centered hint distinguishing
    // "your filter matches nothing" from "the server is idle".
    if app.row_order.is_empty() {
        draw_empty(app, frame, table_area);
    }
    if app.detail_open {
        draw_detail(app, frame, area);
    }
}

/// One-line strip: `waits 4/6 waiting │ Lock:… ×2 │ IO:… ×1 …` — Lock:*
/// tinted red (contention), IO:* yellow (disk pressure), the rest default.
/// Only called with a non-empty summary; overflow clips right, which drops
/// the least frequent entries first (the list is ranked).
fn waits_strip(waits: &WaitSummary) -> Line<'static> {
    let sep = Span::styled(" \u{2502} ", style::label_style());
    let mut spans = vec![
        Span::styled(" waits ", style::label_style()),
        Span::styled(
            format!("{}/{}", waits.waiting, waits.total),
            Style::new().bold(),
        ),
        Span::styled(" waiting", style::label_style()),
    ];
    for (wait, count) in waits.ranked.iter().take(WAITS_TOP_N) {
        let color = if wait.starts_with("Lock:") {
            Some(Color::Red)
        } else if wait.starts_with("IO:") {
            Some(Color::Yellow)
        } else {
            None
        };
        let wait_style = color.map_or_else(Style::new, |c| Style::new().fg(c));
        spans.push(sep.clone());
        spans.push(Span::styled(wait.clone(), wait_style));
        spans.push(Span::styled(
            format!(" \u{d7}{count}"),
            style::label_style(),
        ));
    }
    Line::from(spans)
}

/// Centered placeholder drawn inside the table body when no rows show.
fn draw_empty(app: &App, frame: &mut Frame, area: Rect) {
    // Inside the border, below the header row.
    let inner = Rect {
        x: area.x + 1,
        y: area.y + 2,
        width: area.width.saturating_sub(2),
        height: area.height.saturating_sub(3),
    };
    if inner.height == 0 {
        return;
    }
    let msg = if !app.snapshot.activity.is_empty() && !app.filter.is_empty() {
        format!("No sessions match \u{201c}{}\u{201d}", app.filter)
    } else {
        "No active sessions".to_string()
    };
    let para = Paragraph::new(Line::from(Span::styled(
        msg,
        Style::new().fg(Color::DarkGray).add_modifier(Modifier::ITALIC),
    )))
    .alignment(ratatui::layout::Alignment::Center);
    frame.render_widget(para, inner);
}

fn draw_table(app: &mut App, frame: &mut Frame, area: Rect) {
    let header = Row::new([
        "S", "PID", "DB", "User", "Client", "State", "Wait", "Duration", "Query",
    ])
    .style(Style::new().bold());

    // Pids blocked according to the blocking query — red beats yellow.
    let blocked: HashSet<i32> = app.snapshot.locks.iter().map(|l| l.pid).collect();

    let query_width = query_column_width(area.width);

    // Render in the sort order computed by update() (`s` cycles the mode).
    let rows = app
        .row_order
        .iter()
        .filter_map(|&i| app.snapshot.activity.get(i))
        .map(|row| {
            let is_blocked = blocked.contains(&row.pid);
            let is_waiting = row.wait_event.is_some();
            let status = if is_blocked {
                "B"
            } else if is_waiting {
                "W"
            } else {
                " "
            };
            let style = if is_blocked {
                Style::new().fg(Color::Red).bold()
            } else if is_waiting {
                Style::new().fg(Color::Yellow)
            } else {
                Style::new()
            };
            // Truncate FIRST (char-safe), then tokenize the truncated text —
            // the ellipsis lands in a default-styled span. Tinted rows
            // (blocked red / waiting yellow) keep PLAIN text: their row fg
            // is the severity signal, and per-span SQL colors would
            // fragment it (documented decision — severity beats syntax).
            let query_text = format::truncate_with_ellipsis(&row.query, query_width);
            let query_cell = if is_blocked || is_waiting {
                Cell::from(query_text)
            } else {
                Cell::from(sql::highlight_line(&query_text))
            };
            Row::new(vec![
                Cell::from(status.to_string()),
                Cell::from(row.pid.to_string()),
                Cell::from(row.database.clone()),
                Cell::from(row.username.clone()),
                Cell::from(row.client.clone()),
                Cell::from(row.state.clone()),
                Cell::from(row.wait_event.clone().unwrap_or_default()),
                Cell::from(format::human_duration(row.duration_secs)),
                query_cell,
            ])
            .style(style)
        });

    let widths = [
        Constraint::Length(STATUS_WIDTH),
        Constraint::Length(FIXED_WIDTHS[0]),
        Constraint::Length(FIXED_WIDTHS[1]),
        Constraint::Length(FIXED_WIDTHS[2]),
        Constraint::Length(FIXED_WIDTHS[3]),
        Constraint::Length(FIXED_WIDTHS[4]),
        Constraint::Length(FIXED_WIDTHS[5]),
        Constraint::Length(FIXED_WIDTHS[6]),
        Constraint::Min(10),
    ];

    let table = Table::new(rows, widths)
        .header(header)
        .block(Block::bordered().title(activity_title(app)))
        .row_highlight_style(Style::new().add_modifier(Modifier::REVERSED))
        .highlight_symbol("\u{25b6} ");

    frame.render_stateful_widget(table, area, &mut app.table_state);
}

/// Block title showing the row count and the filter state. Plain
/// `Activity (N)` when unfiltered; while editing (`/`) it shows the live
/// term with a cursor block and `shown/total`; a committed filter shows the
/// term without the cursor.
fn activity_title(app: &App) -> Line<'static> {
    let shown = app.row_order.len();
    let total = app.snapshot.activity.len();
    let mut spans = vec![Span::styled("Activity", Style::new().bold())];
    if app.filter_editing {
        spans.push(Span::raw("  "));
        spans.push(Span::styled("/", Style::new().fg(Color::Cyan).bold()));
        spans.push(Span::styled(
            app.filter.clone(),
            Style::new().fg(Color::Cyan),
        ));
        // A block cursor makes the edit field obvious in a screenshot.
        spans.push(Span::styled(
            "\u{2588}",
            Style::new().fg(Color::Cyan).add_modifier(Modifier::SLOW_BLINK),
        ));
        spans.push(Span::styled(
            format!("  {shown}/{total}"),
            Style::new().fg(Color::DarkGray),
        ));
    } else if app.filter.is_empty() {
        spans.push(Span::styled(
            format!(" ({total})"),
            Style::new().fg(Color::DarkGray),
        ));
    } else {
        spans.push(Span::raw("  "));
        spans.push(Span::styled(
            format!("filter: {}", app.filter),
            Style::new().fg(Color::Cyan),
        ));
        spans.push(Span::styled(
            format!("  {shown}/{total}"),
            Style::new().fg(Color::DarkGray),
        ));
    }
    Line::from(spans)
}

/// How many characters the Query column can hold at this terminal width:
/// total width minus borders, highlight symbol, the fixed columns and the
/// spacing between all nine columns.
fn query_column_width(area_width: u16) -> usize {
    let fixed: u16 = FIXED_WIDTHS.iter().sum::<u16>() + STATUS_WIDTH;
    let overhead = 2 /* block borders */ + HIGHLIGHT_WIDTH + fixed + 8 * COLUMN_SPACING;
    usize::from(area_width.saturating_sub(overhead))
}

/// Full-query detail panel, drawn over the lower part of the table area.
fn draw_detail(app: &App, frame: &mut Frame, area: Rect) {
    let Some(row) = app.selected_row() else {
        return;
    };

    let [_, panel_area] =
        Layout::vertical([Constraint::Percentage(40), Constraint::Percentage(60)]).areas(area);

    let title = format!(
        "Detail \u{2014} pid {} \u{2502} {}@{} \u{2502} {} \u{2502} {} (Enter/Esc: close)",
        row.pid,
        row.username,
        row.database,
        row.state,
        format::human_duration(row.duration_secs),
    );
    let mut lines = Vec::new();
    if let Some(wait) = &row.wait_event {
        lines.push(Line::from(format!("wait: {wait}")).style(Style::new().fg(Color::Yellow)));
    }
    // Full query, line by line, with SQL syntax highlighting.
    lines.extend(sql::highlight_lines(&row.query));

    let panel = Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .block(Block::bordered().title(title));
    frame.render_widget(Clear, panel_area);
    frame.render_widget(panel, panel_area);
}

#[cfg(test)]
mod tests {
    use super::{WAITS_TOP_N, query_column_width, waits_strip};
    use pg_lens_core::waits::WaitSummary;
    use ratatui::style::Color;

    #[test]
    fn query_width_shrinks_with_the_terminal_and_never_underflows() {
        // 120 cols: 120 - (2 + 2 + 82 + 8) = 26 chars for the query.
        assert_eq!(query_column_width(120), 26);
        assert!(query_column_width(80) < query_column_width(120));
        // Absurdly narrow terminals must not panic or wrap around.
        assert_eq!(query_column_width(10), 0);
    }

    #[test]
    fn waits_strip_shows_ratio_counts_and_severity_colors() {
        let summary = WaitSummary {
            waiting: 3,
            total: 7,
            ranked: vec![
                ("Lock:transactionid".to_string(), 2),
                ("IO:DataFileRead".to_string(), 1),
                ("Client:ClientRead".to_string(), 1),
            ],
        };
        let line = waits_strip(&summary);
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains("3/7 waiting"), "{text}");
        assert!(text.contains("Lock:transactionid \u{d7}2"), "{text}");
        assert!(text.contains("IO:DataFileRead \u{d7}1"), "{text}");
        // Severity tints: Lock:* red, IO:* yellow, others default.
        let color_of = |needle: &str| {
            line.spans
                .iter()
                .find(|s| s.content == needle)
                .expect("span present")
                .style
                .fg
        };
        assert_eq!(color_of("Lock:transactionid"), Some(Color::Red));
        assert_eq!(color_of("IO:DataFileRead"), Some(Color::Yellow));
        assert_eq!(color_of("Client:ClientRead"), None);
    }

    #[test]
    fn waits_strip_caps_at_top_n() {
        let ranked: Vec<(String, usize)> = (0..WAITS_TOP_N + 3)
            .map(|i| (format!("LWLock:Fake{i}"), 1))
            .collect();
        let summary = WaitSummary {
            waiting: ranked.len(),
            total: ranked.len(),
            ranked,
        };
        let line = waits_strip(&summary);
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text.matches("LWLock:Fake").count(), WAITS_TOP_N);
    }
}
