//! pg_lens_core — data models and the PostgreSQL poller shared by every
//! pg_lens frontend (TUI, future web).
//!
//! This crate must stay frontend-agnostic: no terminal/TUI dependencies and
//! no knowledge of any frontend's internal message types.

pub mod blocking;
pub mod blocks;
pub mod db;
pub mod history;
pub mod history_store;
pub mod idle_sessions;
pub mod index_advisor;
pub mod lock_capacity;
pub mod models;
pub mod poller;
pub mod prepared_xacts;
pub mod queries;
pub mod recording;
pub mod remote_config;
pub mod schema_growth;
pub mod services;
pub mod settings;
pub mod waits;
pub mod xact_age;

// Re-exported so frontends can name `tokio_postgres::Config` (the type
// `settings::resolve` returns) without pinning their own copy of the crate.
pub use tokio_postgres;

pub use blocking::{BlockingChain, blocking_chain};
pub use blocks::{BlockTreeNode, build_blocking_tree};
pub use history::{
    HistoryPoint, SnapshotHistory, TREND_DEADBAND, TREND_LOOKBACK_TICKS, Trend, trend,
};
pub use index_advisor::{IndexCatalogRow, classify as classify_indexes};
pub use models::{
    ActiveLockRow, ActivityRow, AdminActionResult, AdminCommand, AdminKind, AdminOutcome, BloatRow,
    CheckpointerStats, DatabaseConflicts, DatabaseRow, DbSnapshot, DdlProgressRow, IdleSessionRow,
    IndexFinding, IndexRow, IoStatRow, LockCapacity, LockRow, PollerStatus, PreparedXactRow,
    ProgressUnifiedRow, PublicationRow, ReplicationInfo, ReplicationSlotRow, SchemaSnapshot,
    SchemaStatus, SequenceRow, SequenceSeverity, ServerVitals, SlruRow, SlruStats, StatementRow,
    StatementsSnapshot, StatementsStatus, SubscriptionRow, TableDetail, TableDetailColumn,
    TableDetailConstraint, TableDetailIndex, TableDetailRequest, TableStatRow, VacuumClusterAge,
    VacuumProgressRow, VacuumTableRow, WalReceiverRow, WalSenderRow, WalStats,
    calculate_sequence_exhaustion,
};
pub use idle_sessions::{
    OldestIdleSession, Severity as IdleSessionSeverity, oldest_idle_session,
    severity as idle_session_severity,
};
pub use lock_capacity::{Severity as LockCapacitySeverity, severity as lock_capacity_severity};
pub use prepared_xacts::{
    OldestPreparedXact, Severity as PreparedXactSeverity, oldest_prepared_xact,
    severity as prepared_xact_severity,
};
pub use recording::{
    RecordingReader, RecordingWriter, export_snapshot, export_snapshot_to, exports_dir,
    recordings_dir, sanitize_target_name,
};
pub use services::PasswordSource;
pub use waits::{WaitSummary, top_waits};
pub use settings::{ConnLabel, ConnSpec, Resolved, SettingsError};
pub use xact_age::{OldestXact, Severity as XactAgeSeverity, oldest_open_xact, xact_age_severity};
