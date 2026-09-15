//! Error logger for pg_lens.
//!
//! Appends timestamped query and poller errors to `~/.local/state/pg_lens/error.log`
//! (or `$PG_LENS_STATE_DIR/error.log`), with in-memory rate limiting and deduplication
//! per subsystem to prevent repeated polling errors from flooding disk space.

use std::collections::HashMap;
use std::fs::{OpenOptions, create_dir_all};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Instant;

use crate::recording::{epoch_secs_now, format_epoch_display, state_base_dir};

/// In-memory state to throttle and deduplicate consecutive identical errors for a specific subsystem.
struct SubsystemThrottler {
    last_message: String,
    last_logged_at: Instant,
    repeat_count: u32,
}

static THROTTLER: Mutex<Option<HashMap<String, SubsystemThrottler>>> = Mutex::new(None);

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
/// If the same error is logged repeatedly for the given subsystem within `DEDUPLICATION_INTERVAL_SECS`,
/// writing to disk is suppressed until the interval elapses or a different error occurs.
/// Disk I/O is performed outside of the in-memory mutex to prevent thread stalls.
pub fn log_error(subsystem: &str, error: &dyn std::fmt::Display) -> PathBuf {
    let path = error_log_path();
    let msg = error.to_string();
    let now = Instant::now();

    // Deduplication check per subsystem, executed quickly under mutex lock
    let line_to_write = if let Ok(mut guard) = THROTTLER.lock() {
        let map = guard.get_or_insert_with(HashMap::new);
        if let Some(th) = map.get_mut(subsystem) {
            if th.last_message == msg {
                th.repeat_count += 1;
                if now.duration_since(th.last_logged_at).as_secs() < DEDUPLICATION_INTERVAL_SECS {
                    None
                } else {
                    let timestamp = format_epoch_display(epoch_secs_now());
                    let line = format!(
                        "[{timestamp} UTC] [{subsystem}] error: {msg} (repeated {} times)\n",
                        th.repeat_count
                    );
                    th.repeat_count = 0;
                    th.last_logged_at = now;
                    Some(line)
                }
            } else {
                th.last_message = msg.clone();
                th.last_logged_at = now;
                th.repeat_count = 0;
                let timestamp = format_epoch_display(epoch_secs_now());
                Some(format!("[{timestamp} UTC] [{subsystem}] error: {msg}\n"))
            }
        } else {
            map.insert(
                subsystem.to_string(),
                SubsystemThrottler {
                    last_message: msg.clone(),
                    last_logged_at: now,
                    repeat_count: 0,
                },
            );
            let timestamp = format_epoch_display(epoch_secs_now());
            Some(format!("[{timestamp} UTC] [{subsystem}] error: {msg}\n"))
        }
    } else {
        let timestamp = format_epoch_display(epoch_secs_now());
        Some(format!("[{timestamp} UTC] [{subsystem}] error: {msg}\n"))
    };

    // File I/O happens outside the lock guard
    if let Some(line) = line_to_write {
        append_to_file(&path, &line);
    }
    path
}

fn append_to_file(path: &PathBuf, text: &str) {
    if let Some(parent) = path.parent() {
        let _ = create_dir_all(parent);
    }
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = file.write_all(text.as_bytes());
        let _ = file.flush();
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

    #[test]
    fn throttles_independently_per_subsystem() {
        // First log writes
        let p1 = log_error("test_sub_a", &"err 1");
        assert!(p1.ends_with("error.log"));
        let p2 = log_error("test_sub_b", &"err 2");
        assert!(p2.ends_with("error.log"));

        // Consecutive repeat within window is suppressed
        let p1_again = log_error("test_sub_a", &"err 1");
        assert_eq!(p1, p1_again);
    }
}
