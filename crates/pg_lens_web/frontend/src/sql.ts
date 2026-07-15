// SQL syntax highlighting for the activity table's query column.
//
// A direct port of the TUI's tokenizer (crates/pg_lens_tui/src/ui/sql.rs) so
// both frontends agree on what lights up. Pure function: SQL string in,
// token list out — DOM construction stays with the caller, and callers must
// render tokens via `textContent` (never innerHTML) since query text comes
// from pg_stat_activity and is attacker-influenceable.
//
// Token classes (cls → CSS class `sql-<cls>` in style.css):
// - "kw"  keywords (case-insensitive, word-boundary): bold cyan;
// - "str" single-quoted strings (with `''` escape): green;
// - "num" numeric literals: magenta — digits inside identifiers (`col1`)
//   or dollar params (`$1`) stay plain;
// - "cmt" `--` comments to end of input: dim;
// - null  everything else (identifiers, operators, ellipsis): default.
//
// Invariant (mirrors the TUI): concatenated token text === input.

/** Same keyword set as the TUI's `KEYWORDS` in ui/sql.rs. */
const KEYWORDS = new Set([
  "SELECT", "INSERT", "UPDATE", "DELETE", "FROM", "WHERE", "JOIN", "LEFT",
  "RIGHT", "INNER", "OUTER", "ON", "GROUP", "BY", "ORDER", "LIMIT", "OFFSET",
  "HAVING", "VALUES", "SET", "INTO", "AS", "AND", "OR", "NOT", "NULL",
  "BEGIN", "COMMIT", "ROLLBACK", "VACUUM", "ANALYZE", "CREATE", "TABLE",
  "INDEX", "DROP", "ALTER", "WITH", "UNION", "ALL", "DISTINCT", "CASE",
  "WHEN", "THEN", "ELSE", "END", "RETURNING",
]);

export type SqlTokenClass = "kw" | "str" | "num" | "cmt";

export interface SqlToken {
  text: string;
  /** null = unstyled (identifiers, operators, whitespace). */
  cls: SqlTokenClass | null;
}

const isAlpha = (c: string): boolean =>
  (c >= "a" && c <= "z") || (c >= "A" && c <= "Z");
const isDigit = (c: string): boolean => c >= "0" && c <= "9";
const isWordChar = (c: string): boolean =>
  isAlpha(c) || isDigit(c) || c === "_" || c === "$";

/**
 * Single-pass tokenizer; unstyled runs are batched into one token so
 * typical output stays small (same shape as the TUI's span list).
 */
export function tokenizeSql(sql: string): SqlToken[] {
  const tokens: SqlToken[] = [];
  let plain = "";

  const flush = (): void => {
    if (plain !== "") {
      tokens.push({ text: plain, cls: null });
      plain = "";
    }
  };

  let i = 0;
  const n = sql.length;
  while (i < n) {
    const c = sql[i] as string;

    // `--` comment: everything to end of input (queries are one line here).
    if (c === "-" && sql[i + 1] === "-") {
      flush();
      tokens.push({ text: sql.slice(i), cls: "cmt" });
      break;
    }

    // Single-quoted string, `''` = escaped quote inside.
    if (c === "'") {
      let j = i + 1;
      while (j < n) {
        if (sql[j] === "'") {
          if (sql[j + 1] === "'") {
            j += 2;
            continue;
          }
          j += 1;
          break;
        }
        j += 1;
      }
      flush();
      tokens.push({ text: sql.slice(i, j), cls: "str" });
      i = j;
      continue;
    }

    // Word: identifier, keyword, or `$n` param. Consuming trailing digits
    // here is what keeps `col1` / `$1` out of the number class.
    if (isAlpha(c) || c === "_" || c === "$") {
      let j = i + 1;
      while (j < n && isWordChar(sql[j] as string)) j += 1;
      const word = sql.slice(i, j);
      if (c !== "$" && KEYWORDS.has(word.toUpperCase())) {
        flush();
        tokens.push({ text: word, cls: "kw" });
      } else {
        plain += word;
      }
      i = j;
      continue;
    }

    // Numeric literal (only reachable at a true token start — a digit after
    // an identifier head was already consumed above).
    if (isDigit(c)) {
      let j = i + 1;
      while (j < n && (isDigit(sql[j] as string) || sql[j] === ".")) j += 1;
      flush();
      tokens.push({ text: sql.slice(i, j), cls: "num" });
      i = j;
      continue;
    }

    plain += c;
    i += 1;
  }
  flush();
  return tokens;
}

/**
 * Render `sql` into `parent` as highlighted spans. XSS-safe by
 * construction: text only ever lands in `textContent` / text nodes.
 */
export function renderSqlInto(parent: HTMLElement, sql: string): void {
  for (const token of tokenizeSql(sql)) {
    if (token.cls === null) {
      parent.append(document.createTextNode(token.text));
    } else {
      const span = document.createElement("span");
      span.className = `sql-${token.cls}`;
      span.textContent = token.text;
      parent.append(span);
    }
  }
}
