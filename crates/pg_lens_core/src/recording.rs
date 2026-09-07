//! Snapshot export and incident recording (Flight Recorder).
//!
//! Provides two complementary capabilities for post-mortem analysis and incident sharing:
//! 1. **Snapshot Export**: serializes a single point-in-time [`DbSnapshot`] as pretty JSON
//!    into `~/.local/state/pg_lens/exports/snapshot-<target>-<timestamp>.json`.
//! 2. **Incident Recording (Record Mode)**: streams consecutive [`DbSnapshot`] frames as JSONL
//!    into `~/.local/state/pg_lens/recordings/rec-<target>-<timestamp>.jsonl`.
//! 3. **Recording Reader**: loads `.jsonl` recordings (or single `.json` snapshots) back into
//!    memory for interactive offline playback (`pg_lens replay <file>`).

use std::fs::{File, create_dir_all};
use std::io::{BufRead, BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;

use crate::models::DbSnapshot;

/// Default state directory for exported single snapshots.
pub fn exports_dir() -> Option<PathBuf> {
    state_base_dir().map(|base| base.join("exports"))
}

/// Default state directory for incident recordings.
pub fn recordings_dir() -> Option<PathBuf> {
    state_base_dir().map(|base| base.join("recordings"))
}

fn is_permission_denied(err: &std::io::Error) -> bool {
    err.kind() == std::io::ErrorKind::PermissionDenied || err.raw_os_error() == Some(1)
}

fn state_base_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("PG_LENS_STATE_DIR") {
        return Some(PathBuf::from(dir));
    }
    if let Some(dir) = std::env::var_os("XDG_STATE_HOME") {
        return Some(PathBuf::from(dir).join("pg_lens"));
    }
    if let Some(home) = std::env::var_os("HOME") {
        return Some(PathBuf::from(home).join(".local").join("state").join("pg_lens"));
    }
    Some(std::env::temp_dir().join("pg_lens"))
}

/// Default max bytes for recording split: 100 MB.
pub const DEFAULT_MAX_RECORD_BYTES: usize = 100 * 1024 * 1024;

/// Sanitizes target label for safe use in file names.
pub fn sanitize_target_name(target: &str) -> String {
    let sanitized: String = target
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect();
    if sanitized.is_empty() {
        "postgres".to_string()
    } else {
        sanitized
    }
}

/// Formats epoch seconds as `YYYYMMDD_HHMMSS`.
pub fn format_epoch_slug(secs: u64) -> String {
    let days = secs / 86400;
    let rem_secs = secs % 86400;
    let hours = rem_secs / 3600;
    let minutes = (rem_secs % 3600) / 60;
    let seconds = rem_secs % 60;

    let z = days as i64 + 719468;
    let era = (if z >= 0 { z } else { z - 146096 }) / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };

    format!("{y:04}{m:02}{d:02}_{hours:02}{minutes:02}{seconds:02}")
}

/// Formats epoch seconds as `YYYY-MM-DD HH:MM:SS`.
pub fn format_epoch_display(secs: u64) -> String {
    let days = secs / 86400;
    let rem_secs = secs % 86400;
    let hours = rem_secs / 3600;
    let minutes = (rem_secs % 3600) / 60;
    let seconds = rem_secs % 60;

    let z = days as i64 + 719468;
    let era = (if z >= 0 { z } else { z - 146096 }) / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };

    format!("{y:04}-{m:02}-{d:02} {hours:02}:{minutes:02}:{seconds:02}")
}

/// Formats seconds as a human-readable duration (e.g. `14m 20s`, `1h 05m 12s`, `42s`).
pub fn format_duration_secs(secs: u64) -> String {
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m {:02}s", secs / 60, secs % 60)
    } else {
        format!("{}h {:02}m {:02}s", secs / 3600, (secs % 3600) / 60, secs % 60)
    }
}

/// Parses timestamp from string (either `YYYYMMDD_HHMMSS` or raw epoch seconds).
pub fn parse_epoch_timestamp(s: &str) -> Option<u64> {
    if s.len() == 15 && s.as_bytes().get(8) == Some(&b'_') {
        let y: i64 = s[0..4].parse().ok()?;
        let m: u64 = s[4..6].parse().ok()?;
        let d: u64 = s[6..8].parse().ok()?;
        let h: u64 = s[9..11].parse().ok()?;
        let min: u64 = s[11..13].parse().ok()?;
        let sec: u64 = s[13..15].parse().ok()?;

        if !(1..=12).contains(&m) || !(1..=31).contains(&d) || h >= 24 || min >= 60 || sec >= 60 {
            return None;
        }

        let y = if m <= 2 { y - 1 } else { y };
        let era = (if y >= 0 { y } else { y - 399 }) / 400;
        let yoe = (y - era * 400) as u64;
        let mp = if m > 2 { m - 3 } else { m + 9 };
        let doy = (153 * mp + 2) / 5 + d - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        let days = era * 146097 + doe as i64 - 719468;
        if days < 0 {
            return None;
        }
        return Some(days as u64 * 86400 + h * 3600 + min * 60 + sec);
    }
    s.parse::<u64>().ok()
}

/// Returns current wall-clock epoch in seconds.
pub fn epoch_secs_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Formats current wall-clock time as a compact slug (`YYYYMMDD_HHMMSS`).
pub fn timestamp_slug() -> String {
    format_epoch_slug(epoch_secs_now())
}

/// Exports a single [`DbSnapshot`] to pretty JSON in the specified directory.
///
/// Returns the canonical path of the written file.
pub fn export_snapshot_to(
    snapshot: &DbSnapshot,
    target_label: &str,
    dir: &Path,
) -> std::io::Result<PathBuf> {
    create_dir_all(dir)?;
    let clean_target = sanitize_target_name(target_label);
    let ts = timestamp_slug();
    let filename = format!("snapshot-{clean_target}-{ts}.json");
    let path = dir.join(filename);

    let json = serde_json::to_string_pretty(snapshot).map_err(|e| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())
    })?;

    let mut file = File::create(&path)?;
    file.write_all(json.as_bytes())?;
    file.flush()?;

    Ok(path)
}

/// Exports a single [`DbSnapshot`] to pretty JSON in the exports directory.
///
/// Returns the canonical path of the written file.
pub fn export_snapshot(snapshot: &DbSnapshot, target_label: &str) -> std::io::Result<PathBuf> {
    let dir = exports_dir().unwrap_or_else(|| std::env::temp_dir().join("pg_lens").join("exports"));
    match export_snapshot_to(snapshot, target_label, &dir) {
        Ok(path) => Ok(path),
        Err(e) if is_permission_denied(&e) => {
            let fallback = std::env::temp_dir().join("pg_lens").join("exports");
            export_snapshot_to(snapshot, target_label, &fallback)
        }
        Err(e) => Err(e),
    }
}

/// Kind of recorded file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum RecordingKind {
    /// Multi-frame continuous recording (`.jsonl`).
    Recording,
    /// Single snapshot point-in-time bookmark (`.json`).
    Bookmark,
}

/// Information about a recorded incident file or snapshot bookmark on disk.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RecordingEntry {
    pub path: PathBuf,
    pub filename: String,
    pub target: String,
    pub kind: RecordingKind,
    pub size_bytes: u64,
    pub started_at_secs: Option<u64>,
    pub ended_at_secs: Option<u64>,
    pub started_at: Option<String>,
    pub ended_at: Option<String>,
    pub duration_secs: Option<u64>,
    pub frame_count: Option<usize>,
    pub is_active: bool,
}

/// Parses metadata from a recording or snapshot filename.
pub fn parse_recording_filename(filename: &str) -> (String, RecordingKind, Option<u64>, Option<u64>) {
    let name = filename.strip_suffix(".gz").unwrap_or(filename);
    if name.starts_with("snapshot-") && name.ends_with(".json") {
        let stem = &name["snapshot-".len()..name.len() - ".json".len()];
        if let Some((target, ts_str)) = stem.rsplit_once('-') {
            let ts = parse_epoch_timestamp(ts_str);
            return (target.to_string(), RecordingKind::Bookmark, ts, ts);
        }
        return (stem.to_string(), RecordingKind::Bookmark, None, None);
    }

    if name.starts_with("rec-") && name.ends_with(".jsonl") {
        let stem = &name["rec-".len()..name.len() - ".jsonl".len()];
        if let Some((before_to, end_str)) = stem.split_once("-to-") {
            let end_ts = parse_epoch_timestamp(end_str);
            if let Some((target, start_str)) = before_to.rsplit_once('-') {
                let start_ts = parse_epoch_timestamp(start_str);
                return (target.to_string(), RecordingKind::Recording, start_ts, end_ts);
            }
            return (before_to.to_string(), RecordingKind::Recording, None, end_ts);
        }

        if let Some((target, start_str)) = stem.rsplit_once('-') {
            let start_ts = parse_epoch_timestamp(start_str);
            return (target.to_string(), RecordingKind::Recording, start_ts, None);
        }
        return (stem.to_string(), RecordingKind::Recording, None, None);
    }

    (filename.to_string(), RecordingKind::Recording, None, None)
}

/// Deletes a recording or bookmark file from disk.
pub fn delete_recording(path: &Path) -> std::io::Result<()> {
    std::fs::remove_file(path)
}

/// Scans the recordings and exports directories for all available recorded files.
pub fn list_recordings(
    recordings_dir: &Path,
    exports_dir: &Path,
    active_path: Option<&Path>,
) -> Vec<RecordingEntry> {
    let mut entries = Vec::new();

    let scan_dir = |dir: &Path, entries: &mut Vec<RecordingEntry>| {
        let Ok(read_dir) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in read_dir.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let filename = path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();

            let is_jsonl = filename.ends_with(".jsonl") || filename.ends_with(".jsonl.gz");
            let is_json = filename.ends_with(".json") || filename.ends_with(".json.gz");
            if !is_jsonl && !is_json {
                continue;
            }

            let meta = match path.metadata() {
                Ok(m) => m,
                Err(_) => continue,
            };
            let size_bytes = meta.len();
            let mtime_secs = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_secs());

            let (target, kind, mut start_secs, mut end_secs) = parse_recording_filename(&filename);

            if start_secs.is_none() {
                start_secs = mtime_secs;
            }
            let is_active = active_path.is_some_and(|p| p == path);
            if end_secs.is_none() && !is_active {
                end_secs = mtime_secs;
            }

            let duration_secs = match (start_secs, end_secs) {
                (Some(s), Some(e)) if e >= s => Some(e - s),
                _ => None,
            };

            let started_at = start_secs.map(format_epoch_display);
            let ended_at = if is_active {
                Some("● recording...".to_string())
            } else {
                end_secs.map(format_epoch_display)
            };

            // Estimate frames if json bookmark = 1, or count lines if small
            let frame_count = if kind == RecordingKind::Bookmark {
                Some(1)
            } else if size_bytes < 2 * 1024 * 1024 {
                // Quick line count for files under 2MB (decompressed if .gz)
                File::open(&path).ok().map(|f| {
                    if filename.ends_with(".gz") {
                        BufReader::new(GzDecoder::new(f))
                            .lines()
                            .map_while(Result::ok)
                            .filter(|l| !l.trim().is_empty())
                            .count()
                    } else {
                        BufReader::new(f)
                            .lines()
                            .map_while(Result::ok)
                            .filter(|l| !l.trim().is_empty())
                            .count()
                    }
                })
            } else {
                None
            };

            entries.push(RecordingEntry {
                path,
                filename,
                target,
                kind,
                size_bytes,
                started_at_secs: start_secs,
                ended_at_secs: end_secs,
                started_at,
                ended_at,
                duration_secs,
                frame_count,
                is_active,
            });
        }
    };

    scan_dir(recordings_dir, &mut entries);
    scan_dir(exports_dir, &mut entries);

    // Sort newest start timestamp first
    entries.sort_by_key(|a| std::cmp::Reverse(a.started_at_secs));
    entries
}

/// Result of an auto-pruning run.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PruneResult {
    pub files_deleted: usize,
    pub bytes_freed: u64,
}

/// Prunes old recordings by age (days) and by total size quota (FIFO).
///
/// Never prunes the active in-progress recording file (`active_path`).
pub fn prune_recordings(
    recordings_dir: &Path,
    exports_dir: &Path,
    max_total_bytes: Option<u64>,
    retention_days: Option<u64>,
    active_path: Option<&Path>,
) -> std::io::Result<PruneResult> {
    if max_total_bytes.is_none() && retention_days.is_none() {
        return Ok(PruneResult::default());
    }

    let mut result = PruneResult::default();
    let entries = list_recordings(recordings_dir, exports_dir, active_path);
    let now_secs = epoch_secs_now();

    // Filter out active recording
    let mut candidates: Vec<RecordingEntry> = entries
        .into_iter()
        .filter(|e| !e.is_active)
        .collect();

    // Sort oldest first (started_at_secs ascending)
    candidates.sort_by_key(|e| e.started_at_secs.unwrap_or(0));

    // Phase 1: Prune by age if retention_days is configured
    if let Some(days) = retention_days {
        let max_age_secs = days * 86400;
        let mut remaining = Vec::new();
        for entry in candidates {
            let file_age_secs = entry.started_at_secs.map(|s| now_secs.saturating_sub(s));
            if let Some(age) = file_age_secs {
                if age > max_age_secs {
                    if delete_recording(&entry.path).is_ok() {
                        result.files_deleted += 1;
                        result.bytes_freed += entry.size_bytes;
                    }
                    continue;
                }
            }
            remaining.push(entry);
        }
        candidates = remaining;
    }

    // Phase 2: Prune by total size if max_total_bytes is configured
    if let Some(max_bytes) = max_total_bytes {
        let mut total_bytes: u64 = candidates.iter().map(|e| e.size_bytes).sum();
        for entry in candidates {
            if total_bytes <= max_bytes {
                break;
            }
            if delete_recording(&entry.path).is_ok() {
                result.files_deleted += 1;
                result.bytes_freed += entry.size_bytes;
                total_bytes = total_bytes.saturating_sub(entry.size_bytes);
            }
        }
    }

    Ok(result)
}

enum RecordSink {
    Plain(BufWriter<File>),
    Gz(GzEncoder<BufWriter<File>>),
}

impl std::fmt::Debug for RecordSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Plain(_) => write!(f, "RecordSink::Plain"),
            Self::Gz(_) => write!(f, "RecordSink::Gz"),
        }
    }
}

impl Write for RecordSink {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            Self::Plain(w) => w.write(buf),
            Self::Gz(w) => w.write(buf),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Self::Plain(w) => w.flush(),
            Self::Gz(w) => w.flush(),
        }
    }
}

impl RecordSink {
    fn finish(self) -> std::io::Result<()> {
        match self {
            Self::Plain(mut w) => w.flush(),
            Self::Gz(w) => {
                let mut inner = w.finish()?;
                inner.flush()
            }
        }
    }
}

/// Sequential JSONL recording session with automatic size-based rotation and optional gzip compression.
#[derive(Debug)]
pub struct RecordingWriter {
    target: String,
    dir: PathBuf,
    max_bytes: usize,
    compress: bool,
    current_file_start_secs: u64,
    is_custom_path: bool,
    path: PathBuf,
    sink: Option<RecordSink>,
    frame_count: usize,
    bytes_written: usize,
    total_session_frames: usize,
    total_session_bytes: usize,
    rotation_count: usize,
}

impl Drop for RecordingWriter {
    fn drop(&mut self) {
        if let Some(sink) = self.sink.take() {
            let _ = sink.finish();
        }
    }
}

impl RecordingWriter {
    fn file_extension(compress: bool) -> &'static str {
        if compress { "jsonl.gz" } else { "jsonl" }
    }

    fn create_sink(path: &Path, compress: bool) -> std::io::Result<RecordSink> {
        let file = File::create(path)?;
        let writer = BufWriter::new(file);
        if compress {
            Ok(RecordSink::Gz(GzEncoder::new(writer, Compression::default())))
        } else {
            Ok(RecordSink::Plain(writer))
        }
    }

    /// Creates a new recording session for `target_label` in `dir` with custom size limit and compression option.
    pub fn create_in_full(
        target_label: &str,
        dir: &Path,
        max_bytes: usize,
        compress: bool,
    ) -> std::io::Result<Self> {
        create_dir_all(dir)?;
        let clean_target = sanitize_target_name(target_label);
        let start_secs = epoch_secs_now();
        let start_slug = format_epoch_slug(start_secs);
        let ext = Self::file_extension(compress);
        let filename = format!("rec-{clean_target}-{start_slug}.{ext}");
        let path = dir.join(filename);

        let sink = Self::create_sink(&path, compress)?;
        Ok(Self {
            target: clean_target,
            dir: dir.to_path_buf(),
            max_bytes,
            compress,
            current_file_start_secs: start_secs,
            is_custom_path: false,
            path,
            sink: Some(sink),
            frame_count: 0,
            bytes_written: 0,
            total_session_frames: 0,
            total_session_bytes: 0,
            rotation_count: 0,
        })
    }

    /// Creates a new recording session for `target_label` in `dir` with a custom size limit.
    pub fn create_in_with_limit(target_label: &str, dir: &Path, max_bytes: usize) -> std::io::Result<Self> {
        Self::create_in_full(target_label, dir, max_bytes, false)
    }

    /// Creates a new recording session for `target_label` in the given directory (default 100 MB limit).
    pub fn create_in(target_label: &str, dir: &Path) -> std::io::Result<Self> {
        Self::create_in_with_limit(target_label, dir, DEFAULT_MAX_RECORD_BYTES)
    }

    /// Creates a new recording session with default 100 MB limit in recordings directory.
    pub fn new(target_label: &str) -> std::io::Result<Self> {
        Self::new_with_limit(target_label, DEFAULT_MAX_RECORD_BYTES)
    }

    /// Creates a new recording session for `target_label` with `max_bytes` in recordings directory.
    pub fn new_with_limit(target_label: &str, max_bytes: usize) -> std::io::Result<Self> {
        Self::new_full(target_label, max_bytes, false)
    }

    /// Creates a new recording session for `target_label` with `max_bytes` and optional compression in recordings directory.
    pub fn new_full(target_label: &str, max_bytes: usize, compress: bool) -> std::io::Result<Self> {
        let dir = recordings_dir().unwrap_or_else(|| std::env::temp_dir().join("pg_lens").join("recordings"));
        match Self::create_in_full(target_label, &dir, max_bytes, compress) {
            Ok(writer) => Ok(writer),
            Err(e) if is_permission_denied(&e) => {
                let fallback = std::env::temp_dir().join("pg_lens").join("recordings");
                Self::create_in_full(target_label, &fallback, max_bytes, compress)
            }
            Err(e) => Err(e),
        }
    }

    /// Creates a recording writer targeting a custom path (useful in tests or CLI flags).
    pub fn create_at(path: PathBuf) -> std::io::Result<Self> {
        if let Some(parent) = path.parent() {
            create_dir_all(parent)?;
        }
        let dir = path.parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("."));
        let compress = path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.ends_with(".gz"));
        let sink = Self::create_sink(&path, compress)?;
        Ok(Self {
            target: "custom".to_string(),
            dir,
            max_bytes: 0, // unlimited for explicit file path
            compress,
            current_file_start_secs: epoch_secs_now(),
            is_custom_path: true,
            path,
            sink: Some(sink),
            frame_count: 0,
            bytes_written: 0,
            total_session_frames: 0,
            total_session_bytes: 0,
            rotation_count: 0,
        })
    }

    /// Appends one [`DbSnapshot`] as a JSON line, rotating to a new file if `max_bytes` is reached.
    ///
    /// Returns `Ok(Some(new_path))` if a rotation occurred, or `Ok(None)` if written to current file.
    pub fn append(&mut self, snapshot: &DbSnapshot) -> std::io::Result<Option<PathBuf>> {
        let line = serde_json::to_string(snapshot).map_err(|e| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())
        })?;

        let mut rotated_path = None;
        if self.max_bytes > 0 && self.bytes_written > 0 && self.bytes_written + line.len() + 1 > self.max_bytes {
            let new_path = self.rotate()?;
            rotated_path = Some(new_path);
        }

        if let Some(ref mut sink) = self.sink {
            sink.write_all(line.as_bytes())?;
            sink.write_all(b"\n")?;
            sink.flush()?;
        }

        self.frame_count += 1;
        self.total_session_frames += 1;
        self.bytes_written += line.len() + 1;
        self.total_session_bytes += line.len() + 1;
        Ok(rotated_path)
    }

    /// Rotates the current recording to a new file, stamping end timestamp on the finished file.
    pub fn rotate(&mut self) -> std::io::Result<PathBuf> {
        if let Some(sink) = self.sink.take() {
            sink.finish()?;
        }
        let now_secs = epoch_secs_now();

        // Stamp end timestamp on the completed file: rec-<target>-<start>-to-<end>.jsonl[.gz]
        let start_slug = format_epoch_slug(self.current_file_start_secs);
        let end_slug = format_epoch_slug(now_secs);
        let ext = Self::file_extension(self.compress);
        let closed_filename = format!("rec-{}-{}-to-{}.{ext}", self.target, start_slug, end_slug);
        let closed_path = self.dir.join(closed_filename);
        let _ = std::fs::rename(&self.path, &closed_path);

        // Open new file for continuation
        self.current_file_start_secs = now_secs;
        let new_start_slug = format_epoch_slug(now_secs);
        let new_filename = format!("rec-{}-{}.{ext}", self.target, new_start_slug);
        let new_path = self.dir.join(new_filename);

        self.sink = Some(Self::create_sink(&new_path, self.compress)?);
        self.path = new_path.clone();
        self.frame_count = 0;
        self.bytes_written = 0;
        self.rotation_count += 1;

        Ok(new_path)
    }

    /// Path to the recording file currently being written.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Frames written to the current file.
    pub fn frame_count(&self) -> usize {
        self.frame_count
    }

    /// Total frames across all rotated files in this recording session.
    pub fn total_session_frames(&self) -> usize {
        self.total_session_frames
    }

    /// Bytes written to the current file.
    pub fn bytes_written(&self) -> usize {
        self.bytes_written
    }

    /// Total bytes written across all files in this session.
    pub fn total_session_bytes(&self) -> usize {
        self.total_session_bytes
    }

    /// Number of times this recording has rotated so far.
    pub fn rotation_count(&self) -> usize {
        self.rotation_count
    }

    /// Finalizes the recording session, flushes and stamps the end timestamp in the title.
    pub fn finish(mut self) -> std::io::Result<(PathBuf, usize, usize)> {
        if let Some(sink) = self.sink.take() {
            sink.finish()?;
        }
        let now_secs = epoch_secs_now();
        let start_slug = format_epoch_slug(self.current_file_start_secs);
        let end_slug = format_epoch_slug(now_secs);
        let ext = Self::file_extension(self.compress);

        if !self.is_custom_path && now_secs >= self.current_file_start_secs {
            let final_filename = format!("rec-{}-{}-to-{}.{ext}", self.target, start_slug, end_slug);
            let final_path = self.dir.join(final_filename);
            if std::fs::rename(&self.path, &final_path).is_ok() {
                self.path = final_path;
            }
        }

        Ok((self.path.clone(), self.total_session_frames, self.total_session_bytes))
    }
}

/// Reads recordings (.jsonl, .jsonl.gz) or single snapshots (.json, .json.gz) back into memory.
pub struct RecordingReader;

impl RecordingReader {
    /// Loads all frames from `path`.
    ///
    /// Transparently supports uncompressed and gzip-compressed files
    /// (`.jsonl`, `.jsonl.gz`, `.json`, `.json.gz`).
    pub fn load(path: &Path) -> std::io::Result<Vec<DbSnapshot>> {
        let mut file = File::open(path)?;
        let mut header = [0u8; 2];
        let n = file.read(&mut header).unwrap_or(0);
        file.seek(SeekFrom::Start(0))?;
        let is_gzip = n == 2 && header[0] == 0x1f && header[1] == 0x8b;

        let filename = path.file_name().unwrap_or_default().to_string_lossy();
        let is_single_json = filename.ends_with(".json") || filename.ends_with(".json.gz");

        let reader: Box<dyn Read> = if is_gzip {
            Box::new(GzDecoder::new(file))
        } else {
            Box::new(file)
        };
        let buf_reader = BufReader::new(reader);

        if is_single_json {
            let snapshot: DbSnapshot = serde_json::from_reader(buf_reader).map_err(|e| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("failed to parse snapshot JSON: {e}"),
                )
            })?;
            return Ok(vec![snapshot]);
        }

        // Otherwise parse line-delimited JSON
        let mut frames = Vec::new();
        for (idx, line) in buf_reader.lines().enumerate() {
            let line = line?;
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let snapshot: DbSnapshot = serde_json::from_str(trimmed).map_err(|e| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("failed to parse line {}: {e}", idx + 1),
                )
            })?;
            frames.push(snapshot);
        }

        if frames.is_empty() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "recording file contains 0 valid snapshot frames",
            ));
        }

        Ok(frames)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn roundtrip_recording_writer_and_reader() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("test_rec.jsonl");

        let mut writer = RecordingWriter::create_at(path.clone()).expect("writer create");
        let snap1 = DbSnapshot::mock();
        let snap2 = DbSnapshot::mock();

        writer.append(&snap1).expect("append 1");
        writer.append(&snap2).expect("append 2");
        let (saved_path, count, bytes) = writer.finish().expect("finish");

        assert_eq!(saved_path, path);
        assert_eq!(count, 2);
        assert!(bytes > 0);

        let frames = RecordingReader::load(&path).expect("load");
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].vitals.server_version, snap1.vitals.server_version);
        assert_eq!(frames[1].vitals.server_version, snap2.vitals.server_version);
    }

    #[test]
    fn single_json_snapshot_load() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("snapshot.json");

        let snap = DbSnapshot::mock();
        let json = serde_json::to_string_pretty(&snap).expect("serialize");
        std::fs::write(&path, json).expect("write");

        let frames = RecordingReader::load(&path).expect("load");
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].vitals.server_version, snap.vitals.server_version);
    }

    #[test]
    fn sanitize_target_name_strips_unsafe_characters() {
        assert_eq!(sanitize_target_name("prod:5432/shop?ssl=true"), "prod_5432_shop_ssl_true");
        assert_eq!(sanitize_target_name("simple-db_1"), "simple-db_1");
        assert_eq!(sanitize_target_name(""), "postgres");
    }

    #[test]
    fn parse_recording_filenames_various_formats() {
        let (target, kind, start, end) =
            parse_recording_filename("rec-shop-20260907_143000-to-20260907_151500.jsonl");
        assert_eq!(target, "shop");
        assert_eq!(kind, RecordingKind::Recording);
        assert!(start.is_some());
        assert!(end.is_some());
        assert!(end.unwrap() > start.unwrap());

        let (target2, kind2, start2, end2) =
            parse_recording_filename("rec-analytics_db-20260907_143000.jsonl");
        assert_eq!(target2, "analytics_db");
        assert_eq!(kind2, RecordingKind::Recording);
        assert!(start2.is_some());
        assert_eq!(end2, None);

        let (target3, kind3, start3, end3) =
            parse_recording_filename("snapshot-shop-20260907_143000.json");
        assert_eq!(target3, "shop");
        assert_eq!(kind3, RecordingKind::Bookmark);
        assert!(start3.is_some());
        assert_eq!(start3, end3);
    }

    #[test]
    fn rotation_by_size_limit() {
        let dir = tempdir().expect("tempdir");
        // Limit to 500 bytes to force immediate rotation on append
        let mut writer =
            RecordingWriter::create_in_with_limit("shop", dir.path(), 500).expect("writer create");

        let snap = DbSnapshot::mock();
        let rot1 = writer.append(&snap).expect("append 1");
        assert_eq!(rot1, None); // First frame fits

        // Second frame exceeds 500 bytes and triggers rotation
        let rot2 = writer.append(&snap).expect("append 2");
        assert!(rot2.is_some());
        let new_path = rot2.unwrap();
        assert!(new_path.exists());
        assert_eq!(writer.rotation_count(), 1);
        assert_eq!(writer.total_session_frames(), 2);

        let (final_path, frames, bytes) = writer.finish().expect("finish");
        assert!(final_path.exists());
        assert_eq!(frames, 2);
        assert!(bytes > 0);
    }

    #[test]
    fn list_and_delete_recordings() {
        let dir = tempdir().expect("tempdir");
        let rec_dir = dir.path().join("recordings");
        let exp_dir = dir.path().join("exports");
        std::fs::create_dir_all(&rec_dir).expect("create rec_dir");
        std::fs::create_dir_all(&exp_dir).expect("create exp_dir");

        let f1 = rec_dir.join("rec-shop-20260907_143000-to-20260907_151500.jsonl");
        let f2 = exp_dir.join("snapshot-shop-20260907_143000.json");
        std::fs::write(&f1, "{\"vitals\":{}}\n").expect("write f1");
        std::fs::write(&f2, "{\"vitals\":{}}\n").expect("write f2");

        let list = list_recordings(&rec_dir, &exp_dir, Some(&f1));
        assert_eq!(list.len(), 2);
        let rec_entry = list.iter().find(|e| e.kind == RecordingKind::Recording).expect("rec");
        assert!(rec_entry.is_active);
        assert_eq!(rec_entry.target, "shop");

        let bookmark_entry = list.iter().find(|e| e.kind == RecordingKind::Bookmark).expect("bookmark");
        assert_eq!(bookmark_entry.frame_count, Some(1));

        delete_recording(&f2).expect("delete");
        assert!(!f2.exists());
    }

    #[test]
    fn compressed_recording_roundtrip() {
        let dir = tempdir().expect("tempdir");
        let mut writer = RecordingWriter::create_in_full("shop", dir.path(), DEFAULT_MAX_RECORD_BYTES, true)
            .expect("writer create");

        let snap = DbSnapshot::mock();
        writer.append(&snap).expect("append snap");
        let (path, frames, bytes) = writer.finish().expect("finish");
        assert!(path.exists());
        assert!(path.to_string_lossy().ends_with(".jsonl.gz"));
        assert_eq!(frames, 1);
        assert!(bytes > 0);

        // Verify transparent decompression
        let loaded = RecordingReader::load(&path).expect("load compressed");
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].vitals.server_version, snap.vitals.server_version);
    }

    #[test]
    fn parse_recording_filenames_compressed() {
        let (target, kind, start, end) =
            parse_recording_filename("rec-shop-20260907_143000-to-20260907_151500.jsonl.gz");
        assert_eq!(target, "shop");
        assert_eq!(kind, RecordingKind::Recording);
        assert!(start.is_some());
        assert!(end.is_some());

        let (target2, kind2, start2, end2) =
            parse_recording_filename("snapshot-shop-20260907_143000.json.gz");
        assert_eq!(target2, "shop");
        assert_eq!(kind2, RecordingKind::Bookmark);
        assert!(start2.is_some());
        assert_eq!(start2, end2);
    }

    #[test]
    fn prune_recordings_retention_and_quota() {
        let dir = tempdir().expect("tempdir");
        let rec_dir = dir.path().join("recordings");
        let exp_dir = dir.path().join("exports");
        std::fs::create_dir_all(&rec_dir).expect("create rec_dir");
        std::fs::create_dir_all(&exp_dir).expect("create exp_dir");

        // Create 3 files:
        // f_old: 2020-01-01 (very old, should be pruned by retention)
        // f_mid: recent file (1000 bytes)
        // f_active: active recording file (must never be pruned)
        let f_old = rec_dir.join("rec-shop-20200101_100000-to-20200101_110000.jsonl");
        let f_mid = rec_dir.join("rec-shop-20260907_100000-to-20260907_110000.jsonl");
        let f_active = rec_dir.join("rec-shop-20260907_120000.jsonl");

        std::fs::write(&f_old, "{\"vitals\":{}}\n").expect("write f_old");
        std::fs::write(&f_mid, "{\"vitals\":{}}\n").expect("write f_mid");
        std::fs::write(&f_active, "{\"vitals\":{}}\n").expect("write f_active");

        // 1. Test pruning by retention days (e.g. 30 days)
        let res_age = prune_recordings(&rec_dir, &exp_dir, None, Some(30), Some(&f_active))
            .expect("prune by age");
        assert_eq!(res_age.files_deleted, 1);
        assert!(!f_old.exists());
        assert!(f_mid.exists());
        assert!(f_active.exists());

        // 2. Test pruning by max_total_bytes quota (e.g. 1 byte limit to force pruning non-active)
        let res_quota = prune_recordings(&rec_dir, &exp_dir, Some(1), None, Some(&f_active))
            .expect("prune by quota");
        assert_eq!(res_quota.files_deleted, 1);
        assert!(!f_mid.exists());
        assert!(f_active.exists()); // Active file was preserved!
    }
}
