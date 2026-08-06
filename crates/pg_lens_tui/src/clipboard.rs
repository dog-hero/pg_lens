//! Copy-to-clipboard via the OSC 52 terminal escape sequence (v0.16, `y`).
//!
//! OSC 52 needs no new dependency and — unlike a native clipboard crate —
//! works over SSH, since the terminal EMULATOR (not the remote shell) is the
//! one that owns the system clipboard and reacts to the sequence. Support
//! varies: it works in iTerm2, kitty, WezTerm, Ghostty, and inside tmux with
//! `set -g set-clipboard on`; Apple Terminal does not implement it. Because
//! of that, the toast this module's callers show only ever claims the
//! sequence was SENT ("sent to clipboard (OSC 52)"), never that the paste
//! buffer definitely changed — pg_lens has no way to confirm the terminal
//! actually acted on it, and claiming success unconditionally would be
//! dishonest UI copy (see `PRD.md`'s "never invent data" thread).
//!
//! This module is pure I/O-adjacent: [`osc52_sequence`]/[`cap_payload`]/
//! [`base64_encode`] are pure functions (unit-tested below without a real
//! terminal); [`copy_to_clipboard`] takes a `Write` so tests can capture the
//! bytes into a `Vec<u8>` instead. Callers write the sequence directly to
//! stdout via `main.rs`'s event loop (never from `crates/pg_lens_tui/src/ui/`
//! — the view stays pure and synchronous, per `CLAUDE.md`'s hard invariant).

use std::io::{self, Write};

/// Payload size cap, in bytes of the ORIGINAL text (before base64 inflates
/// it ~4/3x) — large enough for any real query/index definition/column
/// list, small enough that a pathological multi-megabyte statement can't
/// blow up the escape sequence written to the terminal.
pub const CLIPBOARD_MAX_BYTES: usize = 100_000;

/// Truncates `text` to at most [`CLIPBOARD_MAX_BYTES`] bytes, walking back to
/// the nearest char boundary so a multi-byte UTF-8 character never splits —
/// returns `(possibly-truncated text, was_capped)`.
pub fn cap_payload(text: &str) -> (&str, bool) {
    if text.len() <= CLIPBOARD_MAX_BYTES {
        return (text, false);
    }
    let mut end = CLIPBOARD_MAX_BYTES;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    (&text[..end], true)
}

const BASE64_ALPHABET: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// A tiny standard-alphabet, padded base64 encoder (~20 lines) — OSC 52
/// requires the payload base64-encoded, and while the `base64` crate is
/// already in `Cargo.lock` (a transitive dependency of something else), it
/// is not a DIRECT dependency of any pg_lens crate today; declaring it just
/// for this one call site is more ceremony (a new `Cargo.toml` line, a new
/// name in the dependency tree's blast radius) than this well-understood,
/// rarely-changing algorithm is worth.
pub fn base64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0];
        let b1 = chunk.get(1).copied().unwrap_or(0);
        let b2 = chunk.get(2).copied().unwrap_or(0);
        out.push(BASE64_ALPHABET[(b0 >> 2) as usize] as char);
        out.push(BASE64_ALPHABET[(((b0 & 0x03) << 4) | (b1 >> 4)) as usize] as char);
        out.push(if chunk.len() > 1 {
            BASE64_ALPHABET[(((b1 & 0x0f) << 2) | (b2 >> 6)) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            BASE64_ALPHABET[(b2 & 0x3f) as usize] as char
        } else {
            '='
        });
    }
    out
}

/// Builds the full `ESC ]52;c;<base64>BEL` "set clipboard" sequence for
/// `text`, capping the payload first (see [`CLIPBOARD_MAX_BYTES`]). Returns
/// the sequence plus whether it was capped, so the caller can be honest
/// about it in the toast.
pub fn osc52_sequence(text: &str) -> (String, bool) {
    let (capped_text, was_capped) = cap_payload(text);
    let b64 = base64_encode(capped_text.as_bytes());
    (format!("\u{1b}]52;c;{b64}\u{7}"), was_capped)
}

/// Writes the OSC 52 sequence for `text` to `out` and returns the exact
/// toast message to show — honest about a cap, and about this being a
/// "sent", not a confirmed, copy (see the module doc). `text`'s character
/// count is reported pre-truncation (what the operator was looking at), not
/// the capped byte count, since that is what "412 chars" means to a human.
pub fn copy_to_clipboard(out: &mut impl Write, text: &str) -> io::Result<String> {
    let (sequence, was_capped) = osc52_sequence(text);
    out.write_all(sequence.as_bytes())?;
    out.flush()?;
    let chars = text.chars().count();
    Ok(if was_capped {
        format!(
            "sent to clipboard (OSC 52) \u{2014} capped at {CLIPBOARD_MAX_BYTES} bytes \
             ({chars} chars total)"
        )
    } else {
        format!("sent to clipboard (OSC 52) \u{2014} copied {chars} chars")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_encode_matches_known_vectors() {
        // RFC 4648 test vectors.
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
        assert_eq!(base64_encode(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn base64_encode_handles_a_sql_query() {
        let sql = "SELECT * FROM orders WHERE id = 1;";
        let encoded = base64_encode(sql.as_bytes());
        // Round-trip via a minimal decoder check: re-derive the length
        // relationship (4 chars per 3 input bytes, rounded up) rather than
        // hand-writing a decoder just for the test.
        assert_eq!(encoded.len(), sql.len().div_ceil(3) * 4);
        assert!(encoded.chars().all(|c| BASE64_ALPHABET.contains(&(c as u8)) || c == '='));
    }

    #[test]
    fn cap_payload_leaves_small_text_untouched() {
        let (text, capped) = cap_payload("SELECT 1");
        assert_eq!(text, "SELECT 1");
        assert!(!capped);
    }

    #[test]
    fn cap_payload_caps_at_the_byte_limit_on_a_char_boundary() {
        // A multibyte char (é, 2 bytes in UTF-8) sits right at the boundary —
        // the cap must never split it.
        let mut big = "a".repeat(CLIPBOARD_MAX_BYTES - 1);
        big.push('é');
        big.push_str(&"b".repeat(1_000));
        let (text, capped) = cap_payload(&big);
        assert!(capped);
        assert!(text.len() <= CLIPBOARD_MAX_BYTES);
        // Never panics on a non-boundary split, and always valid UTF-8 (the
        // slice itself would have panicked already if it weren't).
        assert!(text.is_char_boundary(text.len()));
    }

    #[test]
    fn osc52_sequence_wraps_base64_in_the_escape_envelope() {
        let (seq, capped) = osc52_sequence("hi");
        assert!(!capped);
        assert_eq!(seq, "\u{1b}]52;c;aGk=\u{7}");
    }

    #[test]
    fn osc52_sequence_reports_the_cap() {
        let big = "x".repeat(CLIPBOARD_MAX_BYTES + 500);
        let (seq, capped) = osc52_sequence(&big);
        assert!(capped);
        // The encoded payload reflects the CAPPED length, not the original.
        let expected_b64_len = CLIPBOARD_MAX_BYTES.div_ceil(3) * 4;
        // seq = ESC ]52;c; <b64> BEL
        let prefix = "\u{1b}]52;c;";
        let b64_part = &seq[prefix.len()..seq.len() - 1];
        assert_eq!(b64_part.len(), expected_b64_len);
    }

    #[test]
    fn copy_to_clipboard_writes_bytes_and_reports_char_count() {
        let mut buf: Vec<u8> = Vec::new();
        let toast = copy_to_clipboard(&mut buf, "SELECT 1").expect("write to Vec never fails");
        assert!(toast.contains("copied 8 chars"), "{toast}");
        assert!(toast.contains("OSC 52"), "{toast}");
        let written = String::from_utf8(buf).expect("valid utf8");
        assert!(written.starts_with("\u{1b}]52;c;"));
        assert!(written.ends_with('\u{7}'));
    }

    #[test]
    fn copy_to_clipboard_reports_the_cap_honestly() {
        let mut buf: Vec<u8> = Vec::new();
        let big = "q".repeat(CLIPBOARD_MAX_BYTES + 42);
        let toast = copy_to_clipboard(&mut buf, &big).expect("write to Vec never fails");
        assert!(toast.contains("capped"), "{toast}");
        assert!(toast.contains(&CLIPBOARD_MAX_BYTES.to_string()), "{toast}");
    }
}
