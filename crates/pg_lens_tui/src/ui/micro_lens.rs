//! Micro Lens: per-session activity table + detail panel (Fase 4).
//!
//! Row semantics:
//! - status column `S`: `B` = blocked (pid present in `DbSnapshot::locks`),
//!   `W` = waiting on a non-null `wait_event`, ` ` otherwise — so captures
//!   prove the state without relying on color;
//! - v0.16: the WHOLE row is colored, pg_activity-style — see
//!   [`row_severity_style`] for the full precedence (blocked wins, then a
//!   running-query duration override, then plain waiting, then the state's
//!   own base color from [`state_row_color`]); the `S` column's `B`/`W`
//!   marker stays the textual proof (captures never rely on color alone);
//! - the query cell is truncated to the column width with an explicit `…`,
//!   in the row's own color — SQL keyword highlighting only appears in the
//!   `Enter` detail panel, never the table row (Part B, v0.16);
//! - `Enter` opens a detail panel with the full query (wrapped); while it is
//!   open `j`/`k` keep moving the selection (the panel follows),
//!   `Enter`/`Esc` close it (see `crate::app::handle_key`).
//! - `w` (U3) opens the full ranked-waits panel — the one-line strip above
//!   the table only ever shows the top few entries; this is the complete
//!   list, one row per distinct `wait_event`, each with its share of
//!   WAITING sessions and a proportional bar. `w`/`Esc` close it; it and the
//!   detail panel are mutually exclusive (see `crate::app::handle_key`).
//! - v0.9: a `Xact` column shows the age of each session's open transaction
//!   (`—` when it has none), tinted by [`pg_lens_core::xact_age`]'s severity
//!   tiers (idle-in-transaction reads worse than an equally-old active
//!   query); an "oldest xact" one-line headline appears above the table,
//!   same gating as the waits strip, whenever the oldest open transaction
//!   in the snapshot is at least Warn severity (calm snapshots stay quiet).

use std::collections::HashSet;

use pg_lens_core::blocking::{BlockingChain, blocking_chain};
use pg_lens_core::idle_sessions::{Severity as IdleSeverity, oldest_idle_session, severity as idle_session_severity};
use pg_lens_core::waits::{WaitSummary, top_waits};
use pg_lens_core::xact_age::{OldestXact, Severity as XactSeverity, oldest_open_xact, xact_age_severity};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::{Block, Cell, Clear, Paragraph, Row, Table, Wrap},
};

use crate::app::{App, MicroView};
use crate::ui::{format, sql, style};

/// (width, spacing-follows) of every fixed column, in order. The last,
/// flexible column (Query) takes whatever is left.
const FIXED_WIDTHS: [u16; 8] = [6, 10, 12, 12, 11, 22, 8, 8];
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
    // v0.11: the idle census (`I`) fully replaces the body — same shape as
    // the Schema Lens's Vacuum sub-view — so the waits strip / oldest-xact
    // headline (both about ACTIVE sessions) stay hidden there; the census
    // gets its own count headline instead.
    if app.micro_view == MicroView::Idle {
        draw_idle_view(app, frame, area);
        return;
    }
    // Top-waits strip: aggregated over the FULL activity set, never the
    // filtered `row_order` — it answers "what is the *server* stuck on",
    // and a display filter must not change that answer. One line, hidden
    // when nothing waits or the terminal is too narrow/short to afford it.
    let waits = top_waits(&app.snapshot.activity);
    let oldest_xact = oldest_open_xact(&app.snapshot.activity);
    let room = area.width >= WAITS_MIN_WIDTH && area.height >= WAITS_MIN_HEIGHT;
    let show_waits = !waits.is_empty() && room;
    // Calm snapshots (nothing past Warn) stay quiet — same "hidden unless
    // there is something to say" contract as the waits strip.
    let show_xact = room
        && oldest_xact
            .as_ref()
            .is_some_and(|o| o.severity != XactSeverity::Ok);

    let mut constraints = Vec::new();
    if show_waits {
        constraints.push(Constraint::Length(1));
    }
    if show_xact {
        constraints.push(Constraint::Length(1));
    }
    let table_area = if constraints.is_empty() {
        area
    } else {
        constraints.push(Constraint::Min(0));
        let chunks = Layout::vertical(constraints).split(area);
        let mut idx = 0;
        if show_waits {
            frame.render_widget(Paragraph::new(waits_strip(&waits)), chunks[idx]);
            idx += 1;
        }
        if show_xact {
            // Safe: `show_xact` is only true when `oldest_xact` is `Some`.
            if let Some(oldest) = &oldest_xact {
                frame.render_widget(Paragraph::new(oldest_xact_headline(oldest)), chunks[idx]);
            }
            idx += 1;
        }
        chunks[idx]
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
    if app.waits_open {
        draw_waits_panel(app, frame, area);
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

/// Yellow warn / red bad, matching the rest of the TUI's severity
/// convention (`ui/vacuum.rs`, the waits strip). `Ok` renders default (no
/// tint needed — callers only reach this on a non-Ok headline anyway).
fn xact_severity_color(severity: XactSeverity) -> Color {
    match severity {
        XactSeverity::Ok => Color::Reset,
        XactSeverity::Warn => Color::Yellow,
        XactSeverity::Bad => Color::Red,
    }
}

/// One-line "oldest open transaction" headline: `oldest xact 38m12s │ pid
/// 4312 │ idle in transaction`. Only rendered when the oldest transaction in
/// the snapshot is at least Warn severity — the exact session driving
/// XID-wraparound risk and lock retention.
fn oldest_xact_headline(oldest: &OldestXact<'_>) -> Line<'static> {
    let color = xact_severity_color(oldest.severity);
    let age = oldest.row.xact_age_secs.unwrap_or(0.0);
    Line::from(vec![
        Span::styled(" oldest xact ", style::label_style()),
        Span::styled(format::human_duration(age), Style::new().fg(color).bold()),
        Span::styled(" \u{2502} pid ", style::label_style()),
        Span::styled(oldest.row.pid.to_string(), Style::new().bold()),
        Span::styled(" \u{2502} ", style::label_style()),
        Span::styled(oldest.row.state.clone(), Style::new().fg(color)),
    ])
}

/// Yellow warn / red bad, matching [`xact_severity_color`]'s convention.
fn idle_severity_color(severity: IdleSeverity) -> Color {
    match severity {
        IdleSeverity::Ok => Color::Reset,
        IdleSeverity::Warn => Color::Yellow,
        IdleSeverity::Bad => Color::Red,
    }
}

/// v0.11: idle connection / connection-age census — the Micro Lens's `I`
/// body swap (see [`crate::app::MicroView`]). A count headline ("N idle
/// connections, oldest 4h12m") above the same table component the Activity
/// view uses, tinted by the oldest row's severity; the empty state reads
/// calmly ("no idle connections") rather than showing an empty border.
fn draw_idle_view(app: &mut App, frame: &mut Frame, area: Rect) {
    let rows = app.snapshot.idle_sessions.clone().unwrap_or_default();
    let [headline_area, table_area] =
        Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(area);
    frame.render_widget(Paragraph::new(idle_headline(&rows)), headline_area);
    draw_idle_table(app, frame, table_area, &rows);
    if rows.is_empty() {
        draw_idle_empty(frame, table_area);
    }
}

/// One-line count headline: `12 idle connections │ oldest 4h12m │ pid 6104`
/// — colored by the oldest row's severity tier, calm gray when there are
/// none (rather than hidden — unlike the waits strip, "no idle connections"
/// is itself useful information the operator asked for by pressing `I`).
fn idle_headline(rows: &[pg_lens_core::IdleSessionRow]) -> Line<'static> {
    if rows.is_empty() {
        return Line::from(Span::styled(
            " no idle connections",
            style::label_style(),
        ));
    }
    let oldest = oldest_idle_session(rows).expect("rows is non-empty");
    let color = idle_severity_color(oldest.severity);
    let noun = if rows.len() == 1 {
        "idle connection"
    } else {
        "idle connections"
    };
    Line::from(vec![
        Span::raw(" "),
        Span::styled(rows.len().to_string(), Style::new().bold()),
        Span::styled(format!(" {noun}"), style::label_style()),
        Span::styled(" \u{2502} oldest ", style::label_style()),
        Span::styled(
            format::human_duration(oldest.row.idle_age_secs),
            Style::new().fg(color).bold(),
        ),
        Span::styled(" \u{2502} pid ", style::label_style()),
        Span::styled(oldest.row.pid.to_string(), Style::new().bold()),
    ])
}

/// Same column widths philosophy as the Activity table, just fewer columns
/// (no state/wait/query — a plain idle session has neither).
const IDLE_FIXED_WIDTHS: [u16; 4] = [8, 12, 12, 22];

fn draw_idle_table(
    app: &mut App,
    frame: &mut Frame,
    area: Rect,
    rows: &[pg_lens_core::IdleSessionRow],
) {
    let header = Row::new(["PID", "Age", "User", "DB", "App", "Client"]).style(Style::new().bold());

    let table_rows = rows.iter().map(|row| {
        let color = idle_severity_color(idle_session_severity(row.idle_age_secs));
        Row::new(vec![
            Cell::from(row.pid.to_string()),
            Cell::from(Span::styled(
                format::human_duration(row.idle_age_secs),
                Style::new().fg(color),
            )),
            Cell::from(row.username.clone()),
            Cell::from(row.database.clone()),
            Cell::from(row.application_name.clone()),
            Cell::from(row.client.clone()),
        ])
    });

    let widths = [
        Constraint::Length(IDLE_FIXED_WIDTHS[0]),
        Constraint::Length(IDLE_FIXED_WIDTHS[1]),
        Constraint::Length(IDLE_FIXED_WIDTHS[2]),
        Constraint::Length(IDLE_FIXED_WIDTHS[3]),
        Constraint::Min(10),
        Constraint::Min(10),
    ];

    let table = Table::new(table_rows, widths)
        .header(header)
        .block(Block::bordered().title(format!("Idle connections ({}) — I/Esc: back", rows.len())))
        .row_highlight_style(Style::new().add_modifier(Modifier::REVERSED))
        .highlight_symbol("\u{25b6} ");

    frame.render_stateful_widget(table, area, &mut app.idle_table_state);
}

/// Centered placeholder, same shape as [`draw_empty`] but for the calm
/// "server has no idle connections right now" case.
fn draw_idle_empty(frame: &mut Frame, area: Rect) {
    let inner = Rect {
        x: area.x + 1,
        y: area.y + 2,
        width: area.width.saturating_sub(2),
        height: area.height.saturating_sub(3),
    };
    if inner.height == 0 {
        return;
    }
    let para = Paragraph::new(Line::from(Span::styled(
        "No idle connections",
        Style::new().fg(Color::DarkGray).add_modifier(Modifier::ITALIC),
    )))
    .alignment(ratatui::layout::Alignment::Center);
    frame.render_widget(para, inner);
}

/// v0.9: wait-for chain rendered inside the detail panel — one arrow-joined
/// line `pid -> pid -> pid`, the root blocker (the one to act on) bold red,
/// plus an explicit deadlock flag when the walk found a cycle instead of a
/// free session at the head.
fn blocking_chain_lines(chain: &BlockingChain) -> Vec<Line<'static>> {
    let mut spans = vec![Span::styled("blocked by: ", style::label_style())];
    let last_idx = chain.chain.len() - 1;
    for (i, pid) in chain.chain.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw(" \u{2192} "));
        }
        let is_root = i == last_idx;
        let style = if is_root {
            Style::new().fg(Color::Red).bold()
        } else {
            Style::new()
        };
        spans.push(Span::styled(pid.to_string(), style));
        if is_root && !chain.deadlock {
            spans.push(Span::styled(" (root)", Style::new().fg(Color::Red)));
        }
    }
    let mut lines = vec![Line::from(spans)];
    if chain.deadlock {
        lines.push(Line::from(Span::styled(
            "\u{26a0} deadlock cycle detected \u{2014} the chain loops back on itself",
            Style::new().fg(Color::Red).bold(),
        )));
    }
    lines
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

/// Above this running-query duration the Duration column's color indicates
/// bad-tier red; below it but above [`ROW_DURATION_WARN_SECS`]
/// it indicates warn-tier yellow/orange; below both, calm green is used.
/// The styling keys on [`pg_lens_core::ActivityRow::duration_secs`] — the age of the RUNNING
/// QUERY — and only ever fires for `state == "active"` sessions: idle sessions
/// remain calm dark gray regardless of age.
const ROW_DURATION_BAD_SECS: f64 = 30.0;
const ROW_DURATION_WARN_SECS: f64 = 10.0;

/// Maps PostgreSQL `pg_stat_activity.state` string to a semantic color:
/// - `active`: green — working on a query right now;
/// - `idle`: dark gray — connection open, doing nothing;
/// - `idle in transaction`: yellow — holds locks and snapshot;
/// - `idle in transaction (aborted)`: red — a leaked, doomed transaction;
/// - anything else: neutral reset.
fn state_color(state: &str) -> Color {
    match state {
        "active" => Color::Green,
        "idle" => Color::DarkGray,
        "idle in transaction" => Color::Yellow,
        "idle in transaction (aborted)" => Color::Red,
        _ => Color::Reset,
    }
}

/// Pure time-based severity color for query duration (applied ONLY to the Duration column):
/// - active sessions:
///   - > 30s (`ROW_DURATION_BAD_SECS`): Red bold
///   - > 10s (`ROW_DURATION_WARN_SECS`): Yellow bold
///   - <= 10s: Green (calm ok execution)
/// - idle / non-active sessions:
///   - DarkGray (idle duration is not a running query)
fn duration_style(state: &str, duration_secs: f64) -> Style {
    if state == "active" {
        if duration_secs > ROW_DURATION_BAD_SECS {
            Style::new().fg(Color::Red).bold()
        } else if duration_secs > ROW_DURATION_WARN_SECS {
            Style::new().fg(Color::Yellow).bold()
        } else {
            Style::new().fg(Color::Green)
        }
    } else {
        Style::new().fg(Color::DarkGray)
    }
}

/// Wait event column style:
/// - `Lock:*` (relation, transactionid, etc.): Red bold (lock contention)
/// - `IO:*`: Yellow (storage pressure)
/// - Other wait events: Yellow
/// - None: Dim dash
fn wait_event_style(wait_event: Option<&str>) -> Style {
    match wait_event {
        Some(w) if w.starts_with("Lock:") => Style::new().fg(Color::Red).bold(),
        Some(w) if w.starts_with("IO:") => Style::new().fg(Color::Yellow),
        Some(_) => Style::new().fg(Color::Yellow),
        None => style::label_style(),
    }
}

fn draw_table(app: &mut App, frame: &mut Frame, area: Rect) {
    let header = Row::new([
        "S", "PID", "DB", "User", "Client", "State", "Wait", "Duration", "Xact", "Query",
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

            // Status cell: "B" (red bold), "W" (yellow bold), or empty
            let status_cell = if is_blocked {
                Cell::from(Span::styled("B", Style::new().fg(Color::Red).bold()))
            } else if is_waiting {
                Cell::from(Span::styled("W", Style::new().fg(Color::Yellow).bold()))
            } else {
                Cell::from(Span::raw(" "))
            };

            // PID cell: red bold when blocked, otherwise cyan identifier
            let pid_style = if is_blocked {
                Style::new().fg(Color::Red).bold()
            } else {
                Style::new().fg(Color::Cyan)
            };
            let pid_cell = Cell::from(Span::styled(row.pid.to_string(), pid_style));

            // DB cell: cyan / blue
            let db_cell = Cell::from(Span::styled(row.database.clone(), Style::new().fg(Color::Cyan)));

            // User cell: white / light
            let user_cell = Cell::from(Span::styled(row.username.clone(), Style::new().fg(Color::White)));

            // Client cell: dark gray
            let client_cell = Cell::from(Span::styled(row.client.clone(), Style::new().fg(Color::DarkGray)));

            // State cell: state-specific color
            let state_cell = Cell::from(Span::styled(row.state.clone(), Style::new().fg(state_color(&row.state))));

            // Wait cell: Lock (red bold), IO (yellow), other (yellow), none (dim dash)
            let wait_cell = match &row.wait_event {
                Some(w) => Cell::from(Span::styled(w.clone(), wait_event_style(Some(w)))),
                None => Cell::from(Span::styled("\u{2014}", style::label_style())),
            };

            // Duration cell: strictly time-based coloring
            let duration_cell = Cell::from(Span::styled(
                format::human_duration(row.duration_secs),
                duration_style(&row.state, row.duration_secs),
            ));

            // Xact column: age of the open transaction (`—` when none)
            let xact_cell = match row.xact_age_secs {
                Some(age) => {
                    let color = xact_severity_color(xact_age_severity(age, &row.state));
                    Cell::from(Span::styled(format::human_duration(age), Style::new().fg(color)))
                }
                None => Cell::from(Span::styled("\u{2014}", style::label_style())),
            };

            // Query cell: clean, neutral readable SQL text
            let query_cell = Cell::from(Span::styled(
                format::truncate_with_ellipsis(&row.query, query_width),
                Style::new().fg(Color::Reset),
            ));

            Row::new(vec![
                status_cell,
                pid_cell,
                db_cell,
                user_cell,
                client_cell,
                state_cell,
                wait_cell,
                duration_cell,
                xact_cell,
                query_cell,
            ])
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
        Constraint::Length(FIXED_WIDTHS[7]),
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
/// spacing between all ten columns.
fn query_column_width(area_width: u16) -> usize {
    let fixed: u16 = FIXED_WIDTHS.iter().sum::<u16>() + STATUS_WIDTH;
    let overhead = 2 /* block borders */ + HIGHLIGHT_WIDTH + fixed + 9 * COLUMN_SPACING;
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
    if let Some(age) = row.xact_age_secs {
        let severity = xact_age_severity(age, &row.state);
        lines.push(
            Line::from(format!("xact age: {}", format::human_duration(age)))
                .style(Style::new().fg(xact_severity_color(severity))),
        );
    }
    // SSL / Security status (v0.17)
    let ssl_info = if row.ssl {
        let ver = row.ssl_version.as_deref().unwrap_or("TLS");
        let cipher = row.ssl_cipher.as_deref().unwrap_or("");
        if cipher.is_empty() {
            format!("security: {ver} (encrypted)")
        } else {
            format!("security: {ver} ({cipher})")
        }
    } else if row.client == "local" {
        "security: unix-socket (local)".to_string()
    } else {
        "security: plain (unencrypted)".to_string()
    };
    let ssl_color = if row.ssl {
        Color::Green
    } else if row.client == "local" {
        Color::DarkGray
    } else {
        Color::Red
    };
    lines.push(Line::from(ssl_info).style(Style::new().fg(ssl_color)));

    // DDL & maintenance progress (v0.17)
    if let Some(ddl_list) = &app.snapshot.ddl_progress {
        if let Some(ddl) = ddl_list.iter().find(|d| d.pid == row.pid) {
            let pct_str = ddl
                .progress_pct
                .map(|p| format!("{p:.1}%"))
                .unwrap_or_else(|| "in progress".to_string());
            let detail_suffix = if !ddl.detail.is_empty() {
                format!(" [{}]", ddl.detail)
            } else {
                String::new()
            };
            let progress_line = format!(
                "ddl progress: {} on {} \u{2502} {} ({}/{}) \u{2502} {}{}",
                ddl.command,
                ddl.relation,
                ddl.phase,
                ddl.current_step,
                ddl.total_step,
                pct_str,
                detail_suffix
            );
            lines.push(Line::from(progress_line).style(Style::new().fg(Color::Cyan).bold()));
        }
    }
    // v0.9: wait-for chain, only when this pid is actually blocked — the
    // root blocker (the one to act on) is highlighted; a deadlock cycle is
    // flagged explicitly rather than silently showing a chain that loops.
    if let Some(chain) = blocking_chain(row.pid, &app.snapshot.locks) {
        lines.push(Line::default());
        lines.extend(blocking_chain_lines(&chain));
    }
    // Full query, line by line, with SQL syntax highlighting.
    lines.extend(sql::highlight_lines(&row.query));

    let panel = Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .block(Block::bordered().title(title));
    frame.render_widget(Clear, panel_area);
    frame.render_widget(panel, panel_area);
}

/// How wide the proportional bar renders, in `█` characters, at its max.
const WAITS_BAR_WIDTH: usize = 20;

/// Full ranked wait list (U3, `w` toggle) — same overlay proportions as the
/// detail panel (lower 60%): the COMPLETE list from `top_waits`, not just
/// the strip's top-5, one row per distinct `wait_event` with its count, its
/// share of WAITING sessions (not of all sessions — the question here is
/// "of everyone stuck, who's stuck on what"), and a bar proportional to the
/// busiest wait. Lock:* red, IO:* yellow, same severity convention as the
/// strip and the row status column.
fn draw_waits_panel(app: &App, frame: &mut Frame, area: Rect) {
    let waits = top_waits(&app.snapshot.activity);

    let [_, panel_area] =
        Layout::vertical([Constraint::Percentage(40), Constraint::Percentage(60)]).areas(area);

    let title = format!(
        "Waits \u{2014} waiting {}/{} sessions (w/Esc: close)",
        waits.waiting, waits.total
    );

    let lines: Vec<Line> = if waits.is_empty() {
        vec![Line::from("  no sessions are waiting").dim()]
    } else {
        let max_count = waits.ranked.iter().map(|&(_, c)| c).max().unwrap_or(1);
        waits
            .ranked
            .iter()
            .map(|(wait, count)| {
                let color = if wait.starts_with("Lock:") {
                    Some(Color::Red)
                } else if wait.starts_with("IO:") {
                    Some(Color::Yellow)
                } else {
                    None
                };
                let wait_style = color.map_or_else(Style::new, |c| Style::new().fg(c));
                let pct = wait_percent(*count, waits.waiting);
                let bar = wait_bar(*count, max_count, WAITS_BAR_WIDTH);
                Line::from(vec![
                    Span::styled(format!("{wait:<28}"), wait_style),
                    Span::styled(format!("{count:>6} "), style::label_style()),
                    Span::styled(format!("{pct:>5.1}% "), style::label_style()),
                    Span::styled(bar, wait_style),
                ])
            })
            .collect()
    };

    let panel = Paragraph::new(lines).block(Block::bordered().title(title));
    frame.render_widget(Clear, panel_area);
    frame.render_widget(panel, panel_area);
}

/// `count`'s share of `waiting` sessions, as a percentage; `0.0` when
/// nothing is waiting (never NaN).
fn wait_percent(count: usize, waiting: usize) -> f64 {
    if waiting > 0 {
        100.0 * count as f64 / waiting as f64
    } else {
        0.0
    }
}

/// A simple proportional bar: `count/max` of `width` filled with `█`.
/// `max == 0` (defensive: callers pass the ranked list's own max, which is
/// never zero when the list is non-empty) yields an empty bar.
fn wait_bar(count: usize, max: usize, width: usize) -> String {
    let filled = count.saturating_mul(width).checked_div(max).unwrap_or(0);
    "\u{2588}".repeat(filled.min(width))
}

#[cfg(test)]
mod tests {
    use super::{
        ROW_DURATION_BAD_SECS, ROW_DURATION_WARN_SECS, WAITS_TOP_N, blocking_chain_lines,
        duration_style, idle_headline, oldest_xact_headline, query_column_width, state_color,
        wait_bar, wait_event_style, wait_percent, waits_strip,
    };
    use pg_lens_core::waits::WaitSummary;
    use pg_lens_core::xact_age::{OldestXact, Severity as XactSeverity};
    use ratatui::style::{Color, Modifier, Style};

    #[test]
    fn query_width_shrinks_with_the_terminal_and_never_underflows() {
        // 120 cols: 120 - (2 + 2 + 90 + 9) = 17 chars for the query.
        assert_eq!(query_column_width(120), 17);
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
    fn oldest_xact_headline_shows_age_pid_and_state() {
        let mock_snapshot = crate::app::App::new().snapshot.clone();
        let row = mock_snapshot
            .activity
            .iter()
            .find(|r| r.pid == 4312)
            .expect("mock's idle-in-transaction row");
        let oldest = OldestXact {
            row,
            severity: XactSeverity::Bad,
        };
        let line = oldest_xact_headline(&oldest);
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains("pid 4312"), "{text}");
        assert!(text.contains("idle in transaction"), "{text}");
        // Bad severity tints the age span (index 1: label, age, sep, pid,
        // sep, state) red.
        assert_eq!(line.spans[1].style.fg, Some(Color::Red));
    }

    #[test]
    fn micro_lens_renders_the_xact_column_and_oldest_headline_from_mock() {
        let mut app = crate::app::App::new();
        app.active_tab = crate::app::Tab::MicroLens;
        let snapshot = app.snapshot.clone();
        crate::app::update(&mut app, crate::app::Action::Snapshot(snapshot));

        let backend = ratatui::backend::TestBackend::new(120, 36);
        let mut terminal = ratatui::Terminal::new(backend).expect("test terminal");
        terminal
            .draw(|frame| crate::ui::draw(&mut app, frame))
            .expect("draw");
        let screen: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(screen.contains("Xact"), "column header present: {screen}");
        // The mock's oldest transaction is pid 4312, idle in transaction,
        // well past the Bad threshold — the headline must call it out.
        assert!(screen.contains("oldest xact"), "{screen}");
        assert!(screen.contains("4312"), "{screen}");
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

    // --- v0.9: blocking chain in the detail panel -------------------------

    #[test]
    fn detail_panel_shows_the_wait_for_chain_and_highlights_the_root() {
        // Mock data: pid 5104 -> 4977 -> 4312 (root), a real 3-level chain.
        let mut app = crate::app::App::new();
        app.active_tab = crate::app::Tab::MicroLens;
        let snapshot = app.snapshot.clone();
        crate::app::update(&mut app, crate::app::Action::Snapshot(snapshot));

        let pos = app
            .row_order
            .iter()
            .position(|&i| app.snapshot.activity[i].pid == 5104)
            .expect("mock's deepest blocked pid is present");
        app.table_state.select(Some(pos));
        app.detail_open = true;

        let backend = ratatui::backend::TestBackend::new(120, 36);
        let mut terminal = ratatui::Terminal::new(backend).expect("test terminal");
        terminal
            .draw(|frame| crate::ui::draw(&mut app, frame))
            .expect("draw");
        let screen: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(screen.contains("blocked by"), "{screen}");
        assert!(screen.contains("5104"), "{screen}");
        assert!(screen.contains("4977"), "{screen}");
        assert!(screen.contains("4312"), "{screen}");
        assert!(screen.contains("root"), "{screen}");
    }

    #[test]
    fn blocking_chain_lines_flags_a_deadlock_cycle() {
        use pg_lens_core::blocking::BlockingChain;

        let chain = BlockingChain {
            chain: vec![1, 2, 1],
            deadlock: true,
        };
        let lines = blocking_chain_lines(&chain);
        let text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .map(|s| s.content.as_ref())
            .collect();
        assert!(text.contains("deadlock cycle"), "{text}");
    }

    #[test]
    fn blocking_chain_lines_highlights_the_root_blocker() {
        use pg_lens_core::blocking::BlockingChain;

        let chain = BlockingChain {
            chain: vec![3, 2, 1],
            deadlock: false,
        };
        let lines = blocking_chain_lines(&chain);
        let root_span = lines[0]
            .spans
            .iter()
            .find(|s| s.content == "1")
            .expect("root pid span present");
        assert_eq!(root_span.style.fg, Some(Color::Red));
    }

    // --- U3: full waits panel (`w`) --------------------------------------

    #[test]
    fn wait_percent_is_share_of_waiting_never_all_sessions() {
        // 3 of 4 waiting, 2 hold this one wait — 2/4 (all sessions) would be
        // 50%, but the panel's question is "of everyone STUCK", i.e. 2/3.
        assert!((wait_percent(2, 3) - 66.666_666_666_666_66).abs() < 1e-9);
        assert_eq!(wait_percent(0, 3), 0.0);
        // Nothing waiting: never divide by zero / NaN.
        assert_eq!(wait_percent(0, 0), 0.0);
    }

    #[test]
    fn wait_bar_is_proportional_to_the_busiest_wait() {
        assert_eq!(wait_bar(10, 10, 20).chars().count(), 20, "the max fills it");
        assert_eq!(wait_bar(5, 10, 20).chars().count(), 10, "half fills half");
        assert_eq!(wait_bar(0, 10, 20).chars().count(), 0);
        // Defensive: a zero max (should never happen for a non-empty ranked
        // list) never divides by zero.
        assert_eq!(wait_bar(3, 0, 20).chars().count(), 0);
    }

    #[test]
    fn waits_panel_renders_the_complete_ranked_list_with_bars() {
        let mut app = crate::app::App::new();
        app.active_tab = crate::app::Tab::MicroLens;
        crate::app::update(
            &mut app,
            crate::app::Action::Key(crossterm::event::KeyEvent::new(
                crossterm::event::KeyCode::Char('w'),
                crossterm::event::KeyModifiers::NONE,
            )),
        );
        assert!(app.waits_open);

        let backend = ratatui::backend::TestBackend::new(120, 36);
        let mut terminal = ratatui::Terminal::new(backend).expect("test terminal");
        terminal
            .draw(|frame| crate::ui::draw(&mut app, frame))
            .expect("draw");
        let screen: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(screen.contains("Waits \u{2014} waiting"), "{screen}");
        assert!(screen.contains("w/Esc: close"), "{screen}");
        // The mock's full ranked list (not just the strip's top-5): every
        // distinct wait_event renders, plus the bar glyph for the busiest.
        assert!(screen.contains("Lock:transactionid"), "{screen}");
        assert!(screen.contains("IO:DataFileRead"), "{screen}");
        assert!(screen.contains('\u{2588}'), "bar glyph present: {screen}");

        // `w` again closes it WITHOUT arming the quit barrier.
        crate::app::update(
            &mut app,
            crate::app::Action::Key(crossterm::event::KeyEvent::new(
                crossterm::event::KeyCode::Char('w'),
                crossterm::event::KeyModifiers::NONE,
            )),
        );
        assert!(!app.waits_open);
        assert!(!app.should_quit);
    }

    // --- v0.11: idle connection / connection-age census (`I`) -------------

    #[test]
    fn idle_view_renders_the_census_and_headline_from_mock() {
        let mut app = crate::app::App::new();
        app.active_tab = crate::app::Tab::MicroLens;
        crate::app::update(
            &mut app,
            crate::app::Action::Key(crossterm::event::KeyEvent::new(
                crossterm::event::KeyCode::Char('I'),
                crossterm::event::KeyModifiers::NONE,
            )),
        );
        assert_eq!(app.micro_view, crate::app::MicroView::Idle);

        let backend = ratatui::backend::TestBackend::new(120, 36);
        let mut terminal = ratatui::Terminal::new(backend).expect("test terminal");
        terminal
            .draw(|frame| crate::ui::draw(&mut app, frame))
            .expect("draw");
        let screen: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        // Count headline (mock carries 4 idle sessions, oldest ~4h12m).
        assert!(screen.contains("idle connections"), "{screen}");
        assert!(screen.contains("oldest"), "{screen}");
        // The census table's own columns + the mock's oldest suspect pid.
        assert!(screen.contains("Idle connections"), "{screen}");
        assert!(screen.contains("6104"), "{screen}");
        assert!(screen.contains("reporting-pool"), "{screen}");

        // `I` again closes it WITHOUT arming the quit barrier, back to the
        // Activity table.
        crate::app::update(
            &mut app,
            crate::app::Action::Key(crossterm::event::KeyEvent::new(
                crossterm::event::KeyCode::Char('I'),
                crossterm::event::KeyModifiers::NONE,
            )),
        );
        assert_eq!(app.micro_view, crate::app::MicroView::Activity);
        assert!(!app.should_quit);

        // Esc also closes it, same contract.
        crate::app::update(
            &mut app,
            crate::app::Action::Key(crossterm::event::KeyEvent::new(
                crossterm::event::KeyCode::Char('I'),
                crossterm::event::KeyModifiers::NONE,
            )),
        );
        assert_eq!(app.micro_view, crate::app::MicroView::Idle);
        crate::app::update(&mut app, crate::app::Action::Key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Esc,
            crossterm::event::KeyModifiers::NONE,
        )));
        assert_eq!(app.micro_view, crate::app::MicroView::Activity);
    }

    #[test]
    fn idle_headline_reports_zero_calmly_when_empty() {
        let line = idle_headline(&[]);
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains("no idle connections"), "{text}");
    }

    #[test]
    fn idle_headline_colors_by_the_oldest_rows_severity() {
        let rows = vec![
            pg_lens_core::IdleSessionRow {
                pid: 1,
                application_name: "a".to_string(),
                database: "d".to_string(),
                client: "10.0.0.1".to_string(),
                username: "u".to_string(),
                idle_age_secs: 100.0,
                ssl: false,
                ssl_version: None,
                ssl_cipher: None,
            },
            pg_lens_core::IdleSessionRow {
                pid: 2,
                application_name: "a".to_string(),
                database: "d".to_string(),
                client: "10.0.0.1".to_string(),
                username: "u".to_string(),
                idle_age_secs: 20_000.0,
                ssl: false,
                ssl_version: None,
                ssl_cipher: None,
            },
        ];
        let line = idle_headline(&rows);
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains("2 idle connections"), "{text}");
        assert!(text.contains("pid 2"), "{text}: must headline the oldest, not the first");
        // The age span is tinted red (past BAD_AGE_SECS).
        let age_span = line
            .spans
            .iter()
            .find(|s| s.content.contains('h') || s.content.contains('m'))
            .expect("age span present");
        assert_eq!(age_span.style.fg, Some(Color::Red));
    }

    // --- v0.16/v0.17: column color system & duration-only time coloring ----------

    #[test]
    fn state_color_maps_every_known_state_and_defaults_neutral_for_unknown() {
        assert_eq!(state_color("active"), Color::Green);
        assert_eq!(state_color("idle"), Color::DarkGray);
        assert_eq!(state_color("idle in transaction"), Color::Yellow);
        assert_eq!(state_color("idle in transaction (aborted)"), Color::Red);
        // Unknown/rare states (fastpath function call, disabled, or any
        // future addition) must never invent a severity — neutral default.
        assert_eq!(state_color("fastpath function call"), Color::Reset);
        assert_eq!(state_color("disabled"), Color::Reset);
        assert_eq!(state_color("something new in a future PG"), Color::Reset);
    }

    #[test]
    fn duration_style_applies_time_based_coloring_only_to_active_sessions() {
        // An active query past the bad threshold: red, bold.
        assert_eq!(
            duration_style("active", ROW_DURATION_BAD_SECS + 0.1),
            Style::new().fg(Color::Red).bold()
        );
        // Past the warn threshold only: yellow, bold.
        assert_eq!(
            duration_style("active", ROW_DURATION_WARN_SECS + 0.1),
            Style::new().fg(Color::Yellow).bold()
        );
        // At/below warn: plain active green.
        assert_eq!(
            duration_style("active", ROW_DURATION_WARN_SECS),
            Style::new().fg(Color::Green)
        );

        // The exact same duration on an IDLE session must NOT turn red/
        // yellow — it stays dark gray.
        assert_eq!(
            duration_style("idle", ROW_DURATION_BAD_SECS + 10_000.0),
            Style::new().fg(Color::DarkGray)
        );
        assert_eq!(
            duration_style("idle in transaction", ROW_DURATION_BAD_SECS + 10_000.0),
            Style::new().fg(Color::DarkGray)
        );
    }

    #[test]
    fn wait_event_style_highlights_locks_and_io() {
        assert_eq!(
            wait_event_style(Some("Lock:relation")),
            Style::new().fg(Color::Red).bold()
        );
        assert_eq!(
            wait_event_style(Some("IO:DataFileRead")),
            Style::new().fg(Color::Yellow)
        );
        assert_eq!(
            wait_event_style(Some("Client:ClientRead")),
            Style::new().fg(Color::Yellow)
        );
        assert_eq!(
            wait_event_style(None),
            Style::new().fg(Color::DarkGray)
        );
    }

    /// Render proof: a >30s active row renders red, AND the selected row
    /// stays visibly distinct from an unselected row of the same color (the
    /// REVERSED highlight modifier is present only on the selected row's
    /// cells) — the sharp edge the owner flagged explicitly.
    #[test]
    fn a_long_running_active_row_renders_red_and_selection_stays_distinguishable() {
        let mut app = crate::app::App::new();
        app.active_tab = crate::app::Tab::MicroLens;
        let snapshot = app.snapshot.clone();
        crate::app::update(&mut app, crate::app::Action::Snapshot(snapshot));

        // Mock pid 4650 (vacuumdb) is `active`, not blocked/waiting, with a
        // duration well past ROW_DURATION_BAD_SECS.
        let pos = app
            .row_order
            .iter()
            .position(|&i| app.snapshot.activity[i].pid == 4650)
            .expect("mock's long-running active row is present");
        app.table_state.select(Some(pos));

        let backend = ratatui::backend::TestBackend::new(120, 36);
        let mut terminal = ratatui::Terminal::new(backend).expect("test terminal");
        terminal
            .draw(|frame| crate::ui::draw(&mut app, frame))
            .expect("draw");
        let buffer = terminal.backend().buffer().clone();

        // Find the row of cells containing "4650" (the PID column) and
        // assert its fg is red.
        let width = buffer.area.width;
        let row_y = (0..buffer.area.height)
            .find(|&y| {
                let line: String = (0..width)
                    .map(|x| buffer.cell((x, y)).map(|c| c.symbol()).unwrap_or(" "))
                    .collect();
                line.contains("4650")
            })
            .expect("the long-running row is on screen");
        let red_present = (0..width).any(|x| {
            buffer
                .cell((x, row_y))
                .is_some_and(|c| c.fg == Color::Red && c.symbol() != " ")
        });
        assert!(red_present, "the long-running active row must render red");

        // Selection: the same row, once selected, must carry the REVERSED
        // modifier (the table's highlight style) — proving the cursor is
        // still visually distinguishable even though the row itself is
        // already colored.
        let reversed_present = (0..width).any(|x| {
            buffer
                .cell((x, row_y))
                .is_some_and(|c| c.modifier.contains(Modifier::REVERSED))
        });
        assert!(
            reversed_present,
            "the selected row must carry the REVERSED highlight on top of its severity color"
        );
    }

    /// Part B: the table row must NOT carry SQL keyword highlighting (no
    /// cyan-bold `SELECT`/`UPDATE` span in the Query column) while the
    /// `Enter` detail panel still highlights the full query — keyword syntax
    /// coloring moved to the expanded view only.
    #[test]
    fn table_row_query_is_plain_but_the_detail_panel_still_highlights_it() {
        let mut app = crate::app::App::new();
        app.active_tab = crate::app::Tab::MicroLens;
        let snapshot = app.snapshot.clone();
        crate::app::update(&mut app, crate::app::Action::Snapshot(snapshot));
        // pid 4821's query starts with SELECT and is short enough to fit
        // un-truncated at 160 cols — a keyword span would be provable if
        // present.
        let pos = app
            .row_order
            .iter()
            .position(|&i| app.snapshot.activity[i].pid == 4821)
            .expect("mock row present");
        app.table_state.select(Some(pos));

        let backend = ratatui::backend::TestBackend::new(160, 36);
        let mut terminal = ratatui::Terminal::new(backend).expect("test terminal");
        terminal
            .draw(|frame| crate::ui::draw(&mut app, frame))
            .expect("draw");
        let buffer = terminal.backend().buffer().clone();
        let width = buffer.area.width;
        let row_y = (0..buffer.area.height)
            .find(|&y| {
                let line: String = (0..width)
                    .map(|x| buffer.cell((x, y)).map(|c| c.symbol()).unwrap_or(" "))
                    .collect();
                line.contains("SELECT")
            })
            .expect("the SELECT row is on screen");
        // No cyan-bold keyword span in that row (Cyan is reserved for the
        // keyword highlighter; the row itself is state-colored green/etc,
        // never cyan).
        let cyan_bold_present = (0..width).any(|x| {
            buffer.cell((x, row_y)).is_some_and(|c| {
                c.fg == Color::Cyan && c.modifier.contains(Modifier::BOLD) && c.symbol() != " "
            })
        });
        assert!(!cyan_bold_present, "the table row must not carry keyword highlighting");

        // Open the detail panel on the same row: the full query DOES carry
        // keyword highlighting there.
        app.detail_open = true;
        terminal
            .draw(|frame| crate::ui::draw(&mut app, frame))
            .expect("draw");
        let buffer = terminal.backend().buffer().clone();
        let width = buffer.area.width;
        let cyan_bold_present = (0..buffer.area.height).any(|y| {
            (0..width).any(|x| {
                buffer.cell((x, y)).is_some_and(|c| {
                    c.fg == Color::Cyan && c.modifier.contains(Modifier::BOLD) && c.symbol() != " "
                })
            })
        });
        assert!(cyan_bold_present, "the detail panel must still highlight SQL keywords");
    }

    #[test]
    fn detail_panel_shows_ssl_security_and_ddl_progress() {
        let mut app = crate::app::App::new();
        app.active_tab = crate::app::Tab::MicroLens;
        let snapshot = app.snapshot.clone();
        crate::app::update(&mut app, crate::app::Action::Snapshot(snapshot));
        let pos = app
            .row_order
            .iter()
            .position(|&i| app.snapshot.activity[i].pid == 4821)
            .expect("mock row 4821 present");
        app.table_state.select(Some(pos));
        app.detail_open = true;

        let backend = ratatui::backend::TestBackend::new(160, 36);
        let mut terminal = ratatui::Terminal::new(backend).expect("test terminal");
        terminal
            .draw(|frame| crate::ui::draw(&mut app, frame))
            .expect("draw");
        let screen: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(screen.contains("security: TLSv1.3"), "{screen}");
        assert!(
            screen.contains("ddl progress: CREATE INDEX CONCURRENTLY on orders"),
            "{screen}"
        );
        assert!(screen.contains("64.2%"), "{screen}");
    }
}
