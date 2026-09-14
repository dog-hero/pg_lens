//! Error logger for pg_lens.
//!
//! Appends timestamped query and poller errors to `~/.local/state/pg_lens/error.log`
//! (or `$PG_LENS_STATE_DIR/error.log`), with in-memory rate limiting and deduplication
//! to prevent repeated polling errors from flooding disk space.

use std::fs::{OpenOptions, create_dir_all};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Instant;

use crate::recording::{epoch_secs_now, format_epoch_display, state_base_dir};

/// In-memory state to throttle and deduplicate consecutive identical errors.
struct ErrorThrottler {
    last_subsystem: String,
    last_message: String,
    last_logged_at: Instant,
    repeat_count: u32,
}

static THROTTLER: Mutex<Option<ErrorThrottler>> = Mutex::new(None);

/// Minimum interval between logging the exact same error for the same subsystem (default: 30s).
const DEDUPLICATION_INTERVAL_SECS: u64 = 30;

/// Returns the path to `error.log`.
pub fn error_log_path() -> PathBuf {
    state_base_dir()
        .unwrap_or_else(|| std::env::temp_dir().join("pg_lens"))
        .join("error.log")
}

/// Appends an error to `error.log` and returns the file path.
///
/// If the same error is logged repeatedly within `DEDUPLICATION_INTERVAL_SECS`,
/// writing to disk is suppressed until the interval elapses or a different error occurs.
pub fn log_error(subsystem: &str, error: &dyn std::fmt::Display) -> PathBuf {
    let path = error_log_path();
    let msg = error.to_string();
    let now = Instant::now();

    // Deduplication check
    if let Ok(mut guard) = THROTTLER.lock() {
        if let Some(ref mut th) = *guard {
            if th.last_subsystem == subsystem && th.last_message == msg {
                th.repeat_count += 1;
                if now.duration_since(th.last_logged_at).as_secs() < DEDUPLICATION_INTERVAL_SECS {
                    return path;
                }
                // Write summary with repeat count
                let timestamp = format_epoch_display(epoch_secs_now());
                let line = format!(
                    "[{timestamp} UTC] [{subsystem}] error: {msg} (repeated {} times)\n",
                    th.repeat_count
                );
                th.repeat_count = 0;
                th.last_logged_at = now;
                append_to_file(&path, &line);
                return path;
            }
        }
        *guard = Some(ErrorThrottler {
            last_subsystem: subsystem.to_string(),
            last_message: msg.clone(),
            last_logged_at: now,
            repeat_count: 0,
        });
    }

    let timestamp = format_epoch_display(epoch_secs_now());
    let line = format!("[{timestamp} UTC] [{subsystem}] error: {msg}\n");
    append_to_file(&path, &line);
    path
}

fn append_to_file(path: &PathBuf, text: &str) {
    if let Some(parent) = path.parent() {
        let _ = create_dir_all(parent);
    }
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = file.write_all(text.as_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_log_path_resolves_filename() {
        let p = error_log_path();
        assert!(p.ends_with("error.log"));
    }

    #[test]
    fn logs_error_and_formats_entry() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let log_file = temp_dir.path().join("error.log");
        let line = "[test UTC] [mock] error: test error\n".to_string();
        append_to_file(&log_file, &line);

        let content = std::fs::read_to_string(&log_file).expect("read");
        assert!(content.contains("[test UTC] [mock] error: test error"));
    }
}
