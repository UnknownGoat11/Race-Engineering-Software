//! In-memory store (tests and `RELAY_ENV=development` only; everything is lost on exit).
//! Cloning shares the same data, which lets tests simulate a relay restart over one store.

use std::collections::HashMap;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use uuid::Uuid;

use super::{
    Account, AccountStore, EngineerSlotRecord, NewRefreshToken, RefreshTokenRecord, RotateOutcome,
    SessionRecord, SessionStatus, SessionStore, StoreError,
};
use crate::secrets::TokenHash;

#[derive(Debug, Default)]
struct Family {
    account: Uuid,
    revoked_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
struct StoredToken {
    record: RefreshTokenRecord,
    hash: TokenHash,
}

#[derive(Debug, Default)]
struct Inner {
    accounts: HashMap<Uuid, Account>,
    by_username: HashMap<String, Uuid>,
    last_login: HashMap<Uuid, DateTime<Utc>>,
    families: HashMap<Uuid, Family>,
    tokens: HashMap<Uuid, StoredToken>,
    token_by_hash: HashMap<TokenHash, Uuid>,
    sessions: HashMap<u64, SessionRecord>,
    /// Engineer slots by (session id, account).
    slots: HashMap<(u64, Uuid), EngineerSlotRecord>,
}

/// In-memory implementation of [`AccountStore`] and [`SessionStore`].
#[derive(Debug, Clone, Default)]
pub struct MemoryStore {
    inner: Arc<Mutex<Inner>>,
}

impl MemoryStore {
    /// An empty store.
    pub fn new() -> Self {
        Self::default()
    }
}

impl Inner {
    fn insert_token(&mut self, token: &NewRefreshToken) -> Result<(), StoreError> {
        if self.token_by_hash.contains_key(&token.token_hash) {
            return Err(StoreError::Conflict("refresh token"));
        }
        if !self.families.contains_key(&token.family_id) {
            return Err(StoreError::Corrupt(format!(
                "refresh token family {} does not exist",
                token.family_id
            )));
        }
        self.token_by_hash.insert(token.token_hash, token.id);
        self.tokens.insert(
            token.id,
            StoredToken {
                record: RefreshTokenRecord {
                    id: token.id,
                    family_id: token.family_id,
                    account_id: token.account_id,
                    created_at: token.created_at,
                    expires_at: token.expires_at,
                    rotated_at: None,
                    revoked_at: None,
                },
                hash: token.token_hash,
            },
        );
        Ok(())
    }

    fn enforce_family_cap(&mut self, family: Uuid, keep: usize, at: DateTime<Utc>) {
        let mut active: Vec<(DateTime<Utc>, Uuid)> = self
            .tokens
            .values()
            .filter(|t| {
                t.record.family_id == family
                    && t.record.revoked_at.is_none()
                    && t.record.rotated_at.is_none()
                    && t.record.expires_at > at
            })
            .map(|t| (t.record.created_at, t.record.id))
            .collect();
        // Newest first; ties broken by id for determinism.
        active.sort_by(|a, b| b.cmp(a));
        for (_, id) in active.into_iter().skip(keep) {
            if let Some(t) = self.tokens.get_mut(&id) {
                t.record.revoked_at = Some(at);
            }
        }
    }
}

impl AccountStore for MemoryStore {
    async fn create_account(&self, account: &Account) -> Result<(), StoreError> {
        let mut inner = self.inner.lock();
        if inner.by_username.contains_key(&account.username) {
            return Err(StoreError::Conflict("username"));
        }
        if inner.accounts.contains_key(&account.id) {
            return Err(StoreError::Conflict("account id"));
        }
        inner
            .by_username
            .insert(account.username.clone(), account.id);
        inner.accounts.insert(account.id, account.clone());
        Ok(())
    }

    async fn account_by_username(&self, username: &str) -> Result<Option<Account>, StoreError> {
        let inner = self.inner.lock();
        Ok(inner
            .by_username
            .get(username)
            .and_then(|id| inner.accounts.get(id))
            .cloned())
    }

    async fn account_by_id(&self, id: Uuid) -> Result<Option<Account>, StoreError> {
        Ok(self.inner.lock().accounts.get(&id).cloned())
    }

    async fn record_login(&self, id: Uuid, at: DateTime<Utc>) -> Result<(), StoreError> {
        self.inner.lock().last_login.insert(id, at);
        Ok(())
    }

    async fn create_token_family(
        &self,
        family: Uuid,
        account: Uuid,
        _at: DateTime<Utc>,
    ) -> Result<(), StoreError> {
        let mut inner = self.inner.lock();
        if inner.families.contains_key(&family) {
            return Err(StoreError::Conflict("token family"));
        }
        inner.families.insert(
            family,
            Family {
                account,
                revoked_at: None,
            },
        );
        Ok(())
    }

    async fn family_revoked(&self, family: Uuid) -> Result<Option<bool>, StoreError> {
        Ok(self
            .inner
            .lock()
            .families
            .get(&family)
            .map(|f| f.revoked_at.is_some()))
    }

    async fn inactive_families(&self, families: &[Uuid]) -> Result<Vec<Uuid>, StoreError> {
        let inner = self.inner.lock();
        let active = |id: &Uuid| {
            inner.families.get(id).is_some_and(|f| {
                f.revoked_at.is_none()
                    && inner.accounts.get(&f.account).is_some_and(|a| !a.disabled)
            })
        };
        Ok(families.iter().filter(|id| !active(id)).copied().collect())
    }

    async fn insert_refresh_token(
        &self,
        token: &NewRefreshToken,
        keep_active: usize,
    ) -> Result<(), StoreError> {
        let mut inner = self.inner.lock();
        inner.insert_token(token)?;
        inner.enforce_family_cap(token.family_id, keep_active, token.created_at);
        Ok(())
    }

    async fn refresh_token_by_hash(
        &self,
        hash: &TokenHash,
    ) -> Result<Option<RefreshTokenRecord>, StoreError> {
        let inner = self.inner.lock();
        Ok(inner
            .token_by_hash
            .get(hash)
            .and_then(|id| inner.tokens.get(id))
            .filter(|t| t.hash.ct_eq(hash))
            .map(|t| t.record.clone()))
    }

    async fn rotate_refresh_token(
        &self,
        old: Uuid,
        successor: &NewRefreshToken,
        at: DateTime<Utc>,
    ) -> Result<RotateOutcome, StoreError> {
        let mut inner = self.inner.lock();
        let Some(current) = inner.tokens.get(&old) else {
            return Ok(RotateOutcome::Lost);
        };
        if current.record.rotated_at.is_some() || current.record.revoked_at.is_some() {
            return Ok(RotateOutcome::Lost);
        }
        let family = current.record.family_id;
        // Insert first: a failed insert (duplicate digest, unknown family) changes nothing.
        inner.insert_token(successor)?;
        // `old` itself is rotated here too; every other live token of the family is superseded.
        for t in inner.tokens.values_mut() {
            if t.record.family_id == family
                && t.record.id != successor.id
                && t.record.rotated_at.is_none()
                && t.record.revoked_at.is_none()
            {
                t.record.rotated_at = Some(at);
            }
        }
        Ok(RotateOutcome::Rotated)
    }

    async fn revoke_family(&self, family: Uuid, at: DateTime<Utc>) -> Result<(), StoreError> {
        let mut inner = self.inner.lock();
        if let Some(f) = inner.families.get_mut(&family)
            && f.revoked_at.is_none()
        {
            f.revoked_at = Some(at);
        }
        for t in inner.tokens.values_mut() {
            if t.record.family_id == family && t.record.revoked_at.is_none() {
                t.record.revoked_at = Some(at);
            }
        }
        Ok(())
    }

    async fn revoke_account_tokens(
        &self,
        account: Uuid,
        at: DateTime<Utc>,
    ) -> Result<u64, StoreError> {
        let mut inner = self.inner.lock();
        for f in inner.families.values_mut() {
            if f.account == account && f.revoked_at.is_none() {
                f.revoked_at = Some(at);
            }
        }
        let mut revoked = 0u64;
        for t in inner.tokens.values_mut() {
            if t.record.account_id == account && t.record.revoked_at.is_none() {
                t.record.revoked_at = Some(at);
                revoked += 1;
            }
        }
        Ok(revoked)
    }

    async fn purge_refresh_tokens(&self, before: DateTime<Utc>) -> Result<u64, StoreError> {
        let mut inner = self.inner.lock();
        let expired: Vec<(Uuid, TokenHash)> = inner
            .tokens
            .values()
            .filter(|t| t.record.expires_at < before)
            .map(|t| (t.record.id, t.hash))
            .collect();
        for (id, hash) in &expired {
            inner.tokens.remove(id);
            inner.token_by_hash.remove(hash);
        }
        Ok(expired.len() as u64)
    }
}

impl SessionStore for MemoryStore {
    async fn insert_session(&self, record: &SessionRecord) -> Result<(), StoreError> {
        let mut inner = self.inner.lock();
        if inner.sessions.contains_key(&record.id) {
            return Err(StoreError::Conflict("session id"));
        }
        inner.sessions.insert(record.id, record.clone());
        Ok(())
    }

    async fn session(&self, id: u64) -> Result<Option<SessionRecord>, StoreError> {
        Ok(self.inner.lock().sessions.get(&id).cloned())
    }

    async fn set_session_status(
        &self,
        id: u64,
        status: SessionStatus,
        at: DateTime<Utc>,
    ) -> Result<(), StoreError> {
        if let Some(s) = self.inner.lock().sessions.get_mut(&id)
            && s.status != SessionStatus::Closed
        {
            s.status = status;
            s.last_activity = at;
        }
        Ok(())
    }

    async fn close_session(
        &self,
        id: u64,
        reason: &str,
        at: DateTime<Utc>,
    ) -> Result<(), StoreError> {
        let mut inner = self.inner.lock();
        if let Some(s) = inner.sessions.get_mut(&id)
            && s.status != SessionStatus::Closed
        {
            s.status = SessionStatus::Closed;
            s.close_reason = Some(reason.to_owned());
            s.resume_hash = None;
            s.last_activity = at;
            s.ended_at = Some(at);
        }
        inner.slots.retain(|(session, _), _| *session != id);
        Ok(())
    }

    async fn touch_sessions(&self, ids: &[u64], at: DateTime<Utc>) -> Result<(), StoreError> {
        let mut inner = self.inner.lock();
        for id in ids {
            if let Some(s) = inner.sessions.get_mut(id)
                && s.status != SessionStatus::Closed
            {
                s.last_activity = at;
            }
        }
        Ok(())
    }

    async fn expire_sessions(
        &self,
        idle_before: DateTime<Utc>,
        at: DateTime<Utc>,
    ) -> Result<u64, StoreError> {
        let mut inner = self.inner.lock();
        let mut closed = 0u64;
        for s in inner.sessions.values_mut() {
            if s.status != SessionStatus::Closed && s.last_activity < idle_before {
                s.status = SessionStatus::Closed;
                s.close_reason = Some(super::CLOSE_REASON_EXPIRED.to_owned());
                s.resume_hash = None;
                s.ended_at = Some(at);
                closed += 1;
            }
        }
        let Inner {
            sessions, slots, ..
        } = &mut *inner;
        slots.retain(|(session, _), _| {
            sessions
                .get(session)
                .is_some_and(|s| s.status != SessionStatus::Closed)
        });
        Ok(closed)
    }

    async fn purge_closed_sessions(&self, ended_before: DateTime<Utc>) -> Result<u64, StoreError> {
        let mut inner = self.inner.lock();
        let before = inner.sessions.len();
        inner.sessions.retain(|_, s| {
            s.status != SessionStatus::Closed
                || s.ended_at.unwrap_or(s.last_activity) >= ended_before
        });
        let deleted = before - inner.sessions.len();
        let Inner {
            sessions, slots, ..
        } = &mut *inner;
        slots.retain(|(session, _), _| sessions.contains_key(session));
        Ok(u64::try_from(deleted).unwrap_or(u64::MAX))
    }

    async fn put_engineer_slot(&self, slot: &EngineerSlotRecord) -> Result<(), StoreError> {
        let mut inner = self.inner.lock();
        let open = inner
            .sessions
            .get(&slot.session_id)
            .is_some_and(|s| s.status != SessionStatus::Closed);
        if open {
            inner
                .slots
                .insert((slot.session_id, slot.account_id), slot.clone());
        }
        Ok(())
    }

    async fn delete_engineer_slot(
        &self,
        session_id: u64,
        account: Uuid,
        connection_id: u64,
    ) -> Result<(), StoreError> {
        let mut inner = self.inner.lock();
        if inner
            .slots
            .get(&(session_id, account))
            .is_some_and(|s| s.connection_id == connection_id)
        {
            inner.slots.remove(&(session_id, account));
        }
        Ok(())
    }

    async fn engineer_slots(&self, session_id: u64) -> Result<Vec<EngineerSlotRecord>, StoreError> {
        let inner = self.inner.lock();
        let mut slots: Vec<EngineerSlotRecord> = inner
            .slots
            .values()
            .filter(|s| s.session_id == session_id)
            .cloned()
            .collect();
        slots.sort_by_key(|s| s.connection_id);
        Ok(slots)
    }

    async fn ping(&self) -> Result<(), StoreError> {
        Ok(())
    }
}
