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
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

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

/// Formats current wall-clock time as a compact timestamp.
pub fn timestamp_slug() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    format!("{now}")
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

/// Sequential JSONL recording session.
#[derive(Debug)]
pub struct RecordingWriter {
    path: PathBuf,
    writer: BufWriter<File>,
    frame_count: usize,
    bytes_written: usize,
}

impl RecordingWriter {
    /// Creates a new recording session for `target_label` in the given directory.
    pub fn create_in(target_label: &str, dir: &Path) -> std::io::Result<Self> {
        create_dir_all(dir)?;
        let clean_target = sanitize_target_name(target_label);
        let ts = timestamp_slug();
        let filename = format!("rec-{clean_target}-{ts}.jsonl");
        let path = dir.join(filename);

        let file = File::create(&path)?;
        Ok(Self {
            path,
            writer: BufWriter::new(file),
            frame_count: 0,
            bytes_written: 0,
        })
    }

    /// Creates a new recording session for `target_label` in the recordings directory.
    pub fn new(target_label: &str) -> std::io::Result<Self> {
        let dir = recordings_dir().unwrap_or_else(|| std::env::temp_dir().join("pg_lens").join("recordings"));
        match Self::create_in(target_label, &dir) {
            Ok(writer) => Ok(writer),
            Err(e) if is_permission_denied(&e) => {
                let fallback = std::env::temp_dir().join("pg_lens").join("recordings");
                Self::create_in(target_label, &fallback)
            }
            Err(e) => Err(e),
        }
    }

    /// Creates a recording writer targeting a custom path (useful in tests or CLI flags).
    pub fn create_at(path: PathBuf) -> std::io::Result<Self> {
        if let Some(parent) = path.parent() {
            create_dir_all(parent)?;
        }
        let file = File::create(&path)?;
        Ok(Self {
            path,
            writer: BufWriter::new(file),
            frame_count: 0,
            bytes_written: 0,
        })
    }

    /// Appends one [`DbSnapshot`] as a JSON line.
    pub fn append(&mut self, snapshot: &DbSnapshot) -> std::io::Result<()> {
        let line = serde_json::to_string(snapshot).map_err(|e| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())
        })?;
        self.writer.write_all(line.as_bytes())?;
        self.writer.write_all(b"\n")?;
        self.writer.flush()?;

        self.frame_count += 1;
        self.bytes_written += line.len() + 1;
        Ok(())
    }

    /// Path to the recording file being written.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Total frames written so far.
    pub fn frame_count(&self) -> usize {
        self.frame_count
    }

    /// Total uncompressed bytes written so far.
    pub fn bytes_written(&self) -> usize {
        self.bytes_written
    }

    /// Finalizes the recording session and flushes the file.
    pub fn finish(mut self) -> std::io::Result<(PathBuf, usize, usize)> {
        self.writer.flush()?;
        Ok((self.path, self.frame_count, self.bytes_written))
    }
}

/// Reads recordings (.jsonl) or single snapshots (.json) back into memory.
pub struct RecordingReader;

impl RecordingReader {
    /// Loads all frames from `path`.
    ///
    /// Supports both `.jsonl` files (multi-frame recording) and `.json` files
    /// (single snapshot export).
    pub fn load(path: &Path) -> std::io::Result<Vec<DbSnapshot>> {
        let file = File::open(path)?;
        let reader = BufReader::new(file);

        // Check if file extension is .json
        let is_single_json = path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("json"));

        if is_single_json {
            let snapshot: DbSnapshot = serde_json::from_reader(reader).map_err(|e| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("failed to parse snapshot JSON: {e}"),
                )
            })?;
            return Ok(vec![snapshot]);
        }

        // Otherwise parse line-delimited JSON
        let mut frames = Vec::new();
        for (idx, line) in reader.lines().enumerate() {
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
}
