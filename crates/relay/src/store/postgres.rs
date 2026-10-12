//! PostgreSQL store (D-007). Runtime-checked `sqlx` queries only (no `query!` macros), so building
//! never needs a database; the schema lives in `crates/relay/migrations` and is embedded with
//! `sqlx::migrate!` (applied by `lmu-relay migrate` or at relay startup).

use std::str::FromStr as _;
use std::time::Duration;

use chrono::{DateTime, Utc};
use protocol::messages::Secret;
use sqlx::migrate::Migrator;
use sqlx::postgres::{PgConnectOptions, PgPool, PgPoolOptions, PgRow};
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

use super::{
    Account, AccountRole, AccountStore, CLOSE_REASON_EXPIRED, EngineerSlotRecord, NewRefreshToken,
    RefreshTokenRecord, RotateOutcome, SessionMeta, SessionRecord, SessionStatus, SessionStore,
    StoreError,
};
use crate::secrets::TokenHash;

/// Embedded migrations (`crates/relay/migrations`).
pub static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

/// How long a request waits for a pooled connection.
const ACQUIRE_TIMEOUT: Duration = Duration::from_secs(5);
/// Server-side limit of one statement (`statement_timeout`, set for every pooled connection), so
/// a lock wait or runaway query cannot hold a pooled connection indefinitely. The relay also
/// bounds every whole store call on its side ([`super::STORE_CALL_TIMEOUT`]).
const STATEMENT_TIMEOUT: &str = "5s";
/// SQLSTATE of a unique-constraint violation.
const UNIQUE_VIOLATION: &str = "23505";
/// Log component of this module.
const COMPONENT: &str = "relay.store";

/// PostgreSQL implementation of [`AccountStore`] and [`SessionStore`].
#[derive(Debug, Clone)]
pub struct PgStore {
    pool: PgPool,
}

impl PgStore {
    /// Connects a pool of at most `max_connections`; every connection runs with
    /// [`STATEMENT_TIMEOUT`]. The URL is never logged.
    pub async fn connect(url: &Secret, max_connections: u32) -> Result<Self, StoreError> {
        let options = PgConnectOptions::from_str(url.expose())?
            .options([("statement_timeout", STATEMENT_TIMEOUT)]);
        let pool = PgPoolOptions::new()
            .max_connections(max_connections)
            .acquire_timeout(ACQUIRE_TIMEOUT)
            .connect_with(options)
            .await?;
        Ok(Self { pool })
    }

    /// Applies pending migrations on one connection without the statement limit (a migration
    /// may legitimately take longer). That connection is closed afterwards, never returned to
    /// the pool without its limit.
    pub async fn migrate(&self) -> Result<(), StoreError> {
        let mut conn = self.pool.acquire().await?;
        sqlx::query("SET statement_timeout = 0")
            .execute(&mut *conn)
            .await?;
        let result = MIGRATOR.run(&mut *conn).await;
        if let Err(e) = conn.close().await {
            tracing::warn!(component = COMPONENT, error = %e, "migration connection not closed cleanly");
        }
        result?;
        Ok(())
    }

    /// The underlying pool.
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }
}

fn session_id_param(id: u64) -> Result<i64, StoreError> {
    i64::try_from(id).map_err(|_| StoreError::Corrupt(format!("session id {id} exceeds BIGINT")))
}

fn connection_id_param(id: u64) -> Result<i64, StoreError> {
    i64::try_from(id).map_err(|_| StoreError::Corrupt(format!("connection id {id} exceeds BIGINT")))
}

fn slot_from_row(row: &PgRow) -> Result<EngineerSlotRecord, StoreError> {
    let session_id: i64 = row.try_get("session_id")?;
    let connection_id: i64 = row.try_get("connection_id")?;
    Ok(EngineerSlotRecord {
        session_id: u64::try_from(session_id)
            .map_err(|_| StoreError::Corrupt(format!("session id {session_id}")))?,
        account_id: row.try_get("account_id")?,
        connection_id: u64::try_from(connection_id)
            .map_err(|_| StoreError::Corrupt(format!("connection id {connection_id}")))?,
        connected_since: row.try_get("connected_since")?,
        dropped_at: row.try_get("dropped_at")?,
    })
}

fn is_unique_violation(e: &sqlx::Error) -> bool {
    matches!(e, sqlx::Error::Database(db) if db.code().as_deref() == Some(UNIQUE_VIOLATION))
}

fn account_from_row(row: &PgRow) -> Result<Account, StoreError> {
    let role: String = row.try_get("role")?;
    Ok(Account {
        id: row.try_get("id")?,
        username: row.try_get("username")?,
        display_name: row.try_get("display_name")?,
        role: AccountRole::parse(&role)
            .ok_or_else(|| StoreError::Corrupt(format!("unknown account role {role:?}")))?,
        password_hash: row.try_get("password_hash")?,
        disabled: row.try_get("disabled")?,
        created_at: row.try_get("created_at")?,
    })
}

fn token_from_row(row: &PgRow) -> Result<RefreshTokenRecord, StoreError> {
    Ok(RefreshTokenRecord {
        id: row.try_get("id")?,
        family_id: row.try_get("family_id")?,
        account_id: row.try_get("account_id")?,
        created_at: row.try_get("created_at")?,
        expires_at: row.try_get("expires_at")?,
        rotated_at: row.try_get("rotated_at")?,
        revoked_at: row.try_get("revoked_at")?,
    })
}

fn session_from_row(row: &PgRow) -> Result<SessionRecord, StoreError> {
    let id: i64 = row.try_get("id")?;
    let status: String = row.try_get("status")?;
    let schema_version: i32 = row.try_get("schema_version")?;
    let resume: Option<Vec<u8>> = row.try_get("resume_hash")?;
    let resume_hash = match resume {
        Some(bytes) => Some(
            TokenHash::from_slice(&bytes)
                .ok_or_else(|| StoreError::Corrupt("resume_hash is not 32 bytes".into()))?,
        ),
        None => None,
    };
    Ok(SessionRecord {
        id: u64::try_from(id).map_err(|_| StoreError::Corrupt(format!("session id {id}")))?,
        driver_account: row.try_get("driver_account")?,
        resume_hash,
        meta: SessionMeta {
            car: row.try_get("car")?,
            vehicle_class: row.try_get("vehicle_class")?,
            track: row.try_get("track")?,
            session_type: row.try_get("session_type")?,
            lmu_version: row.try_get("lmu_version")?,
            app_version: row.try_get("app_version")?,
            schema_version: u16::try_from(schema_version)
                .map_err(|_| StoreError::Corrupt(format!("schema_version {schema_version}")))?,
        },
        started_at: row.try_get("started_at")?,
        status: SessionStatus::parse(&status)
            .ok_or_else(|| StoreError::Corrupt(format!("unknown session status {status:?}")))?,
        close_reason: row.try_get("close_reason")?,
        last_activity: row.try_get("last_activity")?,
        ended_at: row.try_get("ended_at")?,
    })
}

async fn insert_token(
    tx: &mut Transaction<'_, Postgres>,
    token: &NewRefreshToken,
) -> Result<(), StoreError> {
    sqlx::query(
        "INSERT INTO refresh_tokens (id, family_id, account_id, token_hash, created_at, expires_at) \
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(token.id)
    .bind(token.family_id)
    .bind(token.account_id)
    .bind(token.token_hash.as_bytes().as_slice())
    .bind(token.created_at)
    .bind(token.expires_at)
    .execute(&mut **tx)
    .await
    .map_err(|e| {
        if is_unique_violation(&e) {
            StoreError::Conflict("refresh token")
        } else {
            StoreError::Database(e)
        }
    })?;
    Ok(())
}

/// Row-locks a token family. Every transaction that writes a family's refresh tokens takes this
/// lock **before** touching any token row, so writers of one family are serialized and lock in
/// one order. Without it a rotation (token row, then the family row through the foreign-key check
/// of its INSERT) and a family-wide UPDATE (family row, then token rows) could deadlock.
async fn lock_family(tx: &mut Transaction<'_, Postgres>, family: Uuid) -> Result<(), StoreError> {
    sqlx::query("SELECT id FROM token_families WHERE id = $1 FOR UPDATE")
        .bind(family)
        .fetch_optional(&mut **tx)
        .await?;
    Ok(())
}

// Column lists as literal-producing macros: sqlx 0.9 accepts only `&'static str` SQL (no
// `format!`), and `concat!` needs literals.
macro_rules! account_columns {
    () => {
        "id, username, display_name, role, password_hash, disabled, created_at"
    };
}
macro_rules! token_columns {
    () => {
        "id, family_id, account_id, created_at, expires_at, rotated_at, revoked_at"
    };
}
macro_rules! session_columns {
    () => {
        "id, driver_account, resume_hash, car, vehicle_class, track, session_type, lmu_version, \
         app_version, schema_version, started_at, status, close_reason, last_activity, ended_at"
    };
}

impl AccountStore for PgStore {
    async fn create_account(&self, account: &Account) -> Result<(), StoreError> {
        sqlx::query(
            "INSERT INTO accounts (id, username, display_name, role, password_hash, disabled, created_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(account.id)
        .bind(&account.username)
        .bind(&account.display_name)
        .bind(account.role.as_str())
        .bind(&account.password_hash)
        .bind(account.disabled)
        .bind(account.created_at)
        .execute(&self.pool)
        .await
        .map_err(|e| {
            if is_unique_violation(&e) {
                StoreError::Conflict("username")
            } else {
                StoreError::Database(e)
            }
        })?;
        Ok(())
    }

    async fn account_by_username(&self, username: &str) -> Result<Option<Account>, StoreError> {
        let row = sqlx::query(concat!(
            "SELECT ",
            account_columns!(),
            " FROM accounts WHERE username = $1"
        ))
        .bind(username)
        .fetch_optional(&self.pool)
        .await?;
        row.as_ref().map(account_from_row).transpose()
    }

    async fn account_by_id(&self, id: Uuid) -> Result<Option<Account>, StoreError> {
        let row = sqlx::query(concat!(
            "SELECT ",
            account_columns!(),
            " FROM accounts WHERE id = $1"
        ))
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        row.as_ref().map(account_from_row).transpose()
    }

    async fn record_login(&self, id: Uuid, at: DateTime<Utc>) -> Result<(), StoreError> {
        sqlx::query("UPDATE accounts SET last_login_at = $2 WHERE id = $1")
            .bind(id)
            .bind(at)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn create_token_family(
        &self,
        family: Uuid,
        account: Uuid,
        at: DateTime<Utc>,
    ) -> Result<(), StoreError> {
        sqlx::query("INSERT INTO token_families (id, account_id, created_at) VALUES ($1, $2, $3)")
            .bind(family)
            .bind(account)
            .bind(at)
            .execute(&self.pool)
            .await
            .map_err(|e| {
                if is_unique_violation(&e) {
                    StoreError::Conflict("token family")
                } else {
                    StoreError::Database(e)
                }
            })?;
        Ok(())
    }

    async fn family_revoked(&self, family: Uuid) -> Result<Option<bool>, StoreError> {
        let row = sqlx::query(
            "SELECT revoked_at IS NOT NULL AS revoked FROM token_families WHERE id = $1",
        )
        .bind(family)
        .fetch_optional(&self.pool)
        .await?;
        Ok(match row {
            Some(row) => Some(row.try_get::<bool, _>("revoked")?),
            None => None,
        })
    }

    async fn inactive_families(&self, families: &[Uuid]) -> Result<Vec<Uuid>, StoreError> {
        if families.is_empty() {
            return Ok(Vec::new());
        }
        // A family is active only while it exists (its account was not deleted), is not revoked
        // and its account is not disabled; every other id is returned.
        let rows = sqlx::query(
            "SELECT f.id FROM unnest($1::uuid[]) AS f(id) \
             WHERE NOT EXISTS ( \
                 SELECT 1 FROM token_families t JOIN accounts a ON a.id = t.account_id \
                 WHERE t.id = f.id AND t.revoked_at IS NULL AND NOT a.disabled)",
        )
        .bind(families)
        .fetch_all(&self.pool)
        .await?;
        rows.iter()
            .map(|row| row.try_get::<Uuid, _>("id").map_err(StoreError::from))
            .collect()
    }

    async fn insert_refresh_token(
        &self,
        token: &NewRefreshToken,
        keep_active: usize,
    ) -> Result<(), StoreError> {
        let keep = i64::try_from(keep_active).unwrap_or(i64::MAX);
        let mut tx = self.pool.begin().await?;
        lock_family(&mut tx, token.family_id).await?;
        insert_token(&mut tx, token).await?;
        // Revoke the oldest active tokens of the family beyond `keep` (newest first).
        sqlx::query(
            "UPDATE refresh_tokens SET revoked_at = $2 WHERE id IN ( \
                 SELECT id FROM refresh_tokens \
                 WHERE family_id = $1 AND revoked_at IS NULL AND rotated_at IS NULL AND expires_at > $2 \
                 ORDER BY created_at DESC, id DESC OFFSET $3)",
        )
        .bind(token.family_id)
        .bind(token.created_at)
        .bind(keep)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    async fn refresh_token_by_hash(
        &self,
        hash: &TokenHash,
    ) -> Result<Option<RefreshTokenRecord>, StoreError> {
        let row = sqlx::query(concat!(
            "SELECT ",
            token_columns!(),
            " FROM refresh_tokens WHERE token_hash = $1"
        ))
        .bind(hash.as_bytes().as_slice())
        .fetch_optional(&self.pool)
        .await?;
        row.as_ref().map(token_from_row).transpose()
    }

    async fn rotate_refresh_token(
        &self,
        old: Uuid,
        successor: &NewRefreshToken,
        at: DateTime<Utc>,
    ) -> Result<RotateOutcome, StoreError> {
        let mut tx = self.pool.begin().await?;
        // The family row first (see `lock_family`); `old` names it.
        let family: Option<Uuid> = sqlx::query_scalar(
            "SELECT f.id FROM token_families f \
             JOIN refresh_tokens t ON t.family_id = f.id WHERE t.id = $1 \
             FOR UPDATE OF f",
        )
        .bind(old)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(family) = family else {
            tx.rollback().await?;
            return Ok(RotateOutcome::Lost);
        };
        let updated = sqlx::query(
            "UPDATE refresh_tokens SET rotated_at = $2 \
             WHERE id = $1 AND rotated_at IS NULL AND revoked_at IS NULL",
        )
        .bind(old)
        .bind(at)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if updated == 0 {
            tx.rollback().await?;
            return Ok(RotateOutcome::Lost);
        }
        // One live token per login: every other live token of the family is superseded. The
        // family lock makes this see every successor committed by an earlier writer.
        sqlx::query(
            "UPDATE refresh_tokens SET rotated_at = $2 \
             WHERE family_id = $1 AND rotated_at IS NULL AND revoked_at IS NULL",
        )
        .bind(family)
        .bind(at)
        .execute(&mut *tx)
        .await?;
        insert_token(&mut tx, successor).await?;
        tx.commit().await?;
        Ok(RotateOutcome::Rotated)
    }

    async fn revoke_family(&self, family: Uuid, at: DateTime<Utc>) -> Result<(), StoreError> {
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "UPDATE token_families SET revoked_at = $2 WHERE id = $1 AND revoked_at IS NULL",
        )
        .bind(family)
        .bind(at)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE refresh_tokens SET revoked_at = $2 WHERE family_id = $1 AND revoked_at IS NULL",
        )
        .bind(family)
        .bind(at)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    async fn revoke_account_tokens(
        &self,
        account: Uuid,
        at: DateTime<Utc>,
    ) -> Result<u64, StoreError> {
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "UPDATE token_families SET revoked_at = $2 WHERE account_id = $1 AND revoked_at IS NULL",
        )
        .bind(account)
        .bind(at)
        .execute(&mut *tx)
        .await?;
        let revoked = sqlx::query(
            "UPDATE refresh_tokens SET revoked_at = $2 WHERE account_id = $1 AND revoked_at IS NULL",
        )
        .bind(account)
        .bind(at)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        tx.commit().await?;
        Ok(revoked)
    }

    async fn purge_refresh_tokens(&self, before: DateTime<Utc>) -> Result<u64, StoreError> {
        Ok(
            sqlx::query("DELETE FROM refresh_tokens WHERE expires_at < $1")
                .bind(before)
                .execute(&self.pool)
                .await?
                .rows_affected(),
        )
    }
}

impl SessionStore for PgStore {
    async fn insert_session(&self, record: &SessionRecord) -> Result<(), StoreError> {
        sqlx::query(concat!(
            "INSERT INTO sessions (",
            session_columns!(),
            ") VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15)"
        ))
        .bind(session_id_param(record.id)?)
        .bind(record.driver_account)
        .bind(record.resume_hash.map(|h| h.as_bytes().to_vec()))
        .bind(&record.meta.car)
        .bind(&record.meta.vehicle_class)
        .bind(&record.meta.track)
        .bind(record.meta.session_type)
        .bind(&record.meta.lmu_version)
        .bind(&record.meta.app_version)
        .bind(i32::from(record.meta.schema_version))
        .bind(record.started_at)
        .bind(record.status.as_str())
        .bind(&record.close_reason)
        .bind(record.last_activity)
        .bind(record.ended_at)
        .execute(&self.pool)
        .await
        .map_err(|e| {
            if is_unique_violation(&e) {
                StoreError::Conflict("session id")
            } else {
                StoreError::Database(e)
            }
        })?;
        Ok(())
    }

    async fn session(&self, id: u64) -> Result<Option<SessionRecord>, StoreError> {
        let Ok(id) = i64::try_from(id) else {
            return Ok(None);
        };
        let row = sqlx::query(concat!(
            "SELECT ",
            session_columns!(),
            " FROM sessions WHERE id = $1"
        ))
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        row.as_ref().map(session_from_row).transpose()
    }

    async fn set_session_status(
        &self,
        id: u64,
        status: SessionStatus,
        at: DateTime<Utc>,
    ) -> Result<(), StoreError> {
        sqlx::query(
            "UPDATE sessions SET status = $2, last_activity = $3 WHERE id = $1 AND status <> 'closed'",
        )
        .bind(session_id_param(id)?)
        .bind(status.as_str())
        .bind(at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn close_session(
        &self,
        id: u64,
        reason: &str,
        at: DateTime<Utc>,
    ) -> Result<(), StoreError> {
        let id = session_id_param(id)?;
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "UPDATE sessions SET status = 'closed', close_reason = $2, resume_hash = NULL, \
             last_activity = $3, ended_at = $3 WHERE id = $1 AND status <> 'closed'",
        )
        .bind(id)
        .bind(reason)
        .bind(at)
        .execute(&mut *tx)
        .await?;
        sqlx::query("DELETE FROM session_engineers WHERE session_id = $1")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    async fn touch_sessions(&self, ids: &[u64], at: DateTime<Utc>) -> Result<(), StoreError> {
        if ids.is_empty() {
            return Ok(());
        }
        let ids = ids
            .iter()
            .map(|&id| session_id_param(id))
            .collect::<Result<Vec<i64>, StoreError>>()?;
        sqlx::query(
            "UPDATE sessions SET last_activity = $2 WHERE id = ANY($1) AND status <> 'closed'",
        )
        .bind(ids)
        .bind(at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn expire_sessions(
        &self,
        idle_before: DateTime<Utc>,
        at: DateTime<Utc>,
    ) -> Result<u64, StoreError> {
        let mut tx = self.pool.begin().await?;
        let closed = sqlx::query(
            "UPDATE sessions SET status = 'closed', close_reason = $2, resume_hash = NULL, ended_at = $3 \
             WHERE status <> 'closed' AND last_activity < $1",
        )
        .bind(idle_before)
        .bind(CLOSE_REASON_EXPIRED)
        .bind(at)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        // Also removes slots a crash left behind in sessions closed earlier.
        sqlx::query(
            "DELETE FROM session_engineers e USING sessions s \
             WHERE e.session_id = s.id AND s.status = 'closed'",
        )
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(closed)
    }

    async fn purge_closed_sessions(&self, ended_before: DateTime<Utc>) -> Result<u64, StoreError> {
        // Engineer slots go with their session (ON DELETE CASCADE).
        let deleted = sqlx::query(
            "DELETE FROM sessions WHERE status = 'closed' \
             AND COALESCE(ended_at, last_activity) < $1",
        )
        .bind(ended_before)
        .execute(&self.pool)
        .await?
        .rows_affected();
        Ok(deleted)
    }

    async fn put_engineer_slot(&self, slot: &EngineerSlotRecord) -> Result<(), StoreError> {
        // Only for an open session: a write that lands after the session closed is dropped.
        sqlx::query(
            "INSERT INTO session_engineers \
                 (session_id, account_id, connection_id, connected_since, dropped_at) \
             SELECT $1, $2, $3, $4, $5 \
             WHERE EXISTS (SELECT 1 FROM sessions WHERE id = $1 AND status <> 'closed') \
             ON CONFLICT (session_id, account_id) DO UPDATE SET \
                 connection_id = EXCLUDED.connection_id, \
                 connected_since = EXCLUDED.connected_since, \
                 dropped_at = EXCLUDED.dropped_at",
        )
        .bind(session_id_param(slot.session_id)?)
        .bind(slot.account_id)
        .bind(connection_id_param(slot.connection_id)?)
        .bind(slot.connected_since)
        .bind(slot.dropped_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn delete_engineer_slot(
        &self,
        session_id: u64,
        account: Uuid,
        connection_id: u64,
    ) -> Result<(), StoreError> {
        sqlx::query(
            "DELETE FROM session_engineers \
             WHERE session_id = $1 AND account_id = $2 AND connection_id = $3",
        )
        .bind(session_id_param(session_id)?)
        .bind(account)
        .bind(connection_id_param(connection_id)?)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn engineer_slots(&self, session_id: u64) -> Result<Vec<EngineerSlotRecord>, StoreError> {
        let Ok(id) = i64::try_from(session_id) else {
            return Ok(Vec::new());
        };
        let rows = sqlx::query(
            "SELECT session_id, account_id, connection_id, connected_since, dropped_at \
             FROM session_engineers WHERE session_id = $1 ORDER BY connection_id",
        )
        .bind(id)
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(slot_from_row).collect()
    }

    async fn ping(&self) -> Result<(), StoreError> {
        sqlx::query("SELECT 1").execute(&self.pool).await?;
        Ok(())
    }
}
