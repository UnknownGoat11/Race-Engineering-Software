//! Retention hooks. Nothing is ever deleted by default: the application decides whether to run a policy
//! ([`crate::RecordingStore::apply_retention`]), and [`KeepAll`] (the default policy) selects nothing.
//!
//! A policy only sees *eligible* sessions: finished (COMPLETE or INTERRUPTED), not being recorded, and with
//! retention class DEFAULT (a session marked KEEP is never offered to a policy).

use std::time::Duration;

use crate::index::SessionInfo;
use crate::types::SessionId;

/// Chooses sessions to delete.
pub trait RetentionPolicy {
    /// Returns the ids to delete, chosen from `candidates` (every eligible session, oldest first).
    /// Ids that are not in `candidates` are refused by the store.
    fn select(&self, candidates: &[SessionInfo], now_unix_ns: i64) -> Vec<SessionId>;
}

/// The default policy: keeps every recording.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct KeepAll;

impl RetentionPolicy for KeepAll {
    fn select(&self, _candidates: &[SessionInfo], _now_unix_ns: i64) -> Vec<SessionId> {
        Vec::new()
    }
}

/// Opt-in policy: selects sessions that ended (or, if interrupted, were recovered; else started) more than
/// `max_age` ago.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MaxAge {
    /// Sessions older than this (strictly) are selected.
    pub max_age: Duration,
}

impl RetentionPolicy for MaxAge {
    fn select(&self, candidates: &[SessionInfo], now_unix_ns: i64) -> Vec<SessionId> {
        let max_age_ns = i128::try_from(self.max_age.as_nanos()).unwrap_or(i128::MAX);
        candidates
            .iter()
            .filter(|s| {
                let reference = s
                    .ended_unix_ns
                    .or(s.recovered_unix_ns)
                    .unwrap_or(s.started_unix_ns);
                i128::from(now_unix_ns) - i128::from(reference) > max_age_ns
            })
            .map(|s| s.id)
            .collect()
    }
}

/// What [`crate::RecordingStore::apply_retention`] did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RetentionReport {
    /// Sessions deleted (files and index rows).
    pub deleted: Vec<SessionId>,
    /// Ids the policy returned that were not eligible; nothing was done to them.
    pub refused: Vec<SessionId>,
    /// Deletions that failed; those sessions are left as they are.
    pub failed: Vec<RetentionFailure>,
}

/// A deletion that failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetentionFailure {
    /// The session that could not be deleted.
    pub session_id: SessionId,
    /// Why.
    pub error: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{IntegrityStatus, RetentionClass, SessionMeta, SessionStatus};

    const NS_PER_S: i64 = 1_000_000_000;

    fn info(started_s: i64, ended_s: Option<i64>, recovered_s: Option<i64>) -> SessionInfo {
        SessionInfo {
            id: SessionId::new_random(),
            dir: String::new(),
            status: SessionStatus::Complete,
            started_unix_ns: started_s * NS_PER_S,
            ended_unix_ns: ended_s.map(|s| s * NS_PER_S),
            recovered_unix_ns: recovered_s.map(|s| s * NS_PER_S),
            first_capture_ns: None,
            last_capture_ns: None,
            format_version: 1,
            meta: SessionMeta::default(),
            record_count: 0,
            dropped_records: 0,
            integrity: IntegrityStatus::Ok,
            integrity_detail: None,
            checksum_crc32c: None,
            retention: RetentionClass::Default,
        }
    }

    #[test]
    fn keep_all_selects_nothing() {
        let c = vec![info(0, Some(1), None)];
        assert!(KeepAll.select(&c, i64::MAX).is_empty());
    }

    #[test]
    fn max_age_uses_end_then_recovery_then_start() {
        let ended_old = info(0, Some(10), None);
        let ended_recent = info(0, Some(95), None);
        let recovered_old = info(0, None, Some(20));
        let started_only_old = info(5, None, None);
        let c = vec![
            ended_old.clone(),
            ended_recent,
            recovered_old.clone(),
            started_only_old.clone(),
        ];
        let p = MaxAge {
            max_age: Duration::from_secs(10),
        };
        assert_eq!(
            p.select(&c, 100 * NS_PER_S),
            vec![ended_old.id, recovered_old.id, started_only_old.id]
        );
        let p = MaxAge {
            max_age: Duration::MAX,
        };
        assert!(
            p.select(&c, i64::MAX).is_empty(),
            "no overflow with extreme ages"
        );
    }
}
