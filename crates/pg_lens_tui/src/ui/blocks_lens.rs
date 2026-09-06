//! Blocks & Locks Lens (v0.17): hierarchical wait-for tree (who blocks whom)
//! and active database lock inventory (`pg_locks`).
//!
//! Split-pane layout:
//! - Top: Hierarchical Blocking Tree with Root Blockers, deadlock detection,
//!   and dependency depth.
//! - Bottom: Active Locks Table showing granted vs waiting lock modes, relations,
//!   and query snippets.

use pg_lens_core::BlockTreeNode;
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Row, Table, Wrap},
};

use crate::app::{App, BlocksPane};
use crate::ui::{format, sql};

pub struct FlattenedNode<'a> {
    pub depth: usize,
    pub node: &'a BlockTreeNode,
    pub is_last_child: bool,
}

pub fn flatten_tree<'a>(nodes: &'a [BlockTreeNode]) -> Vec<FlattenedNode<'a>> {
    let mut out = Vec::new();
    fn walk<'a>(nodes: &'a [BlockTreeNode], depth: usize, out: &mut Vec<FlattenedNode<'a>>) {
        let count = nodes.len();
        for (i, node) in nodes.iter().enumerate() {
            out.push(FlattenedNode {
                depth,
                node,
                is_last_child: i + 1 == count,
            });
            walk(&node.children, depth + 1, out);
        }
    }
    walk(nodes, 0, &mut out);
    out
}

pub fn draw(app: &mut App, frame: &mut Frame, area: Rect) {
    let [tree_area, locks_area] = Layout::vertical([
        Constraint::Percentage(55),
        Constraint::Percentage(45),
    ])
    .areas(area);

    draw_tree(app, frame, tree_area);
    draw_locks(app, frame, locks_area);

    if app.detail_open {
        draw_detail(app, frame, area);
    }
}

fn draw_tree(app: &mut App, frame: &mut Frame, area: Rect) {
    let is_active_pane = app.blocks_active_pane == BlocksPane::Tree;
    let border_style = if is_active_pane {
        Style::new().fg(Color::Yellow)
    } else {
        Style::new().dim()
    };

    let tree_data = app.snapshot.blocking_tree.as_deref().unwrap_or(&[]);
    let flattened = flatten_tree(tree_data);

    let title = format!(
        " Blocking Tree ({} roots, {} blocked) ",
        tree_data.len(),
        flattened.iter().filter(|f| !f.node.is_root).count()
    );

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(Span::styled(title, Style::new().bold()));

    if flattened.is_empty() {
        let empty = Paragraph::new(Line::from("  No blocked sessions detected \u{2014} all queries running freely.").dim())
            .block(block);
        frame.render_widget(empty, area);
        return;
    }

    let rows: Vec<Row> = flattened
        .iter()
        .map(|item| {
            let n = item.node;
            let mut prefix = String::new();
            if item.depth > 0 {
                for _ in 0..(item.depth - 1) {
                    prefix.push_str("   ");
                }
                if item.is_last_child {
                    prefix.push_str(" └─ ");
                } else {
                    prefix.push_str(" ├─ ");
                }
            }

            let pid_span = if n.is_deadlock {
                Span::styled(format!("{prefix}[DEADLOCK] PID {}", n.pid), Style::new().fg(Color::Red).bold())
            } else if n.is_root {
                Span::styled(
                    format!("{prefix}[ROOT BLOCKER] PID {} ({} waiters)", n.pid, n.num_descendants),
                    Style::new().fg(Color::Red).bold(),
                )
            } else {
                Span::styled(format!("{prefix}PID {}", n.pid), Style::new().fg(Color::Yellow))
            };

            let user_app = format!("{}@{}", n.usename, n.application_name);
            let state_style = if n.state == "idle in transaction" {
                Style::new().fg(Color::Yellow).bold()
            } else if n.state == "active" {
                Style::new().fg(Color::Green)
            } else {
                Style::new().dim()
            };

            let lock_info = if let (Some(m), Some(r)) = (&n.mode, &n.relation) {
                if !r.is_empty() {
                    format!("{m} on {r}")
                } else {
                    m.clone()
                }
            } else if let Some(m) = &n.mode {
                m.clone()
            } else {
                "-".to_string()
            };

            let dur = format::human_duration(n.duration_secs);

            Row::new(vec![
                Line::from(pid_span),
                Line::from(Span::styled(user_app, Style::new().dim())),
                Line::from(Span::styled(n.state.clone(), state_style)),
                Line::from(Span::styled(lock_info, Style::new().cyan())),
                Line::from(Span::styled(dur, Style::new().dim())),
                Line::from(Span::raw(n.query.replace('\n', " "))),
            ])
        })
        .collect();

    let widths = [
        Constraint::Length(34),
        Constraint::Length(18),
        Constraint::Length(20),
        Constraint::Length(24),
        Constraint::Length(8),
        Constraint::Min(20),
    ];

    let highlight_style = if is_active_pane {
        Style::new().add_modifier(Modifier::REVERSED).bold()
    } else {
        Style::new().bg(Color::DarkGray)
    };

    let table = Table::new(rows, widths)
        .block(block)
        .header(
            Row::new(vec![
                "Session / Hierarchy",
                "User@App",
                "State",
                "Awaiting Lock",
                "Age",
                "Query",
            ])
            .style(Style::new().bold().dim()),
        )
        .row_highlight_style(highlight_style);

    frame.render_stateful_widget(table, area, &mut app.blocks_tree_state);
}

fn draw_locks(app: &mut App, frame: &mut Frame, area: Rect) {
    let is_active_pane = app.blocks_active_pane == BlocksPane::Locks;
    let border_style = if is_active_pane {
        Style::new().fg(Color::Yellow)
    } else {
        Style::new().dim()
    };

    let locks = app.snapshot.active_locks.as_deref().unwrap_or(&[]);

    let title = format!(" Active Locks ({} entries) ", locks.len());

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(Span::styled(title, Style::new().bold()));

    if locks.is_empty() {
        let empty = Paragraph::new(Line::from("  No active relation or transaction locks in database.").dim())
            .block(block);
        frame.render_widget(empty, area);
        return;
    }

    let rows: Vec<Row> = locks
        .iter()
        .map(|l| {
            let pid_span = Span::styled(l.pid.to_string(), Style::new().bold());

            let (status_span, row_style) = if !l.granted {
                (
                    Span::styled("WAIT", Style::new().fg(Color::Red).bold()),
                    Style::new().fg(Color::LightRed),
                )
            } else {
                (
                    Span::styled("GRANT", Style::new().fg(Color::Green)),
                    Style::new(),
                )
            };

            let rel = if !l.relation.is_empty() {
                if !l.schema.is_empty() && l.schema != "public" {
                    format!("{}.{}", l.schema, l.relation)
                } else {
                    l.relation.clone()
                }
            } else {
                "-".to_string()
            };

            let dur = format::human_duration(l.duration_secs);

            Row::new(vec![
                Line::from(pid_span),
                Line::from(Span::raw(l.locktype.clone())),
                Line::from(Span::styled(rel, Style::new().cyan())),
                Line::from(Span::raw(l.mode.clone())),
                Line::from(status_span),
                Line::from(Span::styled(dur, Style::new().dim())),
                Line::from(Span::styled(l.usename.clone(), Style::new().dim())),
                Line::from(Span::raw(l.query.replace('\n', " "))),
            ])
            .style(row_style)
        })
        .collect();

    let widths = [
        Constraint::Length(8),
        Constraint::Length(14),
        Constraint::Length(22),
        Constraint::Length(24),
        Constraint::Length(8),
        Constraint::Length(8),
        Constraint::Length(12),
        Constraint::Min(20),
    ];

    let highlight_style = if is_active_pane {
        Style::new().add_modifier(Modifier::REVERSED).bold()
    } else {
        Style::new().bg(Color::DarkGray)
    };

    let table = Table::new(rows, widths)
        .block(block)
        .header(
            Row::new(vec![
                "PID",
                "Type",
                "Relation",
                "Mode",
                "Status",
                "Age",
                "User",
                "Query",
            ])
            .style(Style::new().bold().dim()),
        )
        .row_highlight_style(highlight_style);

    frame.render_stateful_widget(table, area, &mut app.blocks_locks_state);
}

fn draw_detail(app: &App, frame: &mut Frame, area: Rect) {
    let [_, panel_area] =
        Layout::vertical([Constraint::Percentage(40), Constraint::Percentage(60)]).areas(area);

    let (title, query, extra_lines) = match app.blocks_active_pane {
        BlocksPane::Tree => {
            let Some(node) = app.selected_block_node() else {
                return;
            };
            let lock_info = if let (Some(m), Some(r)) = (&node.mode, &node.relation) {
                if !r.is_empty() {
                    format!("{m} on {r}")
                } else {
                    m.clone()
                }
            } else if let Some(m) = &node.mode {
                m.clone()
            } else {
                "-".to_string()
            };
            let title = format!(
                "Detail \u{2014} PID {} \u{2502} {}@{} \u{2502} {} \u{2502} {} (Enter/Esc: close)",
                node.pid,
                node.usename,
                node.application_name,
                node.state,
                format::human_duration(node.duration_secs)
            );
            let mut lines = Vec::new();
            if node.is_deadlock {
                lines.push(
                    Line::from("DEADLOCK DETECTED IN THIS CYCLE").style(Style::new().fg(Color::Red).bold()),
                );
            } else if node.is_root {
                lines.push(
                    Line::from(format!(
                        "ROOT BLOCKER \u{2014} blocking {} descendant session(s)",
                        node.num_descendants
                    ))
                    .style(Style::new().fg(Color::Red).bold()),
                );
            }
            lines.push(Line::from(vec![
                Span::styled("Awaiting lock: ", Style::new().dim()),
                Span::styled(lock_info, Style::new().cyan()),
            ]));
            (title, node.query.clone(), lines)
        }
        BlocksPane::Locks => {
            let Some(lock) = app.selected_active_lock() else {
                return;
            };
            let title = format!(
                "Detail \u{2014} Lock on PID {} \u{2502} {} \u{2502} {} (Enter/Esc: close)",
                lock.pid,
                lock.locktype,
                if lock.granted { "GRANTED" } else { "WAITING" }
            );
            let mut lines = Vec::new();
            lines.push(Line::from(vec![
                Span::styled("Mode: ", Style::new().dim()),
                Span::raw(lock.mode.clone()),
                Span::styled(" \u{2502} Target: ", Style::new().dim()),
                Span::styled(
                    if lock.relation.is_empty() {
                        "-".to_string()
                    } else {
                        lock.relation.clone()
                    },
                    Style::new().cyan(),
                ),
                Span::styled(" \u{2502} Duration: ", Style::new().dim()),
                Span::raw(format::human_duration(lock.duration_secs)),
                Span::styled(" \u{2502} User: ", Style::new().dim()),
                Span::raw(lock.usename.clone()),
            ]));
            (title, lock.query.clone(), lines)
        }
    };

    let mut lines = extra_lines;
    lines.push(Line::default());
    lines.extend(sql::highlight_lines(&query));

    let panel = Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .block(Block::bordered().title(title));
    frame.render_widget(Clear, panel_area);
    frame.render_widget(panel, panel_area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    #[test]
    fn blocks_lens_renders_tree_and_locks_from_mock() {
        let mut app = App::new();
        app.active_tab = crate::app::Tab::BlocksLens;

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

        assert!(screen.contains("Blocking Tree"));
        assert!(screen.contains("Active Locks"));
        // Mock snapshot has root blocker 4312
        assert!(screen.contains("4312"), "{screen}");
        assert!(screen.contains("ROOT BLOCKER"), "{screen}");
        assert!(screen.contains("WAIT"), "{screen}");
    }

    #[test]
    fn blocks_lens_detail_panel_renders() {
        let mut app = App::new();
        app.active_tab = crate::app::Tab::BlocksLens;
        app.detail_open = true;
        app.blocks_tree_state.select(Some(0));

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

        assert!(screen.contains("Detail \u{2014} PID 4312"), "{screen}");
        assert!(screen.contains("ROOT BLOCKER"), "{screen}");
    }
}
