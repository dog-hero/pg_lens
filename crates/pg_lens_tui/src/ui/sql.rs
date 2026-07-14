//! Hand-rolled SQL syntax highlighting for the Micro Lens (no extra crates).
//!
//! A tiny single-pass tokenizer turns a SQL string into styled ratatui
//! spans. It is deliberately lossy-free: the concatenated span contents are
//! always byte-identical to the input, only styles are added. Colors are
//! foreground-only so table row backgrounds/REVERSED-selection still win.
//!
//! Token classes:
//! - keywords (case-insensitive, word-boundary): bold cyan;
//! - single-quoted strings (with `''` escape): green;
//! - numeric literals: magenta — but digits inside identifiers (`col1`) or
//!   dollar params (`$1`) stay default;
//! - `--` comments to end of line: dim gray;
//! - everything else (identifiers, operators, the truncation ellipsis):
//!   default fg.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

/// Uppercase keyword set (word-boundary matched, case-insensitive).
const KEYWORDS: &[&str] = &[
    "SELECT", "INSERT", "UPDATE", "DELETE", "FROM", "WHERE", "JOIN", "LEFT", "RIGHT", "INNER",
    "OUTER", "ON", "GROUP", "BY", "ORDER", "LIMIT", "OFFSET", "HAVING", "VALUES", "SET", "INTO",
    "AS", "AND", "OR", "NOT", "NULL", "BEGIN", "COMMIT", "ROLLBACK", "VACUUM", "ANALYZE",
    "CREATE", "TABLE", "INDEX", "DROP", "ALTER", "WITH", "UNION", "ALL", "DISTINCT", "CASE",
    "WHEN", "THEN", "ELSE", "END", "RETURNING",
];

fn keyword_style() -> Style {
    Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD)
}

fn string_style() -> Style {
    Style::new().fg(Color::Green)
}

fn number_style() -> Style {
    Style::new().fg(Color::Magenta)
}

fn comment_style() -> Style {
    Style::new().fg(Color::DarkGray)
}

/// One line of SQL → a styled [`Line`]. Newlines are not expected here; use
/// [`highlight_lines`] for multi-line text.
pub fn highlight_line(sql: &str) -> Line<'static> {
    Line::from(highlight_spans(sql))
}

/// Multi-line SQL → one styled [`Line`] per input line (for `Paragraph`s).
pub fn highlight_lines(sql: &str) -> Vec<Line<'static>> {
    sql.split('\n').map(highlight_line).collect()
}

/// The tokenizer proper. Single pass over chars; unstyled runs are batched
/// into one span so typical output stays small.
pub fn highlight_spans(sql: &str) -> Vec<Span<'static>> {
    let chars: Vec<char> = sql.chars().collect();
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut plain = String::new();

    fn flush(spans: &mut Vec<Span<'static>>, plain: &mut String) {
        if !plain.is_empty() {
            spans.push(Span::raw(std::mem::take(plain)));
        }
    }

    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];

        // `--` comment: everything to end of input (we work per line).
        if c == '-' && chars.get(i + 1) == Some(&'-') {
            flush(&mut spans, &mut plain);
            let rest: String = chars[i..].iter().collect();
            spans.push(Span::styled(rest, comment_style()));
            break;
        }

        // Single-quoted string, `''` = escaped quote inside.
        if c == '\'' {
            let mut j = i + 1;
            while j < chars.len() {
                if chars[j] == '\'' {
                    if chars.get(j + 1) == Some(&'\'') {
                        j += 2;
                        continue;
                    }
                    j += 1;
                    break;
                }
                j += 1;
            }
            flush(&mut spans, &mut plain);
            let literal: String = chars[i..j].iter().collect();
            spans.push(Span::styled(literal, string_style()));
            i = j;
            continue;
        }

        // Word: identifier, keyword, or `$n` param. Consuming trailing
        // digits here is what keeps `col1` / `$1` out of the number class.
        if c.is_ascii_alphabetic() || c == '_' || c == '$' {
            let mut j = i + 1;
            while j < chars.len()
                && (chars[j].is_ascii_alphanumeric() || chars[j] == '_' || chars[j] == '$')
            {
                j += 1;
            }
            let word: String = chars[i..j].iter().collect();
            if c != '$' && KEYWORDS.contains(&word.to_ascii_uppercase().as_str()) {
                flush(&mut spans, &mut plain);
                spans.push(Span::styled(word, keyword_style()));
            } else {
                plain.push_str(&word);
            }
            i = j;
            continue;
        }

        // Numeric literal (only reachable at a true token start — a digit
        // after an identifier head was already consumed above).
        if c.is_ascii_digit() {
            let mut j = i + 1;
            while j < chars.len() && (chars[j].is_ascii_digit() || chars[j] == '.') {
                j += 1;
            }
            flush(&mut spans, &mut plain);
            let number: String = chars[i..j].iter().collect();
            spans.push(Span::styled(number, number_style()));
            i = j;
            continue;
        }

        plain.push(c);
        i += 1;
    }
    flush(&mut spans, &mut plain);
    spans
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Concatenated span text must always equal the input (styles only).
    fn text_of(spans: &[Span<'_>]) -> String {
        spans.iter().map(|s| s.content.as_ref()).collect()
    }

    fn styled<'a>(spans: &'a [Span<'a>], style: Style) -> Vec<&'a str> {
        spans
            .iter()
            .filter(|s| s.style == style)
            .map(|s| s.content.as_ref())
            .collect()
    }

    #[test]
    fn keywords_highlight_at_start_and_middle_case_insensitively() {
        let sql = "select balance FROM accounts where id = 7";
        let spans = highlight_spans(sql);
        assert_eq!(text_of(&spans), sql);
        assert_eq!(
            styled(&spans, keyword_style()),
            vec!["select", "FROM", "where"]
        );
        // The number literal is its own class.
        assert_eq!(styled(&spans, number_style()), vec!["7"]);
    }

    #[test]
    fn quoted_strings_are_green_even_when_they_contain_keywords() {
        let sql = "SELECT 'DELETE FROM users' AS note";
        let spans = highlight_spans(sql);
        assert_eq!(text_of(&spans), sql);
        assert_eq!(styled(&spans, string_style()), vec!["'DELETE FROM users'"]);
        // DELETE/FROM inside the string must NOT appear as keyword spans.
        assert_eq!(styled(&spans, keyword_style()), vec!["SELECT", "AS"]);
    }

    #[test]
    fn escaped_quote_stays_inside_the_string() {
        let sql = "SELECT 'it''s' FROM t";
        let spans = highlight_spans(sql);
        assert_eq!(text_of(&spans), sql);
        assert_eq!(styled(&spans, string_style()), vec!["'it''s'"]);
        assert_eq!(styled(&spans, keyword_style()), vec!["SELECT", "FROM"]);
    }

    #[test]
    fn digits_inside_identifiers_and_dollar_params_are_not_numbers() {
        let sql = "SELECT col1, $1 FROM t2 WHERE n = 42";
        let spans = highlight_spans(sql);
        assert_eq!(text_of(&spans), sql);
        // Only the standalone literal is magenta.
        assert_eq!(styled(&spans, number_style()), vec!["42"]);
        // col1 / $1 / t2 stay default (batched into raw spans).
        let raw: String = styled(&spans, Style::new()).concat();
        assert!(raw.contains("col1"));
        assert!(raw.contains("$1"));
        assert!(raw.contains("t2"));
    }

    #[test]
    fn keyword_needs_a_word_boundary() {
        // "selection" and "unfrom" must not light up.
        let sql = "selection unfrom FROMAGE from x";
        let spans = highlight_spans(sql);
        assert_eq!(text_of(&spans), sql);
        assert_eq!(styled(&spans, keyword_style()), vec!["from"]);
    }

    #[test]
    fn line_comment_dims_to_end_of_line() {
        let sql = "SELECT 1 -- DELETE everything 'later'";
        let spans = highlight_spans(sql);
        assert_eq!(text_of(&spans), sql);
        assert_eq!(
            styled(&spans, comment_style()),
            vec!["-- DELETE everything 'later'"]
        );
        assert_eq!(styled(&spans, keyword_style()), vec!["SELECT"]);
    }

    #[test]
    fn decimal_numbers_and_truncation_ellipsis_survive() {
        let sql = "WHERE price > 19.99 AND abandoned_at < now() \u{2026}";
        let spans = highlight_spans(sql);
        assert_eq!(text_of(&spans), sql);
        assert_eq!(styled(&spans, number_style()), vec!["19.99"]);
        // The ellipsis stays in a default-styled span.
        let raw: String = styled(&spans, Style::new()).concat();
        assert!(raw.contains('\u{2026}'));
    }

    #[test]
    fn multiline_input_yields_one_line_per_input_line() {
        let lines = highlight_lines("SELECT *\nFROM t\n-- done");
        assert_eq!(lines.len(), 3);
        assert_eq!(
            lines[1].spans.iter().map(|s| s.content.as_ref()).collect::<String>(),
            "FROM t"
        );
    }
}
