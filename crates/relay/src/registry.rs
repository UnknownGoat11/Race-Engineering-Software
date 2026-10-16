//! Session registry (relay memory): driver sessions, engineer slots, one-time connection codes
//! (D-022), engineer resume tickets, the join cache and the fan-out of driver frames.
//!
//! Every operation takes the current monotonic time as an argument, so the logic is tested
//! deterministically (codes expiring after 10 minutes are tested without waiting).
//!
//! Locking: the session map ([`RwLock`]) is always taken before a session lock, a session lock
//! before the tombstone lock, and outbox locks are leaves. The hot path
//! ([`Registry::forward_driver_frame`]) only locks the one session it serves.
//!
//! Secrets: connection codes are matched only by keyed digest ([`CodeHasher`]); resume tokens and
//! tickets are known only by SHA-256 digest. The one exception is the plaintext of a session's
//! single unused code ([`CodePlaintext`], memory only, redacted, wiped on redemption, rotation,
//! expiry, session end and shutdown): a resuming driver is shown that same code with its original
//! expiry; the same holds when an engineer leaves or its ticket expires (the driver may have
//! shared the code already). A redemption and a kick (the code may have leaked) show a freshly
//! issued code (the unused one is invalidated at the same time), as does every CONNECT_CODE
//! request, and every SESSION_INFO to an engineer carries a new ticket. All comparisons are
//! constant-time.
//!
//! Engineer tickets (D-022, PROTOCOL.md §6): a slot knows its newest ticket and up to two
//! (`MAX_PREVIOUS_TICKETS`) earlier ones the engineer may still hold (the SESSION_INFO with a newer
//! one may have been lost in a second blip). Each earlier ticket is accepted
//! `previous_ticket_grace` after the ticket that replaced it was issued. A resume with any accepted
//! ticket leaves the presented one as the only earlier ticket of the fresh one, so once the newest
//! ticket is used the older ones are dead; a driver-restart notice adds the current ticket and
//! keeps the earlier ones still inside their grace. Kick, DISCONNECT, takeover, ticket expiry and
//! session end remove the slot and all of its tickets.
//!
//! Relay restarts (D-020, D-022): engineer slot changes are queued per session as [`SlotWrite`]s
//! in registry order and persisted by the caller (`SessionCell::take_slot_writes`). After a
//! restart a session is rebuilt from the store when its driver or one of its engineers resumes;
//! restored slots know no ticket digest and check the ticket's tag ([`TicketSealer`]) instead.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use parking_lot::{Mutex, RwLock};
use protocol::frame::{Frame, FrameHeader, FrameMeta, FramePriority, MsgType};
use protocol::messages::{
    ConnectCode, Disconnect, DisconnectReason, DriverPresence, EngineerEntry, EngineerLink,
    EngineerList, ErrorCode, ErrorMsg, IssuedCode, Message, ResumeCredential, Secret, SessionClose,
    SessionCloseReason, SessionInfo, SnapshotReason, SnapshotRequest, normalize_connect_code,
};
use protocol::sequence::SnapshotRequestLimiter;
use telemetry_model::enums::SessionType;
use thiserror::Error;
use uuid::Uuid;

use crate::codes::{CodeHasher, CodePlaintext, generate_connect_code};
use crate::config::RelayConfig;
use crate::metrics::{Metrics, RegistryGauges};
use crate::outbox::{ConnHandle, Forwarded, OutboxStats, Outgoing, PushError};
use crate::ratelimit::{RateMeter, RateSpec};
use crate::secrets::{RandomError, TokenHash, presented_token_hash};
use crate::store::{EngineerSlotRecord, SessionMeta, SessionRecord};
use crate::tickets::{SlotRef, TicketSealer};
use crate::time::{
    datetime_to_ns, duration_ms, duration_ns, mono_ns_since, ns_to_datetime, unix_now_ns,
};

/// Log component of this module.
const COMPONENT: &str = "relay.session";
/// Used and replaced code digests remembered per session (SessionFull vs CodeInvalid answers).
pub const MAX_SPENT_CODES_PER_SESSION: usize = 32;
/// Closed sessions and codes remembered for `SessionClosed` answers.
pub const MAX_TOMBSTONES: usize = 4_096;
/// Slot writes queued per session before the oldest is dropped. Every operation that queues a
/// write is followed by a flush, so a queue only grows while the store hangs; the cap is a safety
/// net for that case (a dropped write is logged as an error).
pub const MAX_PENDING_SLOT_WRITES: usize = 64;

/// Registry settings (from [`RelayConfig`]).
#[derive(Debug, Clone)]
pub struct RegistryConfig {
    pub max_sessions: usize,
    pub max_engineers: u16,
    pub connect_code_ttl: Duration,
    pub engineer_ticket_ttl: Duration,
    /// How long a slot's previous ticket is still accepted after a newer one was issued.
    pub previous_ticket_grace: Duration,
    pub driver_grace: Duration,
    pub join_cache_max_age: Duration,
    pub snapshot_request_min_interval: Duration,
    pub heartbeat_degraded: Duration,
    pub degraded_drop_window: Duration,
    pub session_rate: RateSpec,
}

impl RegistryConfig {
    /// The registry part of the relay configuration.
    pub fn from_relay(config: &RelayConfig) -> Self {
        Self {
            max_sessions: config.limits.max_sessions,
            max_engineers: config.limits.max_engineers_per_session,
            connect_code_ttl: config.timeouts.connect_code_ttl,
            engineer_ticket_ttl: config.timeouts.engineer_ticket_ttl,
            previous_ticket_grace: config.timeouts.previous_ticket_grace,
            driver_grace: config.timeouts.driver_grace,
            join_cache_max_age: config.timeouts.join_cache_max_age,
            snapshot_request_min_interval: Duration::from_millis(
                protocol::timing::SNAPSHOT_REQUEST_MIN_INTERVAL_MS,
            ),
            heartbeat_degraded: config.timeouts.heartbeat_degraded,
            degraded_drop_window: config.timeouts.degraded_drop_window,
            session_rate: config.limits.session_rate,
        }
    }

    /// Tombstones live as long as a code or ticket could still be presented.
    fn tombstone_ttl(&self) -> Duration {
        self.connect_code_ttl.max(self.engineer_ticket_ttl)
    }
}

/// An authenticated account as shown to the other side (SESSION_INFO, ENGINEER_LIST).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    pub account: Uuid,
    pub username: String,
    pub display_name: String,
}

/// Registry failures for driver operations.
#[derive(Debug, Error)]
pub enum RegistryError {
    #[error("the relay holds its maximum number of sessions")]
    Full,
    #[error(transparent)]
    Random(#[from] RandomError),
}

/// Why a SESSION_JOIN was refused (PROTOCOL.md §6).
#[derive(Debug, Error)]
pub enum JoinError {
    #[error("unknown, used or replaced connection code")]
    CodeInvalid,
    #[error("connection code expired")]
    CodeExpired,
    #[error("every engineer slot of the session is taken")]
    SessionFull { max: u16 },
    #[error("the driver's session has ended")]
    SessionClosed,
    #[error(transparent)]
    Random(#[from] RandomError),
}

impl JoinError {
    /// The ERROR reply.
    pub fn to_error_msg(&self) -> ErrorMsg {
        let (code, message) = match self {
            JoinError::CodeInvalid => (
                ErrorCode::CodeInvalid,
                "connection code is not valid".to_owned(),
            ),
            JoinError::CodeExpired => (
                ErrorCode::CodeExpired,
                "connection code has expired".to_owned(),
            ),
            JoinError::SessionFull { max } => (
                ErrorCode::SessionFull,
                format!("Driver already has {max} engineers connected"),
            ),
            JoinError::SessionClosed => (
                ErrorCode::SessionClosed,
                "the driver's session has ended".to_owned(),
            ),
            JoinError::Random(_) => (ErrorCode::Internal, "internal relay error".to_owned()),
        };
        ErrorMsg {
            code,
            message,
            fatal: false,
        }
    }
}

/// Why a SESSION_RESUME was refused.
#[derive(Debug, Error)]
pub enum ResumeError {
    /// The session is not in relay memory (relay restart): the caller consults the store.
    #[error("session not in relay memory")]
    NotInMemory,
    #[error("unknown session")]
    NotFound,
    #[error("session closed")]
    Closed,
    #[error("resume credential refused")]
    NotAuthorized,
    /// The session store could not be read (relay restart path).
    #[error("session store unavailable")]
    StoreUnavailable,
    /// Too many resumes of sessions not in relay memory (each one reads the store).
    #[error("session resume rate limit reached")]
    RateLimited,
    #[error(transparent)]
    Registry(#[from] RegistryError),
}

impl ResumeError {
    /// The ERROR reply.
    pub fn to_error_msg(&self) -> ErrorMsg {
        let (code, message) = match self {
            ResumeError::NotInMemory | ResumeError::NotFound => {
                (ErrorCode::SessionNotFound, "unknown session")
            }
            ResumeError::Closed => (ErrorCode::SessionClosed, "the session has ended"),
            ResumeError::NotAuthorized => (
                ErrorCode::NotAuthorized,
                "resume credential refused; a new connection code is needed",
            ),
            ResumeError::RateLimited => (
                ErrorCode::RateLimited,
                "too many session resumes; try again later",
            ),
            ResumeError::Registry(RegistryError::Full) => {
                (ErrorCode::ServerBusy, "the relay is at its session limit")
            }
            ResumeError::StoreUnavailable | ResumeError::Registry(RegistryError::Random(_)) => {
                (ErrorCode::Internal, "internal relay error")
            }
        };
        ErrorMsg {
            code,
            message: message.to_owned(),
            fatal: false,
        }
    }
}

/// Why a stream frame was not forwarded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForwardReject {
    /// The connection is no longer the session's driver / an engineer of it (replaced, closed).
    NotBound,
    /// The session's inbound rate limit is exhausted.
    RateLimited,
    /// The sequence does not increase (§5): duplicate, replay or a restart without announcement.
    StaleSequence { last: u64, got: u64 },
}

/// Result of a forwarded driver frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FanOut {
    /// Engineers the frame was queued for.
    pub receivers: usize,
    /// Older live frames dropped (latest-wins) by this frame across all receivers.
    pub dropped: u32,
}

/// Outcome of an engineer SNAPSHOT_REQUEST.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotRequestOutcome {
    Forwarded,
    /// Inside the per-engineer minimum interval (§5): dropped and counted, not a violation.
    Coalesced,
    /// The driver is away (grace period).
    DriverAway,
    /// The relay could not encode the request (logged); nothing was forwarded.
    NotEncoded,
}

/// Sessions changed by housekeeping, for the store.
#[derive(Debug, Default)]
pub struct Housekeeping {
    /// Sessions closed because the driver grace period elapsed.
    pub expired: Vec<u64>,
    /// Sessions with queued slot writes (expired engineer tickets) to flush.
    pub slot_writes: Vec<Arc<SessionCell>>,
}

/// A store write owed for an engineer slot change. Queued on the session in registry order and
/// applied in that order by whoever flushes the session (under its store lock), so the store never
/// sees slot changes out of order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlotWrite {
    /// Insert or replace the slot (join, resume, connection drop).
    Put(EngineerSlotRecord),
    /// The slot ended (DISCONNECT, ENGINEER_KICK, ticket expiry): its tickets die with the row.
    Delete {
        session_id: u64,
        account: Uuid,
        connection_id: u64,
    },
}

/// An engineer slot read from the store after a relay restart, with its engineer's identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredSlot {
    pub record: EngineerSlotRecord,
    pub engineer: Identity,
}

/// A session read from the store after a relay restart: its record, its driver's identity and the
/// engineer slots it held.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredSession {
    pub record: SessionRecord,
    pub driver: Identity,
    pub slots: Vec<StoredSlot>,
}

/// A session created by [`Registry::create_session`].
#[derive(Debug)]
pub struct Created {
    pub cell: Arc<SessionCell>,
    /// Older sessions of the same driver account that were closed (replaced).
    pub replaced: Vec<u64>,
}

/// Input of [`Registry::create_session`].
#[derive(Debug, Clone)]
pub struct NewSession {
    pub id: u64,
    pub driver: Identity,
    pub meta: SessionMeta,
    pub started_at_ns: u64,
    /// The driver's resume token (sent once in SESSION_INFO, kept only as `resume_hash`).
    pub resume_token: Secret,
    pub resume_hash: TokenHash,
}

/// Diagnostics view of one engineer slot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineerDebug {
    pub connection_id: u64,
    pub username: String,
    pub connected: bool,
    pub link: EngineerLink,
    pub outbox: Option<OutboxStats>,
}

/// Diagnostics view of one session (tests, admin).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionDebug {
    pub id: u64,
    pub driver_attached: bool,
    pub driver_outbox: Option<OutboxStats>,
    pub engineers: Vec<EngineerDebug>,
    pub code_active: bool,
    pub spent_codes: usize,
    pub cached_frames: usize,
    pub last_driver_sequence: u64,
}

/// One session behind its own lock.
#[derive(Debug)]
pub struct SessionCell {
    id: u64,
    inner: Mutex<Session>,
    /// Serializes the store writes of this session so they land in registry order.
    store_order: tokio::sync::Mutex<()>,
}

impl SessionCell {
    /// Session id.
    pub fn id(&self) -> u64 {
        self.id
    }

    /// Held while a registry change and its store write happen together.
    pub async fn store_order(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.store_order.lock().await
    }

    /// Whether the session has ended.
    pub fn is_closed(&self) -> bool {
        self.inner.lock().closed
    }

    /// Takes the queued slot writes of this session, oldest first. Call it while holding
    /// [`SessionCell::store_order`] and apply the writes in the returned order.
    pub fn take_slot_writes(&self) -> Vec<SlotWrite> {
        std::mem::take(&mut self.inner.lock().pending_writes)
    }

    /// Restores unpersisted slot writes to the front of the queue after a store write failure (H3).
    pub fn prepend_slot_writes(&self, mut writes: Vec<SlotWrite>) {
        if writes.is_empty() {
            return;
        }
        let mut inner = self.inner.lock();
        writes.append(&mut inner.pending_writes);
        inner.pending_writes = writes;
    }
}

#[derive(Debug)]
enum DriverLink {
    Attached(Arc<ConnHandle>),
    /// Away since `since`, plus `carried` absence from before a relay restart (kept as a
    /// duration: the monotonic clock of the new process may not reach back that far).
    Away {
        since: Instant,
        carried: Duration,
    },
}

#[derive(Debug)]
enum SlotLink {
    Connected(Arc<ConnHandle>),
    /// Down since `since`, plus `carried` from before a relay restart (see [`DriverLink::Away`]).
    Dropped {
        since: Instant,
        carried: Duration,
    },
}

/// Time elapsed since `since` at `now`, plus `carried`.
fn elapsed_since(since: Instant, carried: Duration, now: Instant) -> Duration {
    now.saturating_duration_since(since).saturating_add(carried)
}

#[derive(Debug)]
struct EngineerSlot {
    connection_id: u64,
    engineer: Identity,
    connected_since_ns: u64,
    /// Digest of the newest ticket this process issued for the slot. `None` for a slot restored
    /// from the store after a relay restart: its ticket is checked by tag ([`TicketSealer`]).
    ticket: Option<TokenHash>,
    /// Earlier tickets the engineer may still hold, oldest first, each accepted within
    /// `previous_ticket_grace` after the ticket that replaced it was issued (the SESSION_INFO
    /// carrying the newer one may have been lost). See [`EngineerSlot::install_ticket`]. Always
    /// empty while `ticket` is `None`.
    previous: [Option<PreviousTicket>; MAX_PREVIOUS_TICKETS],
    link: SlotLink,
    announced: EngineerLink,
    last_seq: u64,
    snapshot_limiter: SnapshotRequestLimiter,
}

impl EngineerSlot {
    fn conn(&self) -> Option<&Arc<ConnHandle>> {
        match &self.link {
            SlotLink::Connected(c) => Some(c),
            SlotLink::Dropped { .. } => None,
        }
    }

    fn slot_ref(&self, session_id: u64) -> SlotRef {
        SlotRef {
            session_id,
            account: self.engineer.account,
            connection_id: self.connection_id,
        }
    }

    /// The store form of this slot; `dropped_at` is set while the connection is down.
    fn record(&self, session_id: u64, dropped_at: Option<DateTime<Utc>>) -> EngineerSlotRecord {
        EngineerSlotRecord {
            session_id,
            account_id: self.engineer.account,
            connection_id: self.connection_id,
            connected_since: ns_to_datetime(self.connected_since_ns),
            dropped_at,
        }
    }

    /// Whether `ticket` (with digest `digest`) is the slot's current ticket (checked by tag for a
    /// slot restored after a relay restart), or one of its earlier ones less than `grace` after
    /// the ticket that replaced it was issued.
    fn ticket_matches(
        &self,
        session_id: u64,
        ticket: &Secret,
        digest: &TokenHash,
        sealer: &TicketSealer,
        grace: Duration,
        now: Instant,
    ) -> bool {
        let current = match &self.ticket {
            Some(current) => current.ct_eq(digest),
            None => sealer.verify(ticket, self.slot_ref(session_id)),
        };
        // Every comparison always runs (no early exit revealing which ticket matched).
        let previous = self.previous.iter().flatten().fold(false, |matched, p| {
            matched
                | (p.digest.ct_eq(digest)
                    & (now.saturating_duration_since(p.newer_issued_at) < grace))
        });
        current | previous
    }

    /// Makes `digest` (issued at `now`) the slot's newest ticket. `handover` says which earlier
    /// tickets the engineer may still hold: none after a join; after a resume only the presented
    /// one (so using the newest ticket kills every older one); after a driver-restart notice the
    /// current ticket plus the earlier ones still inside `grace` (the notice and an earlier
    /// SESSION_INFO may both have been lost), at most [`MAX_PREVIOUS_TICKETS`], the oldest dropped.
    fn install_ticket(
        &mut self,
        digest: TokenHash,
        handover: TicketHandover,
        grace: Duration,
        now: Instant,
    ) {
        let replaced = |digest| PreviousTicket {
            digest,
            newer_issued_at: now,
        };
        let candidates: Vec<PreviousTicket> = match handover {
            TicketHandover::Join => Vec::new(),
            TicketHandover::Resume(presented) => vec![replaced(presented)],
            TicketHandover::Reissue => self
                .previous
                .iter()
                .flatten()
                .copied()
                .filter(|p| now.saturating_duration_since(p.newer_issued_at) < grace)
                .chain(self.ticket.map(replaced))
                .collect(),
        };
        let mut previous = [None; MAX_PREVIOUS_TICKETS];
        // Newest last: fill from the end, so the oldest candidates fall off beyond the bound.
        for (entry, p) in previous.iter_mut().rev().zip(candidates.into_iter().rev()) {
            *entry = Some(p);
        }
        self.ticket = Some(digest);
        self.previous = previous;
    }
}

/// Most earlier tickets an engineer slot keeps besides its newest one (bounded memory): the one
/// presented in the last resume, plus one more when a driver-restart notice follows a resume whose
/// SESSION_INFO may have been lost.
const MAX_PREVIOUS_TICKETS: usize = 2;

/// An earlier resume ticket of an engineer slot (see [`EngineerSlot::previous`]).
#[derive(Debug, Clone, Copy)]
struct PreviousTicket {
    digest: TokenHash,
    /// When the ticket that replaced it was issued: the grace runs from here.
    newer_issued_at: Instant,
}

/// What a change of the engineer slots means for the session's unused connection code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CodeAfterSlotChange {
    /// The code was redeemed, or the driver kicked an engineer (the code may have leaked): a
    /// fresh code.
    Fresh,
    /// A slot was released (DISCONNECT, ticket expiry): the unused, unexpired code stays valid,
    /// since the driver may have shared it already (else a fresh code, as for `Fresh`).
    Keep,
}

/// Which earlier tickets an engineer may hold when its slot gets a fresh ticket
/// ([`EngineerSlot::install_ticket`]).
#[derive(Debug, Clone, Copy)]
enum TicketHandover {
    /// A new slot (join or takeover): no earlier ticket.
    Join,
    /// A resume presented the ticket with this digest: the only earlier one it can still hold.
    Resume(TokenHash),
    /// A driver-restart notice on the engineer's open connection: it holds the current ticket, or
    /// an earlier one whose newer SESSION_INFO was lost.
    Reissue,
}

#[derive(Debug)]
struct ActiveCode {
    digest: TokenHash,
    issued_at: Instant,
    /// The expiry announced with the code (wall clock ns), re-sent unchanged with the same code.
    expires_at_ns: u64,
    /// The code itself while it is unused and unexpired (relay memory only, wiped on drop), so a
    /// resuming driver is shown the same code. `None` once expired; never set after a restart.
    plaintext: Option<CodePlaintext>,
    expiry_announced: bool,
}

#[derive(Debug, Clone)]
struct Cached {
    frame: Arc<Forwarded>,
    at: Instant,
}

/// Latest frames replayed to a joining engineer: SCHEMA_NEGOTIATE, and TELEMETRY_SNAPSHOT /
/// SCORING_UPDATE per priority.
#[derive(Debug, Default)]
struct JoinCache {
    schema: Option<Cached>,
    snapshot: [Option<Cached>; 3],
    scoring: [Option<Cached>; 3],
}

impl JoinCache {
    fn clear(&mut self) {
        *self = JoinCache::default();
    }

    fn store(&mut self, frame: &Arc<Forwarded>, now: Instant) {
        let entry = Some(Cached {
            frame: Arc::clone(frame),
            at: now,
        });
        let p = match frame.priority {
            FramePriority::P0 => 0,
            FramePriority::P1 => 1,
            FramePriority::P2 => 2,
            FramePriority::Control => 0,
        };
        match frame.msg_type {
            MsgType::SchemaNegotiate => self.schema = entry,
            MsgType::TelemetrySnapshot => self.snapshot[p] = entry,
            MsgType::ScoringUpdate => self.scoring[p] = entry,
            _ => {}
        }
    }

    fn expire(&mut self, now: Instant, max_age: Duration) {
        for slot in self.snapshot.iter_mut().chain(self.scoring.iter_mut()) {
            if slot
                .as_ref()
                .is_some_and(|c| now.saturating_duration_since(c.at) > max_age)
            {
                *slot = None;
            }
        }
    }

    /// Cached frames in their original (sequence) order.
    fn replay(&self) -> Vec<Arc<Forwarded>> {
        let mut frames: Vec<Arc<Forwarded>> = self
            .schema
            .iter()
            .chain(self.snapshot.iter().flatten())
            .chain(self.scoring.iter().flatten())
            .map(|c| Arc::clone(&c.frame))
            .collect();
        frames.sort_by_key(|f| f.sequence);
        frames
    }

    fn len(&self) -> usize {
        usize::from(self.schema.is_some())
            + self.snapshot.iter().flatten().count()
            + self.scoring.iter().flatten().count()
    }
}

#[derive(Debug)]
struct Session {
    id: u64,
    driver: Identity,
    meta: SessionMeta,
    started_at_ns: u64,
    resume_hash: TokenHash,
    driver_link: DriverLink,
    engineers: Vec<EngineerSlot>,
    code: Option<ActiveCode>,
    spent_codes: VecDeque<TokenHash>,
    cache: JoinCache,
    driver_seq: u64,
    relay_seq: u64,
    next_connection_id: u64,
    rate: RateMeter,
    closed: bool,
    /// Slot changes not yet written to the store, oldest first.
    pending_writes: Vec<SlotWrite>,
}

/// Read-only context of session operations.
struct Ctx<'a> {
    cfg: &'a RegistryConfig,
    hasher: &'a CodeHasher,
    sealer: &'a TicketSealer,
    metrics: &'a Metrics,
    epoch: Instant,
}

impl Session {
    fn driver_conn(&self) -> Option<&Arc<ConnHandle>> {
        match &self.driver_link {
            DriverLink::Attached(c) => Some(c),
            DriverLink::Away { .. } => None,
        }
    }

    fn driver_is(&self, conn_id: u64) -> bool {
        self.driver_conn().is_some_and(|c| c.id == conn_id)
    }

    fn send_driver(&self, message: &Message) {
        if let Some(conn) = self.driver_conn() {
            conn.send(message, self.id);
        }
    }

    /// DRIVER_PRESENCE as of `now` (PROTOCOL.md §6 "Driver presence"): while the driver is away,
    /// since when (wall clock, absence carried over a relay restart included) and when its grace
    /// ends.
    fn presence(&self, ctx: &Ctx<'_>, now: Instant) -> DriverPresence {
        match self.driver_link {
            DriverLink::Attached(_) => DriverPresence::present(self.id),
            DriverLink::Away { since, carried } => {
                let away_for = elapsed_since(since, carried, now);
                let since_ns = unix_now_ns().saturating_sub(duration_ns(away_for));
                DriverPresence::away(
                    self.id,
                    since_ns,
                    since_ns.saturating_add(duration_ns(ctx.cfg.driver_grace)),
                )
            }
        }
    }

    /// Sends the driver's presence to every connected engineer.
    fn announce_presence(&self, ctx: &Ctx<'_>, now: Instant) {
        let message = Message::DriverPresence(self.presence(ctx, now));
        for slot in &self.engineers {
            if let Some(conn) = slot.conn() {
                conn.send(&message, self.id);
            }
        }
    }

    fn info(&self, resume: ResumeCredential) -> SessionInfo {
        SessionInfo {
            session_id: self.id,
            driver_display_name: self.driver.display_name.clone(),
            driver_username: self.driver.username.clone(),
            car: self.meta.car.clone(),
            vehicle_class: self.meta.vehicle_class.clone(),
            track: self.meta.track.clone(),
            session_type: self.meta.session_type.map(SessionType::from_lmu),
            started_at_ns: self.started_at_ns,
            schema_version: self.meta.schema_version,
            resume,
        }
    }

    fn slots_used(&self) -> u16 {
        u16::try_from(self.engineers.len()).unwrap_or(u16::MAX)
    }

    fn is_full(&self, cfg: &RegistryConfig) -> bool {
        self.slots_used() >= cfg.max_engineers
    }

    fn retire_code(&mut self) {
        if let Some(old) = self.code.take() {
            if self.spent_codes.len() >= MAX_SPENT_CODES_PER_SESSION {
                self.spent_codes.pop_front();
            }
            self.spent_codes.push_back(old.digest);
        }
    }

    /// Issues a fresh code (the unused one is invalidated).
    fn issue_code(&mut self, ctx: &Ctx<'_>, now: Instant) -> Result<IssuedCode, RandomError> {
        let code = generate_connect_code()?;
        let digest = ctx.hasher.digest(&code);
        let expires_at_ns = unix_now_ns().saturating_add(duration_ns(ctx.cfg.connect_code_ttl));
        self.retire_code();
        self.code = Some(ActiveCode {
            digest,
            issued_at: now,
            expires_at_ns,
            plaintext: Some(CodePlaintext::new(&code)),
            expiry_announced: false,
        });
        Ok(IssuedCode {
            code,
            expires_at_ns,
        })
    }

    /// The code to show a resuming driver: the unused code again, with its original expiry, while
    /// it is unexpired and its plaintext is in memory; otherwise as [`Session::fresh_code_or_none`]
    /// (a fresh code, none while every slot is taken).
    fn current_code_or_fresh(&mut self, ctx: &Ctx<'_>, now: Instant) -> Option<IssuedCode> {
        if !self.is_full(ctx.cfg)
            && let Some(code) = &self.code
            && now.saturating_duration_since(code.issued_at) < ctx.cfg.connect_code_ttl
            && let Some(plaintext) = &code.plaintext
        {
            return Some(IssuedCode {
                code: plaintext.to_secret(),
                expires_at_ns: code.expires_at_ns,
            });
        }
        self.fresh_code_or_none(ctx, now)
    }

    /// The code to announce now: none while every slot is taken, otherwise a fresh one.
    fn fresh_code_or_none(&mut self, ctx: &Ctx<'_>, now: Instant) -> Option<IssuedCode> {
        if self.is_full(ctx.cfg) {
            self.retire_code();
            return None;
        }
        match self.issue_code(ctx, now) {
            Ok(code) => Some(code),
            Err(e) => {
                tracing::error!(component = COMPONENT, session_id = self.id, error = %e, "connection code not issued");
                self.retire_code();
                None
            }
        }
    }

    fn send_connect_code(&self, code: Option<IssuedCode>, ctx: &Ctx<'_>) {
        self.send_driver(&Message::ConnectCode(ConnectCode {
            code,
            engineers_connected: self.slots_used(),
            engineers_max: ctx.cfg.max_engineers,
        }));
    }

    fn link_of(&self, slot: &EngineerSlot, ctx: &Ctx<'_>, now: Instant) -> EngineerLink {
        match &slot.link {
            SlotLink::Dropped { .. } => EngineerLink::Reconnecting,
            SlotLink::Connected(c) => {
                let silent =
                    now.saturating_duration_since(c.last_inbound()) > ctx.cfg.heartbeat_degraded;
                if silent || c.outbox.dropped_recently(now, ctx.cfg.degraded_drop_window) {
                    EngineerLink::Degraded
                } else {
                    EngineerLink::Healthy
                }
            }
        }
    }

    /// Recomputes every link state and sends ENGINEER_LIST to the driver.
    fn send_engineer_list(&mut self, ctx: &Ctx<'_>, now: Instant) {
        let links: Vec<EngineerLink> = self
            .engineers
            .iter()
            .map(|slot| self.link_of(slot, ctx, now))
            .collect();
        for (slot, link) in self.engineers.iter_mut().zip(links) {
            slot.announced = link;
        }
        let list = EngineerList {
            engineers: self
                .engineers
                .iter()
                .map(|s| EngineerEntry {
                    connection_id: s.connection_id,
                    username: s.engineer.username.clone(),
                    display_name: s.engineer.display_name.clone(),
                    connected_since_ns: s.connected_since_ns,
                    link: s.announced,
                })
                .collect(),
        };
        self.send_driver(&Message::EngineerList(list));
    }

    /// After the slot usage changed: tell the driver (the code per `code`, none while every slot
    /// is taken) and update the list. While the driver is away nothing is sent; its resume sends
    /// the current state.
    fn on_slots_changed(&mut self, code: CodeAfterSlotChange, ctx: &Ctx<'_>, now: Instant) {
        if self.driver_conn().is_none() {
            if self.is_full(ctx.cfg) {
                self.retire_code();
            }
            return;
        }
        let code = match code {
            CodeAfterSlotChange::Fresh => self.fresh_code_or_none(ctx, now),
            CodeAfterSlotChange::Keep => self.current_code_or_fresh(ctx, now),
        };
        self.send_connect_code(code, ctx);
        self.send_engineer_list(ctx, now);
    }

    /// Relay-originated SNAPSHOT_REQUEST to the driver (its own sequence space, §5).
    fn request_snapshot(&mut self, reason: SnapshotReason, ctx: &Ctx<'_>) {
        let Some(driver) = self.driver_conn() else {
            return;
        };
        let seq = self.relay_seq.saturating_add(1);
        let meta = FrameMeta::stream(
            FramePriority::Control,
            self.id,
            seq,
            mono_ns_since(ctx.epoch),
        );
        match Message::SnapshotRequest(SnapshotRequest { reason }).to_frame(&meta) {
            Ok(frame) => {
                if driver.outbox.push_reliable(Outgoing::Frame(frame)).is_ok() {
                    self.relay_seq = seq;
                }
            }
            Err(e) => {
                tracing::error!(component = COMPONENT, session_id = self.id, error = %e, "snapshot request not encoded")
            }
        }
    }

    /// Queues a store write for a slot change (see [`SlotWrite`]).
    fn queue_write(&mut self, write: SlotWrite) {
        if self.pending_writes.len() >= MAX_PENDING_SLOT_WRITES {
            if let Some(pos) = self
                .pending_writes
                .iter()
                .position(|w| matches!(w, SlotWrite::Put(_)))
            {
                self.pending_writes.remove(pos);
                tracing::error!(
                    component = COMPONENT,
                    session_id = self.id,
                    "engineer slot write backlog exceeded; dropped oldest Put write"
                );
            } else if matches!(write, SlotWrite::Put(_)) {
                tracing::error!(
                    component = COMPONENT,
                    session_id = self.id,
                    "engineer slot write backlog full of Deletes; dropping new Put write"
                );
                return;
            } else {
                tracing::warn!(
                    component = COMPONENT,
                    session_id = self.id,
                    "engineer slot write backlog exceeded; retaining Delete write"
                );
            }
        }
        self.pending_writes.push(write);
    }

    /// Queues the store form of slot `index` (`dropped_at` set while its connection is down).
    fn queue_put(&mut self, index: usize, dropped_at: Option<DateTime<Utc>>) {
        if let Some(record) = self
            .engineers
            .get(index)
            .map(|s| s.record(self.id, dropped_at))
        {
            self.queue_write(SlotWrite::Put(record));
        }
    }

    /// Queues the deletion of a slot that ended.
    fn queue_delete(&mut self, slot: &EngineerSlot) {
        self.queue_write(SlotWrite::Delete {
            session_id: self.id,
            account: slot.engineer.account,
            connection_id: slot.connection_id,
        });
    }

    /// SESSION_INFO with a fresh ticket, then the join cache (REPLAYED), to a connected engineer
    /// slot. A dropped slot keeps its tickets (its engineer still holds one). `handover` says
    /// which earlier tickets the engineer may hold; they stay accepted within
    /// `previous_ticket_grace` in case this SESSION_INFO is lost
    /// ([`EngineerSlot::install_ticket`]).
    fn welcome_engineer(
        &mut self,
        index: usize,
        handover: TicketHandover,
        ctx: &Ctx<'_>,
        now: Instant,
    ) -> Result<(), RandomError> {
        let Some((conn, slot_ref)) = self.engineers.get(index).and_then(|slot| {
            slot.conn()
                .map(|conn| (Arc::clone(conn), slot.slot_ref(self.id)))
        }) else {
            return Ok(());
        };
        let (ticket, digest) = ctx.sealer.issue(slot_ref)?;
        let info = self.info(ResumeCredential::EngineerTicket(ticket));
        let replay = self.cache.replay();
        if let Some(slot) = self.engineers.get_mut(index) {
            slot.install_ticket(digest, handover, ctx.cfg.previous_ticket_grace, now);
        }
        if !conn.send(&Message::SessionInfo(info), self.id) && conn.outbox.stats().overflowed {
            ctx.metrics.slow_consumer_disconnects.inc();
        }
        for frame in replay {
            if conn
                .outbox
                .push_reliable(Outgoing::Forward {
                    frame,
                    replayed: true,
                })
                .is_err()
            {
                ctx.metrics.slow_consumer_disconnects.inc();
                conn.abort();
                return Ok(());
            }
        }
        // The engineer must know at once whether the stream can flow: a join or resume while the
        // driver is away shows "driver away", never a frozen stream that looks live (H1).
        if !conn.send(&Message::DriverPresence(self.presence(ctx, now)), self.id)
            && conn.outbox.stats().overflowed
        {
            ctx.metrics.slow_consumer_disconnects.inc();
        }
        Ok(())
    }

    /// Ends the session: engineers get SESSION_CLOSE{reason} and are disconnected. With
    /// `replaced`, an attached driver connection gets DISCONNECT{Replaced} and is closed (a newer
    /// session of the account exists); otherwise it stays open, unbound. Returns the digests that
    /// must answer `SessionClosed` from now on.
    fn close(&mut self, engineer_reason: SessionCloseReason, replaced: bool) -> Vec<TokenHash> {
        self.closed = true;
        let close = Message::SessionClose(SessionClose {
            session_id: self.id,
            reason: engineer_reason,
        });
        for slot in &self.engineers {
            if let Some(conn) = slot.conn() {
                conn.send(&close, self.id);
                conn.close();
            }
        }
        if replaced && let Some(conn) = self.driver_conn() {
            conn.send(
                &Message::Disconnect(Disconnect {
                    reason: DisconnectReason::Replaced,
                    message: Some("a newer session of this account replaced this one".into()),
                }),
                self.id,
            );
            conn.close();
        }
        self.engineers.clear();
        self.cache.clear();
        // Closing the session in the store deletes its slots; queued slot writes are moot.
        self.pending_writes.clear();
        let mut digests: Vec<TokenHash> = self.spent_codes.drain(..).collect();
        if let Some(code) = self.code.take() {
            digests.push(code.digest);
        }
        digests
    }
}

#[derive(Debug, Default)]
struct Tombstones {
    sessions: VecDeque<(u64, Instant)>,
    codes: VecDeque<(TokenHash, Instant)>,
}

impl Tombstones {
    fn add(&mut self, session: u64, codes: Vec<TokenHash>, now: Instant) {
        if self.sessions.len() >= MAX_TOMBSTONES {
            self.sessions.pop_front();
        }
        self.sessions.push_back((session, now));
        for digest in codes {
            if self.codes.len() >= MAX_TOMBSTONES {
                self.codes.pop_front();
            }
            self.codes.push_back((digest, now));
        }
    }

    fn expire(&mut self, now: Instant, ttl: Duration) {
        let alive = |at: &Instant| now.saturating_duration_since(*at) <= ttl;
        self.sessions.retain(|(_, at)| alive(at));
        self.codes.retain(|(_, at)| alive(at));
    }

    fn has_session(&self, id: u64) -> bool {
        self.sessions.iter().any(|(s, _)| *s == id)
    }

    fn has_code(&self, digest: &TokenHash) -> bool {
        // Scans every entry: constant time with respect to the position of a match.
        self.codes
            .iter()
            .fold(false, |found, (d, _)| found | d.ct_eq(digest))
    }
}

/// Kind of a code match found by a join scan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CodeMatch {
    Active,
    Spent,
}

/// The in-memory session registry.
#[derive(Debug)]
pub struct Registry {
    cfg: RegistryConfig,
    hasher: CodeHasher,
    sealer: TicketSealer,
    metrics: Arc<Metrics>,
    epoch: Instant,
    sessions: RwLock<HashMap<u64, Arc<SessionCell>>>,
    tombstones: Mutex<Tombstones>,
}

impl Registry {
    /// An empty registry. `sealer` issues and checks engineer resume tickets; `epoch` is the
    /// relay's monotonic epoch.
    pub fn new(
        cfg: RegistryConfig,
        hasher: CodeHasher,
        sealer: TicketSealer,
        metrics: Arc<Metrics>,
        epoch: Instant,
    ) -> Self {
        Self {
            cfg,
            hasher,
            sealer,
            metrics,
            epoch,
            sessions: RwLock::new(HashMap::new()),
            tombstones: Mutex::new(Tombstones::default()),
        }
    }

    fn ctx(&self) -> Ctx<'_> {
        Ctx {
            cfg: &self.cfg,
            hasher: &self.hasher,
            sealer: &self.sealer,
            metrics: &self.metrics,
            epoch: self.epoch,
        }
    }

    /// The settings.
    pub fn config(&self) -> &RegistryConfig {
        &self.cfg
    }

    /// Whether a new session would exceed the session limit.
    pub fn at_capacity(&self) -> bool {
        self.sessions.read().len() >= self.cfg.max_sessions
    }

    /// Whether `id` is in use (live or grace).
    pub fn contains(&self, id: u64) -> bool {
        self.sessions.read().contains_key(&id)
    }

    /// The session `id`, if it is in relay memory.
    pub fn cell(&self, id: u64) -> Option<Arc<SessionCell>> {
        self.sessions.read().get(&id).cloned()
    }

    fn cells(&self) -> Vec<Arc<SessionCell>> {
        self.sessions.read().values().cloned().collect()
    }

    /// Removes `ids` from the map and records their tombstones.
    fn bury(&self, closed: Vec<(u64, Vec<TokenHash>)>, now: Instant) {
        if closed.is_empty() {
            return;
        }
        {
            let mut map = self.sessions.write();
            for (id, _) in &closed {
                map.remove(id);
            }
        }
        let mut tombstones = self.tombstones.lock();
        for (id, codes) in closed {
            tombstones.add(id, codes, now);
        }
    }

    // ---------------------------------------------------------------------------------------
    // Driver
    // ---------------------------------------------------------------------------------------

    /// SESSION_CREATE accepted (the store record exists): registers the session with `conn` as
    /// its driver and sends SESSION_INFO, CONNECT_CODE (first code) and ENGINEER_LIST. Older
    /// sessions of the same driver account are closed (their engineers get
    /// SESSION_CLOSE{DriverStopped}).
    pub fn create_session(
        &self,
        new: NewSession,
        conn: &Arc<ConnHandle>,
        now: Instant,
    ) -> Result<Created, RegistryError> {
        let ctx = self.ctx();
        let cell = Arc::new(SessionCell {
            id: new.id,
            inner: Mutex::new(Session {
                id: new.id,
                driver: new.driver.clone(),
                meta: new.meta,
                started_at_ns: new.started_at_ns,
                resume_hash: new.resume_hash,
                driver_link: DriverLink::Attached(Arc::clone(conn)),
                engineers: Vec::new(),
                code: None,
                spent_codes: VecDeque::new(),
                cache: JoinCache::default(),
                driver_seq: 0,
                relay_seq: 0,
                next_connection_id: 1,
                rate: RateMeter::new(self.cfg.session_rate, now),
                closed: false,
                pending_writes: Vec::new(),
            }),
            store_order: tokio::sync::Mutex::new(()),
        });
        let mut replaced = Vec::new();
        {
            let mut map = self.sessions.write();
            let older: Vec<Arc<SessionCell>> = map
                .values()
                .filter(|c| {
                    let s = c.inner.lock();
                    !s.closed && s.driver.account == new.driver.account
                })
                .cloned()
                .collect();
            if map.len() - older.len() >= self.cfg.max_sessions {
                return Err(RegistryError::Full);
            }
            for old in older {
                let codes = old
                    .inner
                    .lock()
                    .close(SessionCloseReason::DriverStopped, true);
                map.remove(&old.id);
                replaced.push((old.id, codes));
            }
            map.insert(new.id, Arc::clone(&cell));
        }
        let replaced_ids = replaced.iter().map(|(id, _)| *id).collect();
        {
            let mut tombstones = self.tombstones.lock();
            for (id, codes) in replaced {
                tombstones.add(id, codes, now);
            }
        }
        {
            let mut s = cell.inner.lock();
            let info = s.info(ResumeCredential::DriverToken(new.resume_token));
            conn.send(&Message::SessionInfo(info), s.id);
            let code = s.fresh_code_or_none(&ctx, now);
            s.send_connect_code(code, &ctx);
            s.send_engineer_list(&ctx, now);
        }
        tracing::info!(
            component = COMPONENT,
            session_id = new.id,
            driver = %new.driver.username,
            conn = conn.id,
            "session created"
        );
        Ok(Created {
            cell,
            replaced: replaced_ids,
        })
    }

    /// SESSION_RESUME::Driver for a session in relay memory. The resume token is echoed back in
    /// SESSION_INFO (it is not rotated: a driver that misses this reply can still resume), and an
    /// unused, unexpired connection code is re-sent unchanged (same code, same expiry). With
    /// `new_sequence_space` the per-driver sequence state, the join cache and every pending live
    /// frame are dropped and each connected engineer gets SESSION_INFO before any frame of the new
    /// space (§6).
    pub fn resume_driver(
        &self,
        session_id: u64,
        account: Uuid,
        token: &Secret,
        new_sequence_space: bool,
        conn: &Arc<ConnHandle>,
        now: Instant,
    ) -> Result<Arc<SessionCell>, ResumeError> {
        let Some(digest) = presented_token_hash(token) else {
            return Err(ResumeError::NotAuthorized);
        };
        let cell = self.sessions.read().get(&session_id).cloned();
        let Some(cell) = cell else {
            if self.tombstones.lock().has_session(session_id) {
                return Err(ResumeError::Closed);
            }
            return Err(ResumeError::NotInMemory);
        };
        let ctx = self.ctx();
        let mut s = cell.inner.lock();
        if s.closed {
            return Err(ResumeError::Closed);
        }
        // Both checks always run (no early exit on the account).
        let token_ok = s.resume_hash.ct_eq(&digest);
        if !(token_ok && s.driver.account == account) {
            return Err(ResumeError::NotAuthorized);
        }
        self.attach_driver(&mut s, token, new_sequence_space, conn, &ctx, now);
        drop(s);
        tracing::info!(
            component = COMPONENT,
            session_id,
            conn = conn.id,
            new_sequence_space,
            "driver resumed"
        );
        Ok(cell)
    }

    /// A session rebuilt from its store record and engineer slots after a relay restart: the
    /// driver is away (since `driver_away_for` before `now`), every slot is `Reconnecting` with
    /// the drop time from the store (or, if the relay died while the engineer was connected, the
    /// time since this relay process started) and knows no ticket digest. New connection ids
    /// continue above the session's stored high-water mark, so an id of a slot that ended before
    /// the restart is never reused.
    fn rebuilt_session(
        &self,
        stored: &StoredSession,
        driver_away_for: Duration,
        now: Instant,
    ) -> Result<Arc<SessionCell>, ResumeError> {
        let record = &stored.record;
        let Some(resume_hash) = record.resume_hash else {
            return Err(ResumeError::Closed);
        };
        let now_ns = unix_now_ns();
        let since_relay_start = now.saturating_duration_since(self.epoch);
        let engineers: Vec<EngineerSlot> = stored
            .slots
            .iter()
            .filter(|slot| slot.record.session_id == record.id)
            .map(|slot| {
                let dropped_for = match slot.record.dropped_at {
                    Some(dropped_at) => {
                        Duration::from_nanos(now_ns.saturating_sub(datetime_to_ns(dropped_at)))
                    }
                    // Connected when the relay died: the connection ended with the old relay
                    // process, at an instant the store does not record. `last_activity` is no
                    // proxy for it (it lags by up to a touch interval and stops while the driver
                    // is away). The crash happened no later than this process started, so the
                    // time since then is a lower bound of the engineer's absence; the session
                    // itself is still bounded by the driver grace (checked by the caller).
                    None => since_relay_start.min(Duration::from_nanos(
                        now_ns.saturating_sub(datetime_to_ns(record.last_activity)),
                    )),
                };
                EngineerSlot {
                    connection_id: slot.record.connection_id,
                    engineer: slot.engineer.clone(),
                    connected_since_ns: datetime_to_ns(slot.record.connected_since),
                    ticket: None,
                    previous: [None; MAX_PREVIOUS_TICKETS],
                    link: SlotLink::Dropped {
                        since: now,
                        carried: dropped_for,
                    },
                    announced: EngineerLink::Reconnecting,
                    last_seq: 0,
                    snapshot_limiter: SnapshotRequestLimiter::new(duration_ms(
                        self.cfg.snapshot_request_min_interval,
                    )),
                }
            })
            .collect();
        // Continue above every id this session ever stored, not only the surviving slots: an
        // ended slot's id must never be handed out again (its tickets stay dead, and a stale
        // ENGINEER_KICK cannot hit a different engineer).
        let next_connection_id = engineers
            .iter()
            .map(|e| e.connection_id)
            .fold(record.max_connection_id, u64::max)
            .saturating_add(1);
        Ok(Arc::new(SessionCell {
            id: record.id,
            inner: Mutex::new(Session {
                id: record.id,
                driver: stored.driver.clone(),
                meta: record.meta.clone(),
                started_at_ns: datetime_to_ns(record.started_at),
                resume_hash,
                driver_link: DriverLink::Away {
                    since: now,
                    carried: driver_away_for,
                },
                engineers,
                code: None,
                spent_codes: VecDeque::new(),
                cache: JoinCache::default(),
                driver_seq: 0,
                relay_seq: 0,
                next_connection_id,
                rate: RateMeter::new(self.cfg.session_rate, now),
                closed: false,
                pending_writes: Vec::new(),
            }),
            store_order: tokio::sync::Mutex::new(()),
        }))
    }

    /// Inserts a rebuilt session. `Ok(false)` if the session is in memory already (rebuilt
    /// concurrently by its driver or another engineer): the caller resumes in memory instead.
    fn insert_rebuilt(&self, cell: &Arc<SessionCell>) -> Result<bool, ResumeError> {
        let mut map = self.sessions.write();
        if map.contains_key(&cell.id) {
            return Ok(false);
        }
        // Closed by this process meanwhile (its store write may still be in flight).
        if self.tombstones.lock().has_session(cell.id) {
            return Err(ResumeError::Closed);
        }
        if map.len() >= self.cfg.max_sessions {
            return Err(ResumeError::Registry(RegistryError::Full));
        }
        map.insert(cell.id, Arc::clone(cell));
        Ok(true)
    }

    /// SESSION_RESUME::Driver for a session found only in the store (relay restart): rebuilds it
    /// with its engineer slots (`Reconnecting` until each engineer resumes with its ticket) and
    /// attaches the driver. The caller verified account, resume digest, status and grace.
    pub fn restore_session(
        &self,
        stored: &StoredSession,
        token: &Secret,
        new_sequence_space: bool,
        conn: &Arc<ConnHandle>,
        now: Instant,
    ) -> Result<Arc<SessionCell>, ResumeError> {
        let session_id = stored.record.id;
        let cell = self.rebuilt_session(stored, Duration::ZERO, now)?;
        if !self.insert_rebuilt(&cell)? {
            return self.resume_driver(
                session_id,
                stored.driver.account,
                token,
                new_sequence_space,
                conn,
                now,
            );
        }
        let ctx = self.ctx();
        {
            let mut s = cell.inner.lock();
            self.attach_driver(&mut s, token, new_sequence_space, conn, &ctx, now);
        }
        tracing::info!(
            component = COMPONENT,
            session_id,
            conn = conn.id,
            engineer_slots = stored.slots.len(),
            "session restored from the store by its driver after a relay restart"
        );
        Ok(cell)
    }

    /// SESSION_RESUME::Engineer for a session found only in the store (relay restart, the driver
    /// has not resumed yet): the ticket must carry a valid tag for the stored slot of `account`;
    /// then the session is rebuilt with the driver away (for `driver_away_for` already) and the
    /// engineer resumes in it. Nothing changes if the ticket is refused. The caller verified the
    /// session's status and grace window.
    pub fn restore_for_engineer(
        &self,
        stored: &StoredSession,
        driver_away_for: Duration,
        ticket: &Secret,
        account: Uuid,
        conn: &Arc<ConnHandle>,
        now: Instant,
    ) -> Result<Arc<SessionCell>, ResumeError> {
        let session_id = stored.record.id;
        let ticket_ok = stored
            .slots
            .iter()
            .find(|s| s.record.session_id == session_id && s.record.account_id == account)
            .is_some_and(|slot| {
                self.sealer.verify(
                    ticket,
                    SlotRef {
                        session_id,
                        account,
                        connection_id: slot.record.connection_id,
                    },
                )
            });
        if !ticket_ok {
            return Err(ResumeError::NotAuthorized);
        }
        let cell = self.rebuilt_session(stored, driver_away_for, now)?;
        if self.insert_rebuilt(&cell)? {
            tracing::info!(
                component = COMPONENT,
                session_id,
                conn = conn.id,
                engineer_slots = stored.slots.len(),
                "session restored from the store by an engineer after a relay restart"
            );
        }
        self.resume_engineer(session_id, ticket, account, conn, now)
    }

    fn attach_driver(
        &self,
        s: &mut Session,
        token: &Secret,
        new_sequence_space: bool,
        conn: &Arc<ConnHandle>,
        ctx: &Ctx<'_>,
        now: Instant,
    ) {
        if let DriverLink::Attached(old) = &s.driver_link
            && old.id != conn.id
        {
            old.send(
                &Message::Disconnect(Disconnect {
                    reason: DisconnectReason::Replaced,
                    message: Some("the driver reconnected on a new connection".into()),
                }),
                s.id,
            );
            old.close();
        }
        let was_away = matches!(s.driver_link, DriverLink::Away { .. });
        s.driver_link = DriverLink::Attached(Arc::clone(conn));
        // A new sequence space welcomes every connected engineer again (SESSION_INFO, replay,
        // presence); otherwise the engineers are only told that the driver is back.
        if was_away && !new_sequence_space {
            s.announce_presence(ctx, now);
        }
        if new_sequence_space {
            s.driver_seq = 0;
            s.cache.clear();
            for index in 0..s.engineers.len() {
                if let Some(conn) = s.engineers[index].conn() {
                    let dropped = conn.outbox.clear_live(now);
                    ctx.metrics.live_frames_dropped.add(u64::from(dropped));
                }
                // The engineer holds the slot's current ticket (its SESSION_INFO went out on this
                // connection earlier), or an earlier one if that SESSION_INFO was lost: all of
                // them stay valid within their grace.
                if let Err(e) = s.welcome_engineer(index, TicketHandover::Reissue, ctx, now) {
                    tracing::error!(component = COMPONENT, session_id = s.id, error = %e, "engineer ticket not issued");
                }
            }
        }
        conn.send(
            &Message::SessionInfo(s.info(ResumeCredential::DriverToken(token.clone()))),
            s.id,
        );
        // The unused code stays valid across a driver resume: the driver may already have sent it
        // to an engineer. After a relay restart its plaintext is gone and a fresh code is issued.
        let code = s.current_code_or_fresh(ctx, now);
        s.send_connect_code(code, ctx);
        s.send_engineer_list(ctx, now);
        if s.engineers.iter().any(|e| e.conn().is_some()) {
            s.request_snapshot(SnapshotReason::Reconnect, ctx);
        }
    }

    /// The driver connection ended without closing the session: the grace period starts.
    /// Returns `false` if `conn_id` was no longer the session's driver (replaced meanwhile).
    pub fn driver_detached(&self, cell: &SessionCell, conn_id: u64, now: Instant) -> bool {
        let mut s = cell.inner.lock();
        if s.closed || !s.driver_is(conn_id) {
            return false;
        }
        s.driver_link = DriverLink::Away {
            since: now,
            carried: Duration::ZERO,
        };
        // Nothing new reaches the engineers until the driver resumes: they must not keep showing
        // the last values as a live stream (H1).
        s.announce_presence(&self.ctx(), now);
        tracing::info!(
            component = COMPONENT,
            session_id = s.id,
            conn = conn_id,
            "driver disconnected; grace period started"
        );
        true
    }

    /// SESSION_CLOSE from the driver: the session ends (engineers get
    /// SESSION_CLOSE{DriverStopped}); the driver connection stays open, unbound.
    pub fn close_by_driver(&self, cell: &SessionCell, conn_id: u64, now: Instant) -> bool {
        let codes = {
            let mut s = cell.inner.lock();
            if s.closed || !s.driver_is(conn_id) {
                return false;
            }
            s.close(SessionCloseReason::DriverStopped, false)
        };
        self.bury(vec![(cell.id, codes)], now);
        tracing::info!(
            component = COMPONENT,
            session_id = cell.id,
            "session closed by the driver"
        );
        true
    }

    /// CONNECT_CODE_REQUEST: a fresh code (the unused one is invalidated), none while full.
    pub fn request_code(&self, cell: &SessionCell, conn_id: u64, now: Instant) -> bool {
        let ctx = self.ctx();
        let mut s = cell.inner.lock();
        if s.closed || !s.driver_is(conn_id) {
            return false;
        }
        let code = s.fresh_code_or_none(&ctx, now);
        s.send_connect_code(code, &ctx);
        true
    }

    /// ENGINEER_KICK: that engineer gets SESSION_CLOSE{RemovedByDriver} and is disconnected, its
    /// ticket dies (in memory and, through the queued slot deletion, across relay restarts) and the
    /// slot is freed. Unknown ids are ignored (`false`).
    pub fn kick(&self, cell: &SessionCell, conn_id: u64, connection_id: u64, now: Instant) -> bool {
        let ctx = self.ctx();
        let mut s = cell.inner.lock();
        if s.closed || !s.driver_is(conn_id) {
            return false;
        }
        let Some(index) = s
            .engineers
            .iter()
            .position(|e| e.connection_id == connection_id)
        else {
            return false;
        };
        let slot = s.engineers.remove(index);
        s.queue_delete(&slot);
        if let Some(conn) = slot.conn() {
            conn.send(
                &Message::SessionClose(SessionClose {
                    session_id: s.id,
                    reason: SessionCloseReason::RemovedByDriver,
                }),
                s.id,
            );
            conn.close();
        }
        tracing::info!(
            component = COMPONENT,
            session_id = s.id,
            engineer = %slot.engineer.username,
            connection_id,
            "engineer removed by the driver"
        );
        s.on_slots_changed(CodeAfterSlotChange::Fresh, &ctx, now);
        true
    }

    // ---------------------------------------------------------------------------------------
    // Engineer
    // ---------------------------------------------------------------------------------------

    /// SESSION_JOIN{code} by an authenticated ENGINEER (role and rate limits are checked by the
    /// caller). On success the engineer gets SESSION_INFO (identity + ticket) and the replayed
    /// cache, the driver gets SNAPSHOT_REQUEST, CONNECT_CODE (fresh code if a slot is left) and
    /// ENGINEER_LIST.
    pub fn join(
        &self,
        code: &Secret,
        engineer: Identity,
        conn: &Arc<ConnHandle>,
        now: Instant,
    ) -> Result<Arc<SessionCell>, JoinError> {
        let Ok(canonical) = normalize_connect_code(code.expose()) else {
            return Err(JoinError::CodeInvalid);
        };
        let digest = self.hasher.digest(&canonical);
        // Scan every session (constant time with respect to which one matches).
        let mut found: Option<(Arc<SessionCell>, CodeMatch)> = None;
        for cell in self.cells() {
            let s = cell.inner.lock();
            let active = s.code.as_ref().is_some_and(|c| c.digest.ct_eq(&digest));
            let spent = s
                .spent_codes
                .iter()
                .fold(false, |hit, d| hit | d.ct_eq(&digest));
            drop(s);
            if active {
                found = Some((cell, CodeMatch::Active));
            } else if spent && found.is_none() {
                found = Some((cell, CodeMatch::Spent));
            }
        }
        let Some((cell, kind)) = found else {
            if self.tombstones.lock().has_code(&digest) {
                return Err(JoinError::SessionClosed);
            }
            return Err(JoinError::CodeInvalid);
        };
        let ctx = self.ctx();
        let mut s = cell.inner.lock();
        if s.closed {
            return Err(JoinError::SessionClosed);
        }
        // Re-check under the lock: the code may have been rotated or redeemed since the scan.
        let still_active = s.code.as_ref().is_some_and(|c| c.digest.ct_eq(&digest));
        if kind == CodeMatch::Spent || !still_active {
            return Err(if s.is_full(&self.cfg) {
                JoinError::SessionFull {
                    max: self.cfg.max_engineers,
                }
            } else {
                JoinError::CodeInvalid
            });
        }
        let expired = s.code.as_ref().is_some_and(|c| {
            now.saturating_duration_since(c.issued_at) >= self.cfg.connect_code_ttl
        });
        if expired {
            return Err(JoinError::CodeExpired);
        }
        let takeover = s
            .engineers
            .iter()
            .position(|e| e.engineer.account == engineer.account);
        if takeover.is_none() && s.is_full(&self.cfg) {
            return Err(JoinError::SessionFull {
                max: self.cfg.max_engineers,
            });
        }
        // Redeem: single use.
        s.retire_code();
        if let Some(index) = takeover {
            let old = s.engineers.remove(index);
            if let Some(old_conn) = old.conn() {
                old_conn.send(
                    &Message::Disconnect(Disconnect {
                        reason: DisconnectReason::Replaced,
                        message: Some("the same account joined again".into()),
                    }),
                    s.id,
                );
                old_conn.close();
            }
        }
        let connection_id = s.next_connection_id;
        s.next_connection_id = s.next_connection_id.saturating_add(1);
        s.engineers.push(EngineerSlot {
            connection_id,
            engineer: engineer.clone(),
            connected_since_ns: unix_now_ns(),
            // Set by welcome_engineer before anything is sent.
            ticket: None,
            previous: [None; MAX_PREVIOUS_TICKETS],
            link: SlotLink::Connected(Arc::clone(conn)),
            announced: EngineerLink::Healthy,
            last_seq: 0,
            snapshot_limiter: SnapshotRequestLimiter::new(duration_ms(
                self.cfg.snapshot_request_min_interval,
            )),
        });
        let index = s.engineers.len() - 1;
        if let Err(e) = s.welcome_engineer(index, TicketHandover::Join, &ctx, now) {
            s.engineers.pop();
            return Err(JoinError::Random(e));
        }
        // A takeover replaces the account's stored slot (same key).
        s.queue_put(index, None);
        s.request_snapshot(SnapshotReason::EngineerJoined, &ctx);
        s.on_slots_changed(CodeAfterSlotChange::Fresh, &ctx, now);
        let session_id = s.id;
        drop(s);
        tracing::info!(
            component = COMPONENT,
            session_id,
            engineer = %engineer.username,
            connection_id,
            conn = conn.id,
            takeover = takeover.is_some(),
            "engineer joined with a connection code"
        );
        Ok(cell)
    }

    /// SESSION_RESUME::Engineer{session_id, ticket} after a network blip. The ticket must belong
    /// to the slot the authenticated `account` holds in the session: the slot's current ticket,
    /// or one of its earlier tickets still within `previous_ticket_grace`
    /// (`EngineerSlot::ticket_matches`, D-022 clarification: a lost SESSION_OK must not strand
    /// the engineer); for a slot restored after a relay restart, any ticket carrying a valid tag
    /// for it. A dropped slot must be inside the ticket window. `NotInMemory` sends the caller
    /// to the store (relay restart).
    pub fn resume_engineer(
        &self,
        session_id: u64,
        ticket: &Secret,
        account: Uuid,
        conn: &Arc<ConnHandle>,
        now: Instant,
    ) -> Result<Arc<SessionCell>, ResumeError> {
        let Some(digest) = presented_token_hash(ticket) else {
            return Err(ResumeError::NotAuthorized);
        };
        let cell = self.sessions.read().get(&session_id).cloned();
        let Some(cell) = cell else {
            if self.tombstones.lock().has_session(session_id) {
                return Err(ResumeError::Closed);
            }
            return Err(ResumeError::NotInMemory);
        };
        let ctx = self.ctx();
        let mut s = cell.inner.lock();
        if s.closed {
            return Err(ResumeError::Closed);
        }
        // An account holds at most one slot per session (a second join takes it over).
        let Some(index) = s
            .engineers
            .iter()
            .position(|e| e.engineer.account == account)
        else {
            return Err(ResumeError::NotAuthorized);
        };
        if !s.engineers[index].ticket_matches(
            session_id,
            ticket,
            &digest,
            &self.sealer,
            self.cfg.previous_ticket_grace,
            now,
        ) {
            return Err(ResumeError::NotAuthorized);
        }
        // Past the window the ticket is dead; housekeeping frees the slot (and its store row).
        if let SlotLink::Dropped { since, carried } = s.engineers[index].link
            && elapsed_since(since, carried, now) >= self.cfg.engineer_ticket_ttl
        {
            return Err(ResumeError::NotAuthorized);
        }
        if let SlotLink::Connected(old) = &s.engineers[index].link
            && old.id != conn.id
        {
            old.send(
                &Message::Disconnect(Disconnect {
                    reason: DisconnectReason::Replaced,
                    message: Some("the engineer reconnected on a new connection".into()),
                }),
                s.id,
            );
            old.close();
        }
        let was_connected = s.engineers[index].conn().is_some();
        {
            let slot = &mut s.engineers[index];
            slot.link = SlotLink::Connected(Arc::clone(conn));
            slot.last_seq = 0;
        }
        // The presented ticket (current or previous) becomes the previous one of the fresh ticket:
        // if this SESSION_INFO is lost too, the engineer can present it again within the grace.
        s.welcome_engineer(index, TicketHandover::Resume(digest), &ctx, now)
            .map_err(|e| ResumeError::Registry(RegistryError::Random(e)))?;
        if !was_connected {
            s.queue_put(index, None);
        }
        s.request_snapshot(SnapshotReason::Reconnect, &ctx);
        s.send_engineer_list(&ctx, now);
        let connection_id = s.engineers[index].connection_id;
        drop(s);
        tracing::info!(
            component = COMPONENT,
            session_id,
            connection_id,
            conn = conn.id,
            "engineer resumed with its ticket"
        );
        Ok(cell)
    }

    /// The engineer connection dropped without DISCONNECT: its slot is held (`Reconnecting`) for
    /// the ticket lifetime.
    pub fn engineer_detached(&self, cell: &SessionCell, conn_id: u64, now: Instant) {
        let ctx = self.ctx();
        let mut s = cell.inner.lock();
        if s.closed {
            return;
        }
        let Some(index) = s
            .engineers
            .iter()
            .position(|e| e.conn().is_some_and(|c| c.id == conn_id))
        else {
            return;
        };
        s.engineers[index].link = SlotLink::Dropped {
            since: now,
            carried: Duration::ZERO,
        };
        let connection_id = s.engineers[index].connection_id;
        // The ticket window starts now, also for a relay restarted meanwhile.
        s.queue_put(index, Some(ns_to_datetime(unix_now_ns())));
        s.send_engineer_list(&ctx, now);
        tracing::info!(
            component = COMPONENT,
            session_id = s.id,
            connection_id,
            "engineer connection lost; slot held for its ticket"
        );
    }

    /// The engineer sent DISCONNECT: the slot is freed and its ticket dies.
    pub fn engineer_left(&self, cell: &SessionCell, conn_id: u64, now: Instant) {
        let ctx = self.ctx();
        let mut s = cell.inner.lock();
        if s.closed {
            return;
        }
        let Some(index) = s
            .engineers
            .iter()
            .position(|e| e.conn().is_some_and(|c| c.id == conn_id))
        else {
            return;
        };
        let slot = s.engineers.remove(index);
        s.queue_delete(&slot);
        tracing::info!(
            component = COMPONENT,
            session_id = s.id,
            engineer = %slot.engineer.username,
            connection_id = slot.connection_id,
            "engineer disconnected"
        );
        s.on_slots_changed(CodeAfterSlotChange::Keep, &ctx, now);
    }

    /// Whether `conn_id` is a connected engineer of the session.
    pub fn is_engineer(&self, cell: &SessionCell, conn_id: u64) -> bool {
        let s = cell.inner.lock();
        !s.closed
            && s.engineers
                .iter()
                .any(|e| e.conn().is_some_and(|c| c.id == conn_id))
    }

    /// Whether `conn_id` is the attached driver of the session.
    pub fn is_driver(&self, cell: &SessionCell, conn_id: u64) -> bool {
        let s = cell.inner.lock();
        !s.closed && s.driver_is(conn_id)
    }

    // ---------------------------------------------------------------------------------------
    // Hot path
    // ---------------------------------------------------------------------------------------

    /// Fans a validated driver stream frame out to every connected engineer of the session
    /// (latest-wins slot or reliable queue by type) and updates the join cache. The payload is
    /// copied once and never decoded.
    pub fn forward_driver_frame(
        &self,
        cell: &SessionCell,
        conn_id: u64,
        frame: &Frame,
        msg_type: MsgType,
        rx_ns: u64,
        now: Instant,
    ) -> Result<FanOut, ForwardReject> {
        let sequence = frame.header().sequence;
        // Copy the payload before taking the lock (the only allocation of the fan-out).
        let forwarded = Arc::new(Forwarded::new(frame, msg_type, rx_ns, now));
        let mut s = cell.inner.lock();
        if s.closed || !s.driver_is(conn_id) {
            return Err(ForwardReject::NotBound);
        }
        if sequence <= s.driver_seq {
            return Err(ForwardReject::StaleSequence {
                last: s.driver_seq,
                got: sequence,
            });
        }
        if !s.rate.admit(frame.wire_len(), now) {
            return Err(ForwardReject::RateLimited);
        }
        s.driver_seq = sequence;
        s.cache.store(&forwarded, now);
        let live = msg_type.delivery() == protocol::frame::Delivery::LatestWins;
        let mut out = FanOut::default();
        for slot in &s.engineers {
            let Some(conn) = slot.conn() else {
                continue;
            };
            let pushed = if live {
                conn.outbox
                    .push_live(Arc::clone(&forwarded), now)
                    .map(|p| out.dropped = out.dropped.saturating_add(p.dropped))
            } else {
                conn.outbox.push_reliable(Outgoing::Forward {
                    frame: Arc::clone(&forwarded),
                    replayed: false,
                })
            };
            match pushed {
                Ok(()) => out.receivers += 1,
                Err(PushError::Overflow) => {
                    self.metrics.slow_consumer_disconnects.inc();
                    tracing::warn!(
                        component = COMPONENT,
                        session_id = s.id,
                        connection_id = slot.connection_id,
                        "engineer cannot keep up with reliable frames; disconnecting it"
                    );
                    conn.abort();
                }
                Err(PushError::Closed) => {}
            }
        }
        Ok(out)
    }

    /// An engineer's SNAPSHOT_REQUEST (the only engineer→driver type), already decoded by the
    /// caller from a frame with header `header`: forwarded at most once per engineer per
    /// `SNAPSHOT_REQUEST_MIN_INTERVAL_MS`.
    ///
    /// The driver never receives engineer bytes: the request is re-encoded by the relay (canonical
    /// payload at this build's minor, the engineer's sequence and timestamp), so bytes a newer-minor
    /// sender appended are not forwarded. Engineer requests are metered by the engineer's own
    /// connection meter and request limiter only, never by the session meter, which measures the
    /// driver's stream alone (engineers cannot starve the driver's telemetry).
    pub fn forward_snapshot_request(
        &self,
        cell: &SessionCell,
        conn_id: u64,
        request: SnapshotRequest,
        header: &FrameHeader,
        now: Instant,
    ) -> Result<SnapshotRequestOutcome, ForwardReject> {
        let sequence = header.sequence;
        let meta = FrameMeta::stream(
            FramePriority::Control,
            cell.id(),
            sequence,
            header.source_ts_ns,
        );
        let frame = match Message::SnapshotRequest(request).to_frame(&meta) {
            Ok(frame) => frame,
            Err(e) => {
                tracing::error!(component = COMPONENT, session_id = cell.id(), error = %e, "snapshot request not re-encoded");
                return Ok(SnapshotRequestOutcome::NotEncoded);
            }
        };
        let mut s = cell.inner.lock();
        if s.closed {
            return Err(ForwardReject::NotBound);
        }
        let Some(index) = s
            .engineers
            .iter()
            .position(|e| e.conn().is_some_and(|c| c.id == conn_id))
        else {
            return Err(ForwardReject::NotBound);
        };
        let last = s.engineers[index].last_seq;
        if sequence <= last {
            return Err(ForwardReject::StaleSequence {
                last,
                got: sequence,
            });
        }
        let now_ms = duration_ms(now.saturating_duration_since(self.epoch));
        let slot = &mut s.engineers[index];
        slot.last_seq = sequence;
        if !slot.snapshot_limiter.permit(now_ms) {
            return Ok(SnapshotRequestOutcome::Coalesced);
        }
        let Some(driver) = s.driver_conn() else {
            return Ok(SnapshotRequestOutcome::DriverAway);
        };
        match driver.outbox.push_reliable(Outgoing::Frame(frame)) {
            Ok(()) => Ok(SnapshotRequestOutcome::Forwarded),
            Err(_) => Ok(SnapshotRequestOutcome::DriverAway),
        }
    }

    // ---------------------------------------------------------------------------------------
    // Housekeeping, shutdown, diagnostics
    // ---------------------------------------------------------------------------------------

    /// Periodic pass: driver grace expiry, engineer ticket expiry, code expiry, link health
    /// changes, join-cache aging and tombstone expiry.
    pub fn housekeeping(&self, now: Instant) -> Housekeeping {
        let ctx = self.ctx();
        let mut expired = Vec::new();
        let mut slot_writes = Vec::new();
        for cell in self.cells() {
            let mut s = cell.inner.lock();
            if s.closed {
                continue;
            }
            if let DriverLink::Away { since, carried } = s.driver_link
                && elapsed_since(since, carried, now) >= self.cfg.driver_grace
            {
                let codes = s.close(SessionCloseReason::Expired, false);
                expired.push((s.id, codes));
                tracing::info!(
                    component = COMPONENT,
                    session_id = s.id,
                    "driver grace period elapsed; session closed"
                );
                continue;
            }
            let ttl = self.cfg.engineer_ticket_ttl;
            let (kept, ended): (Vec<EngineerSlot>, Vec<EngineerSlot>) =
                std::mem::take(&mut s.engineers)
                    .into_iter()
                    .partition(|slot| match slot.link {
                        SlotLink::Dropped { since, carried } => {
                            elapsed_since(since, carried, now) < ttl
                        }
                        SlotLink::Connected(_) => true,
                    });
            s.engineers = kept;
            let slots_changed = !ended.is_empty();
            if slots_changed {
                for slot in &ended {
                    s.queue_delete(slot);
                }
                tracing::info!(
                    component = COMPONENT,
                    session_id = s.id,
                    removed = ended.len(),
                    "engineer resume tickets expired"
                );
            }
            let code_expired = s.code.as_mut().is_some_and(|c| {
                let expired =
                    now.saturating_duration_since(c.issued_at) >= self.cfg.connect_code_ttl;
                if expired {
                    // The digest stays (CodeExpired answers); the plaintext is wiped.
                    c.plaintext = None;
                }
                let due = expired && !c.expiry_announced;
                if due {
                    c.expiry_announced = true;
                }
                due
            });
            s.cache.expire(now, self.cfg.join_cache_max_age);
            if slots_changed {
                s.on_slots_changed(CodeAfterSlotChange::Keep, &ctx, now);
            } else {
                if code_expired {
                    s.send_connect_code(None, &ctx);
                }
                let links_changed = s
                    .engineers
                    .iter()
                    .any(|slot| s.link_of(slot, &ctx, now) != slot.announced);
                if links_changed {
                    s.send_engineer_list(&ctx, now);
                }
            }
            let has_writes = !s.pending_writes.is_empty();
            drop(s);
            if has_writes {
                slot_writes.push(cell);
            }
        }
        let ids = expired.iter().map(|(id, _)| *id).collect();
        self.bury(expired, now);
        self.tombstones.lock().expire(now, self.cfg.tombstone_ttl());
        Housekeeping {
            expired: ids,
            slot_writes,
        }
    }

    /// Relay shutdown: engineers get SESSION_CLOSE{RelayShutdown} (they may try to resume),
    /// drivers DISCONNECT{ServerShutdown}; every connection flushes and closes. Sessions stay
    /// open in the store so drivers resume after the restart. Code plaintexts are wiped.
    pub fn shutdown(&self) {
        for cell in self.cells() {
            let mut s = cell.inner.lock();
            if let Some(code) = s.code.as_mut() {
                code.plaintext = None;
            }
            let close = Message::SessionClose(SessionClose {
                session_id: s.id,
                reason: SessionCloseReason::RelayShutdown,
            });
            for slot in &s.engineers {
                if let Some(conn) = slot.conn() {
                    conn.send(&close, s.id);
                    conn.close();
                }
            }
            if let Some(conn) = s.driver_conn() {
                conn.send(
                    &Message::Disconnect(Disconnect {
                        reason: DisconnectReason::ServerShutdown,
                        message: Some("the relay is restarting".into()),
                    }),
                    s.id,
                );
                conn.close();
            }
        }
    }

    /// Ids of sessions whose driver is attached (store `last_activity` updates).
    pub fn attached_session_ids(&self) -> Vec<u64> {
        self.cells()
            .into_iter()
            .filter(|c| {
                let s = c.inner.lock();
                !s.closed && s.driver_conn().is_some()
            })
            .map(|c| c.id)
            .collect()
    }

    /// Point-in-time gauges.
    pub fn gauges(&self) -> RegistryGauges {
        let mut g = RegistryGauges::default();
        for cell in self.cells() {
            let s = cell.inner.lock();
            if s.closed {
                continue;
            }
            g.active_sessions += 1;
            if s.driver_conn().is_none() {
                g.sessions_in_grace += 1;
            }
            g.engineers_connected +=
                s.engineers.iter().filter(|e| e.conn().is_some()).count() as u64;
        }
        g
    }

    /// Diagnostics view of one session.
    pub fn session_debug(&self, id: u64, now: Instant) -> Option<SessionDebug> {
        let cell = self.sessions.read().get(&id).cloned()?;
        let ctx = self.ctx();
        let s = cell.inner.lock();
        Some(SessionDebug {
            id,
            driver_attached: s.driver_conn().is_some(),
            driver_outbox: s.driver_conn().map(|c| c.outbox.stats()),
            engineers: s
                .engineers
                .iter()
                .map(|slot| EngineerDebug {
                    connection_id: slot.connection_id,
                    username: slot.engineer.username.clone(),
                    connected: slot.conn().is_some(),
                    link: s.link_of(slot, &ctx, now),
                    outbox: slot.conn().map(|c| c.outbox.stats()),
                })
                .collect(),
            code_active: s.code.as_ref().is_some_and(|c| {
                now.saturating_duration_since(c.issued_at) < self.cfg.connect_code_ttl
            }),
            spent_codes: s.spent_codes.len(),
            cached_frames: s.cache.len(),
            last_driver_sequence: s.driver_seq,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};

    use protocol::frame::FrameHeader;
    use protocol::messages::{CONNECT_CODE_LEN, ConnectCode};

    use super::*;
    use crate::outbox::Batch;
    use crate::secrets::new_token;
    use crate::store::SessionStatus;

    /// Key of the test ticket sealer (a relay "restart" keeps it, like `JWT_KEY_FILE`).
    const TICKET_KEY: [u8; 32] = [9u8; 32];

    const MAX_FRAMES: usize = 256;
    const MAX_BYTES: usize = 1 << 20;

    struct World {
        registry: Registry,
        epoch: Instant,
        next_conn: u64,
    }

    impl World {
        fn new() -> Self {
            Self::with(|_| {})
        }

        fn with(tweak: impl FnOnce(&mut RegistryConfig)) -> Self {
            let mut cfg = RegistryConfig::from_relay(&RelayConfig::development());
            tweak(&mut cfg);
            let epoch = Instant::now();
            Self {
                registry: Registry::new(
                    cfg,
                    CodeHasher::new().expect("rng"),
                    TicketSealer::new(&TICKET_KEY).expect("key"),
                    Arc::new(Metrics::default()),
                    epoch,
                ),
                epoch,
                next_conn: 1,
            }
        }

        fn conn(&mut self) -> Arc<ConnHandle> {
            let id = self.next_conn;
            self.next_conn += 1;
            Arc::new(ConnHandle::new(
                id,
                SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 40_000),
                self.epoch,
                MAX_FRAMES,
                MAX_BYTES,
            ))
        }

        fn identity(name: &str) -> Identity {
            Identity {
                account: Uuid::new_v4(),
                username: name.to_owned(),
                display_name: name.to_uppercase(),
            }
        }

        /// Creates a session; returns (cell, driver conn, resume token, first code).
        fn session(&mut self, now: Instant) -> (Arc<SessionCell>, Arc<ConnHandle>, Secret, Secret) {
            let driver = self.conn();
            let (token, hash) = new_token().expect("rng");
            let created = self
                .registry
                .create_session(
                    NewSession {
                        id: 7,
                        driver: Self::identity("dave"),
                        meta: SessionMeta {
                            car: Some("499P".into()),
                            schema_version: 1,
                            ..SessionMeta::default()
                        },
                        started_at_ns: 1,
                        resume_token: token.clone(),
                        resume_hash: hash,
                    },
                    &driver,
                    now,
                )
                .expect("created");
            let msgs = drain(&driver);
            assert!(matches!(msgs[0], Message::SessionInfo(_)));
            let code = last_code(&msgs).expect("first code issued");
            (created.cell, driver, token, code)
        }
    }

    fn drain(conn: &ConnHandle) -> Vec<Message> {
        let Batch { items, .. } = conn.outbox.take_batch(usize::MAX);
        items
            .into_iter()
            .filter_map(|item| match item {
                Outgoing::Frame(f) => Message::from_frame(&f).ok(),
                Outgoing::Forward { .. } | Outgoing::Pong { .. } => None,
            })
            .collect()
    }

    fn last_code(msgs: &[Message]) -> Option<Secret> {
        msgs.iter().rev().find_map(|m| match m {
            Message::ConnectCode(ConnectCode { code, .. }) => {
                Some(code.as_ref().map(|c| c.code.clone()))
            }
            _ => None,
        })?
    }

    fn last_connect_code(msgs: &[Message]) -> Option<ConnectCode> {
        msgs.iter().rev().find_map(|m| match m {
            Message::ConnectCode(c) => Some(c.clone()),
            _ => None,
        })
    }

    fn ticket_of(msgs: &[Message]) -> Secret {
        msgs.iter()
            .find_map(|m| match m {
                Message::SessionInfo(SessionInfo {
                    resume: ResumeCredential::EngineerTicket(t),
                    ..
                }) => Some(t.clone()),
                _ => None,
            })
            .expect("engineer ticket")
    }

    #[test]
    fn codes_are_single_use_and_rotate_after_redemption() {
        let mut w = World::new();
        let now = w.epoch;
        let (_cell, driver, _, code) = w.session(now);
        assert_eq!(code.expose().len(), CONNECT_CODE_LEN);
        let erin = w.conn();
        w.registry
            .join(&code, World::identity("erin"), &erin, now)
            .expect("joined");
        let msgs = drain(&erin);
        assert!(matches!(msgs[0], Message::SessionInfo(_)));
        let driver_msgs = drain(&driver);
        let cc = last_connect_code(&driver_msgs).expect("CONNECT_CODE");
        assert_eq!(cc.engineers_connected, 1);
        let fresh = cc.code.expect("fresh code while a slot is free").code;
        assert_ne!(fresh, code);
        // The used code is spent: a free slot remains, so CodeInvalid.
        let eddie = w.conn();
        assert!(matches!(
            w.registry
                .join(&code, World::identity("eddie"), &eddie, now),
            Err(JoinError::CodeInvalid)
        ));
        // Display form and look-alikes are accepted.
        let display = format!("{}-{}", &fresh.expose()[..4], &fresh.expose()[4..]).to_lowercase();
        w.registry
            .join(&Secret::new(display), World::identity("eddie"), &eddie, now)
            .expect("second engineer");
        let cc = last_connect_code(&drain(&driver)).expect("CONNECT_CODE");
        assert_eq!((cc.engineers_connected, cc.engineers_max), (2, 2));
        assert!(cc.code.is_none(), "no code while every slot is taken");
        // Any old code now answers SessionFull.
        let emma = w.conn();
        let err = w
            .registry
            .join(&fresh, World::identity("emma"), &emma, now)
            .expect_err("full");
        assert!(matches!(err, JoinError::SessionFull { max: 2 }));
        assert_eq!(
            err.to_error_msg().message,
            "Driver already has 2 engineers connected"
        );
        // A guessed code is invalid.
        assert!(matches!(
            w.registry.join(
                &Secret::new("ZZZZZZZZ"),
                World::identity("emma"),
                &emma,
                now
            ),
            Err(JoinError::CodeInvalid)
        ));
        assert!(matches!(
            w.registry.join(
                &Secret::new("not a code"),
                World::identity("emma"),
                &emma,
                now
            ),
            Err(JoinError::CodeInvalid)
        ));
    }

    #[test]
    fn codes_expire_and_rotation_invalidates_the_old_code() {
        let mut w = World::new();
        let t0 = w.epoch;
        let (cell, driver, _, code) = w.session(t0);
        // Rotation (New code): the old one is invalid at once.
        assert!(w.registry.request_code(&cell, driver.id, t0));
        let rotated = last_code(&drain(&driver)).expect("rotated");
        assert_ne!(rotated, code);
        let erin = w.conn();
        assert!(matches!(
            w.registry.join(&code, World::identity("erin"), &erin, t0),
            Err(JoinError::CodeInvalid)
        ));
        // Expiry after CONNECT_CODE_TTL_S (600 s): CodeExpired, and the driver is told once.
        let later = t0 + Duration::from_secs(600);
        w.registry.housekeeping(later);
        let cc = last_connect_code(&drain(&driver)).expect("expiry announced");
        assert!(cc.code.is_none());
        w.registry.housekeeping(later + Duration::from_secs(1));
        assert!(
            last_connect_code(&drain(&driver)).is_none(),
            "announced once"
        );
        assert!(matches!(
            w.registry
                .join(&rotated, World::identity("erin"), &erin, later),
            Err(JoinError::CodeExpired)
        ));
        // Just before expiry it works.
        assert!(w.registry.request_code(&cell, driver.id, later));
        let fresh = last_code(&drain(&driver)).expect("fresh");
        w.registry
            .join(
                &fresh,
                World::identity("erin"),
                &erin,
                later + Duration::from_secs(599),
            )
            .expect("valid until the TTL");
    }

    /// Plaintext of the session's unused code still held in relay memory.
    fn held_code(cell: &SessionCell) -> Option<Secret> {
        cell.inner
            .lock()
            .code
            .as_ref()
            .and_then(|c| c.plaintext.as_ref().map(CodePlaintext::to_secret))
    }

    /// Driver connection drops and resumes on a new connection; returns its messages.
    fn driver_blip(
        w: &mut World,
        cell: &SessionCell,
        driver: &mut Arc<ConnHandle>,
        token: &Secret,
        now: Instant,
    ) -> Vec<Message> {
        let account = cell.inner.lock().driver.account;
        assert!(w.registry.driver_detached(cell, driver.id, now));
        *driver = w.conn();
        w.registry
            .resume_driver(7, account, token, false, driver, now)
            .expect("driver resumed");
        drain(driver)
    }

    #[test]
    fn a_resuming_driver_is_shown_the_same_unused_code() {
        let mut w = World::new();
        let t0 = w.epoch;
        let ttl = w.registry.config().connect_code_ttl;
        let (cell, mut driver, token, _) = w.session(t0);
        // The first CONNECT_CODE is gone with `session()`: rotate once to see the expiry too.
        assert!(w.registry.request_code(&cell, driver.id, t0));
        let issued = last_connect_code(&drain(&driver))
            .and_then(|c| c.code)
            .expect("code");
        assert_eq!(held_code(&cell).as_ref(), Some(&issued.code));
        assert!(
            !format!("{cell:?}").contains(issued.code.expose()),
            "the held plaintext never reaches a log"
        );
        // A driver blip: the same code with its original expiry, still single use.
        let t1 = t0 + Duration::from_secs(30);
        let again = last_connect_code(&driver_blip(&mut w, &cell, &mut driver, &token, t1))
            .and_then(|c| c.code)
            .expect("code re-sent");
        assert_eq!(again, issued);
        let erin = w.conn();
        w.registry
            .join(&issued.code, World::identity("erin"), &erin, t1)
            .expect("the code the driver already shared still works");
        let eddie = w.conn();
        assert!(matches!(
            w.registry
                .join(&issued.code, World::identity("eddie"), &eddie, t1),
            Err(JoinError::CodeInvalid)
        ));
        // Redeemed: its plaintext is gone, the driver holds the fresh code issued after the join.
        let fresh = last_code(&drain(&driver)).expect("fresh code after the join");
        assert_ne!(fresh, issued.code);
        assert_eq!(held_code(&cell).as_ref(), Some(&fresh));
        // Rotation: a resume shows the rotated code, not an older one.
        assert!(w.registry.request_code(&cell, driver.id, t1));
        let rotated = last_code(&drain(&driver)).expect("rotated");
        let shown = last_code(&driver_blip(&mut w, &cell, &mut driver, &token, t1)).expect("code");
        assert_eq!(shown, rotated);
        // Expiry: the plaintext is wiped and a resume gets a fresh code with a new expiry.
        let expired_at = t1 + ttl;
        w.registry.housekeeping(expired_at);
        assert!(held_code(&cell).is_none());
        let after = last_connect_code(&driver_blip(&mut w, &cell, &mut driver, &token, expired_at))
            .and_then(|c| c.code)
            .expect("fresh code");
        assert_ne!(after.code, rotated);
        // An expired code that housekeeping has not seen yet is not re-sent either.
        let late = expired_at + ttl;
        let renewed =
            last_code(&driver_blip(&mut w, &cell, &mut driver, &token, late)).expect("code");
        assert_ne!(renewed, after.code);
        // Every slot taken: no code is held or shown.
        let eddie = w.conn();
        w.registry
            .join(&renewed, World::identity("eddie"), &eddie, late)
            .expect("second engineer");
        assert!(held_code(&cell).is_none());
        let full = last_connect_code(&driver_blip(&mut w, &cell, &mut driver, &token, late))
            .expect("CONNECT_CODE");
        assert!(full.code.is_none());
    }

    #[test]
    fn shutdown_and_session_end_drop_the_held_code() {
        let mut w = World::new();
        let t0 = w.epoch;
        let (cell, mut driver, token, code) = w.session(t0);
        assert_eq!(held_code(&cell), Some(code.clone()));
        w.registry.shutdown();
        assert!(held_code(&cell).is_none(), "wiped on shutdown");
        // Without the plaintext a resume issues a fresh code (as after a restart).
        let shown = last_code(&driver_blip(&mut w, &cell, &mut driver, &token, t0)).expect("code");
        assert_ne!(shown, code);
        assert_eq!(held_code(&cell), Some(shown));
        assert!(w.registry.close_by_driver(&cell, driver.id, t0));
        assert!(cell.inner.lock().code.is_none(), "dropped at session end");
    }

    #[test]
    fn after_a_relay_restart_the_resuming_driver_gets_a_fresh_code() {
        let mut old = World::new();
        let t0 = old.epoch;
        let (cell, _driver, token, code) = old.session(t0);
        let stored = stored_session(&cell, wall_now());
        let mut new = World::new();
        let now = new.epoch;
        let driver2 = new.conn();
        let cell2 = new
            .registry
            .restore_session(&stored, &token, false, &driver2, now)
            .expect("restored");
        let fresh = last_code(&drain(&driver2)).expect("a code");
        assert_ne!(fresh, code, "the old plaintext did not survive the restart");
        assert_eq!(held_code(&cell2), Some(fresh));
        let erin = new.conn();
        assert!(matches!(
            new.registry
                .join(&code, World::identity("erin"), &erin, now),
            Err(JoinError::CodeInvalid)
        ));
    }

    #[test]
    fn kick_frees_the_slot_and_kills_the_ticket() {
        let mut w = World::new();
        let now = w.epoch;
        let (cell, driver, _, code) = w.session(now);
        let erin_identity = World::identity("erin");
        let erin = w.conn();
        w.registry
            .join(&code, erin_identity.clone(), &erin, now)
            .expect("joined");
        let ticket = ticket_of(&drain(&erin));
        let list = drain(&driver)
            .into_iter()
            .rev()
            .find_map(|m| match m {
                Message::EngineerList(l) => Some(l),
                _ => None,
            })
            .expect("ENGINEER_LIST");
        assert_eq!(list.engineers.len(), 1);
        let connection_id = list.engineers[0].connection_id;
        assert!(
            !w.registry.kick(&cell, driver.id, connection_id + 100, now),
            "unknown id ignored"
        );
        assert!(w.registry.kick(&cell, driver.id, connection_id, now));
        let msgs = drain(&erin);
        assert!(msgs.iter().any(|m| matches!(
            m,
            Message::SessionClose(SessionClose {
                reason: SessionCloseReason::RemovedByDriver,
                ..
            })
        )));
        assert!(erin.outbox.is_closing());
        let cc = last_connect_code(&drain(&driver)).expect("slot freed");
        assert_eq!(cc.engineers_connected, 0);
        assert!(cc.code.is_some());
        let again = w.conn();
        assert!(matches!(
            w.registry
                .resume_engineer(7, &ticket, erin_identity.account, &again, now),
            Err(ResumeError::NotAuthorized)
        ));
    }

    #[test]
    fn ticket_resume_works_within_the_window_only() {
        let mut w = World::new();
        let t0 = w.epoch;
        let (cell, driver, _, code) = w.session(t0);
        let erin_identity = World::identity("erin");
        let erin = w.conn();
        w.registry
            .join(&code, erin_identity.clone(), &erin, t0)
            .expect("joined");
        let ticket = ticket_of(&drain(&erin));
        w.registry.engineer_detached(&cell, erin.id, t0);
        let debug = w.registry.session_debug(7, t0).expect("session");
        assert_eq!(debug.engineers[0].link, EngineerLink::Reconnecting);
        // Wrong account: refused even with the right ticket.
        let thief = w.conn();
        assert!(matches!(
            w.registry
                .resume_engineer(7, &ticket, Uuid::new_v4(), &thief, t0),
            Err(ResumeError::NotAuthorized)
        ));
        // Within ENGINEER_TICKET_TTL_S: resumed, with a new ticket.
        let back = w.conn();
        let t1 = t0 + Duration::from_secs(599);
        w.registry
            .resume_engineer(7, &ticket, erin_identity.account, &back, t1)
            .expect("resumed");
        let new_ticket = ticket_of(&drain(&back));
        assert_ne!(new_ticket, ticket);
        let after_grace = t1 + w.registry.config().previous_ticket_grace;
        assert!(
            matches!(
                w.registry
                    .resume_engineer(7, &ticket, erin_identity.account, &back, after_grace),
                Err(ResumeError::NotAuthorized)
            ),
            "the old ticket dies once the previous-ticket grace is over"
        );
        // Dropped again, then the window passes: the slot is freed by housekeeping.
        w.registry.engineer_detached(&cell, back.id, t1);
        let t2 = t1 + Duration::from_secs(600);
        w.registry.housekeeping(t2);
        let cc = last_connect_code(&drain(&driver)).expect("slot freed");
        assert_eq!(cc.engineers_connected, 0);
        let late = w.conn();
        assert!(matches!(
            w.registry
                .resume_engineer(7, &new_ticket, erin_identity.account, &late, t2),
            Err(ResumeError::NotAuthorized)
        ));
    }

    #[test]
    fn explicit_disconnect_kills_the_ticket() {
        let mut w = World::new();
        let now = w.epoch;
        let (cell, _driver, _, code) = w.session(now);
        let erin_identity = World::identity("erin");
        let erin = w.conn();
        w.registry
            .join(&code, erin_identity.clone(), &erin, now)
            .expect("joined");
        let ticket = ticket_of(&drain(&erin));
        cell.take_slot_writes();
        w.registry.engineer_left(&cell, erin.id, now);
        // The store row goes too, so the ticket stays dead across a relay restart.
        assert!(matches!(
            cell.take_slot_writes().as_slice(),
            [SlotWrite::Delete { account, .. }] if *account == erin_identity.account
        ));
        let again = w.conn();
        assert!(matches!(
            w.registry
                .resume_engineer(7, &ticket, erin_identity.account, &again, now),
            Err(ResumeError::NotAuthorized)
        ));
    }

    /// An engineer of session 7 holding two live tickets: joined with `code`, dropped and resumed
    /// (the resume's SESSION_INFO arrived). Returns (account, connection, joined ticket = the
    /// previous one, current ticket).
    fn engineer_with_two_tickets(
        w: &mut World,
        cell: &SessionCell,
        code: &Secret,
        name: &str,
        now: Instant,
    ) -> (Uuid, Arc<ConnHandle>, Secret, Secret) {
        let identity = World::identity(name);
        let account = identity.account;
        let first = w.conn();
        w.registry
            .join(code, identity, &first, now)
            .expect("joined");
        let joined = ticket_of(&drain(&first));
        w.registry.engineer_detached(cell, first.id, now);
        let conn = w.conn();
        w.registry
            .resume_engineer(7, &joined, account, &conn, now)
            .expect("resumed");
        let current = ticket_of(&drain(&conn));
        (account, conn, joined, current)
    }

    fn refused(w: &mut World, ticket: &Secret, account: Uuid, now: Instant) -> bool {
        let probe = w.conn();
        w.registry
            .resume_engineer(7, ticket, account, &probe, now)
            .is_err()
    }

    #[test]
    fn a_resume_reply_lost_in_a_second_blip_is_retried_with_the_previous_ticket() {
        let mut w = World::new();
        let t0 = w.epoch;
        let grace = w.registry.config().previous_ticket_grace;
        let just_inside = grace - Duration::from_millis(1);
        let (cell, _driver, _, code) = w.session(t0);
        let erin_identity = World::identity("erin");
        let account = erin_identity.account;
        let erin = w.conn();
        w.registry
            .join(&code, erin_identity, &erin, t0)
            .expect("joined");
        let joined = ticket_of(&drain(&erin));
        // Blip 1: the resume is accepted, but its SESSION_INFO is lost in blip 2.
        w.registry.engineer_detached(&cell, erin.id, t0);
        let lost_link = w.conn();
        w.registry
            .resume_engineer(7, &joined, account, &lost_link, t0)
            .expect("resumed");
        let lost = ticket_of(&drain(&lost_link));
        w.registry.engineer_detached(&cell, lost_link.id, t0);
        // The engineer still holds the joined ticket: honoured within the grace, with a fresh one.
        let t1 = t0 + just_inside;
        let back = w.conn();
        w.registry
            .resume_engineer(7, &joined, account, &back, t1)
            .expect("the previous ticket works within the grace");
        let fresh = ticket_of(&drain(&back));
        assert!(fresh != joined && fresh != lost);
        assert!(!back.outbox.is_closing());
        // The never-delivered ticket is dead: the engineer showed it holds the older one.
        w.registry.engineer_detached(&cell, back.id, t1);
        assert!(refused(&mut w, &lost, account, t1));
        // Further lost replies keep working: the presented ticket is the previous one of each
        // fresh ticket.
        let t2 = t1 + just_inside;
        let again = w.conn();
        w.registry
            .resume_engineer(7, &joined, account, &again, t2)
            .expect("a third blip with the same ticket");
        let newest = ticket_of(&drain(&again));
        assert!(refused(&mut w, &fresh, account, t2));
        // Once the newest ticket is used, the joined one is dead even inside the grace.
        w.registry.engineer_detached(&cell, again.id, t2);
        let used = w.conn();
        w.registry
            .resume_engineer(7, &newest, account, &used, t2)
            .expect("newest ticket");
        drain(&used);
        assert!(refused(&mut w, &joined, account, t2));
        // Every accepted resume kept the slot (no second slot, no new connection id).
        assert_eq!(
            w.registry
                .session_debug(7, t2)
                .expect("session")
                .engineers
                .len(),
            1
        );
    }

    #[test]
    fn the_previous_ticket_dies_after_the_grace_or_once_the_newer_one_is_used() {
        let mut w = World::new();
        let t0 = w.epoch;
        let grace = w.registry.config().previous_ticket_grace;
        let (cell, driver, _, code) = w.session(t0);
        // After the grace: the previous ticket is refused, the current one still works.
        let (erin, conn, joined, current) =
            engineer_with_two_tickets(&mut w, &cell, &code, "erin", t0);
        w.registry.engineer_detached(&cell, conn.id, t0);
        let after_grace = t0 + grace;
        assert!(refused(&mut w, &joined, erin, after_grace));
        let back = w.conn();
        w.registry
            .resume_engineer(7, &current, erin, &back, after_grace)
            .expect("the current ticket lives for the whole ticket window");
        drain(&back);
        // Newer used: a second engineer resumes with its current ticket inside the grace; its
        // previous ticket is dead at once.
        let second = last_code(&drain(&driver)).expect("second code");
        let (eddie, conn, joined, current) =
            engineer_with_two_tickets(&mut w, &cell, &second, "eddie", t0);
        w.registry.engineer_detached(&cell, conn.id, t0);
        let soon = t0 + Duration::from_secs(1);
        let back = w.conn();
        w.registry
            .resume_engineer(7, &current, eddie, &back, soon)
            .expect("current ticket");
        drain(&back);
        assert!(refused(&mut w, &joined, eddie, soon));
    }

    #[test]
    fn kick_disconnect_expiry_and_session_end_kill_both_tickets() {
        const TTL: Duration = Duration::from_secs(5);
        let mut w = World::with(|c| c.engineer_ticket_ttl = TTL);
        let t0 = w.epoch;
        assert!(TTL < w.registry.config().previous_ticket_grace);
        let (cell, driver, _, code) = w.session(t0);
        let ids = |w: &World| -> Vec<u64> {
            w.registry
                .session_debug(7, t0)
                .map(|d| d.engineers.iter().map(|e| e.connection_id).collect())
                .unwrap_or_default()
        };
        // Kick.
        let (erin, _conn, previous, current) =
            engineer_with_two_tickets(&mut w, &cell, &code, "erin", t0);
        let kicked = ids(&w)[0];
        assert!(w.registry.kick(&cell, driver.id, kicked, t0));
        assert!(refused(&mut w, &previous, erin, t0));
        assert!(refused(&mut w, &current, erin, t0));
        // DISCONNECT.
        let code = last_code(&drain(&driver)).expect("code after the kick");
        let (eddie, conn, previous, current) =
            engineer_with_two_tickets(&mut w, &cell, &code, "eddie", t0);
        w.registry.engineer_left(&cell, conn.id, t0);
        assert!(refused(&mut w, &previous, eddie, t0));
        assert!(refused(&mut w, &current, eddie, t0));
        // Ticket expiry (inside the previous ticket's grace).
        let code = last_code(&drain(&driver)).expect("code after the disconnect");
        let (emma, conn, previous, current) =
            engineer_with_two_tickets(&mut w, &cell, &code, "emma", t0);
        w.registry.engineer_detached(&cell, conn.id, t0);
        w.registry.housekeeping(t0 + TTL);
        assert!(ids(&w).is_empty(), "the slot expired");
        assert!(refused(&mut w, &previous, emma, t0 + TTL));
        assert!(refused(&mut w, &current, emma, t0 + TTL));
        // Session end.
        let code = last_code(&drain(&driver)).expect("code after the expiry");
        let (fred, _conn, previous, current) =
            engineer_with_two_tickets(&mut w, &cell, &code, "fred", t0);
        assert!(w.registry.close_by_driver(&cell, driver.id, t0));
        for ticket in [&previous, &current] {
            let probe = w.conn();
            assert!(matches!(
                w.registry.resume_engineer(7, ticket, fred, &probe, t0),
                Err(ResumeError::Closed)
            ));
        }
    }

    #[test]
    fn a_driver_restart_notice_keeps_the_ticket_the_engineer_held() {
        let mut w = World::new();
        let t0 = w.epoch;
        let (cell, driver, token, code) = w.session(t0);
        let erin_identity = World::identity("erin");
        let account = erin_identity.account;
        let erin = w.conn();
        w.registry
            .join(&code, erin_identity, &erin, t0)
            .expect("joined");
        let joined = ticket_of(&drain(&erin));
        let driver_account = cell.inner.lock().driver.account;
        assert!(w.registry.driver_detached(&cell, driver.id, t0));
        let restarted = w.conn();
        w.registry
            .resume_driver(7, driver_account, &token, true, &restarted, t0)
            .expect("driver resumed");
        // The restart notice carries a new ticket, but the link drops before it arrives.
        let notice = ticket_of(&drain(&erin));
        assert_ne!(notice, joined);
        w.registry.engineer_detached(&cell, erin.id, t0);
        let back = w.conn();
        w.registry
            .resume_engineer(7, &joined, account, &back, t0 + Duration::from_secs(1))
            .expect("the ticket held before the notice works within the grace");
    }

    /// The SESSION_INFO of a resume and the driver-restart notice sent right after it are both
    /// lost on one half-open link. Returns (account, presented ticket, lost resume ticket, lost
    /// notice ticket, when the notice was sent); the engineer's connection is down.
    fn lost_resume_reply_then_lost_restart_notice(
        w: &mut World,
        t0: Instant,
    ) -> (Uuid, Secret, Secret, Secret, Instant) {
        let (cell, driver, token, code) = w.session(t0);
        let erin_identity = World::identity("erin");
        let account = erin_identity.account;
        let erin = w.conn();
        w.registry
            .join(&code, erin_identity, &erin, t0)
            .expect("joined");
        let joined = ticket_of(&drain(&erin));
        w.registry.engineer_detached(&cell, erin.id, t0);
        let half_open = w.conn();
        w.registry
            .resume_engineer(7, &joined, account, &half_open, t0)
            .expect("resumed");
        let lost_reply = ticket_of(&drain(&half_open));
        let t1 = t0 + Duration::from_secs(5);
        let driver_account = cell.inner.lock().driver.account;
        assert!(w.registry.driver_detached(&cell, driver.id, t1));
        let restarted = w.conn();
        w.registry
            .resume_driver(7, driver_account, &token, true, &restarted, t1)
            .expect("driver resumed");
        let lost_notice = ticket_of(&drain(&half_open));
        assert!(lost_notice != joined && lost_notice != lost_reply);
        w.registry.engineer_detached(&cell, half_open.id, t1);
        (account, joined, lost_reply, lost_notice, t1)
    }

    /// Review finding F5 (relay2): a driver-restart notice no longer replaces an earlier ticket
    /// that is still inside its grace, so the ticket the engineer actually holds keeps working.
    #[test]
    fn a_restart_notice_after_a_lost_resume_reply_keeps_the_presented_ticket() {
        let mut w = World::new();
        let t0 = w.epoch;
        let grace = w.registry.config().previous_ticket_grace;
        let (account, joined, lost_reply, lost_notice, _) =
            lost_resume_reply_then_lost_restart_notice(&mut w, t0);
        let back = w.conn();
        let t2 = t0 + grace - Duration::from_millis(1);
        w.registry
            .resume_engineer(7, &joined, account, &back, t2)
            .expect("the presented ticket survives the restart notice");
        drain(&back);
        // That resume shows which ticket the engineer holds: the undelivered ones are dead.
        let cell = w.registry.cell(7).expect("session");
        w.registry.engineer_detached(&cell, back.id, t2);
        for dead in [&lost_reply, &lost_notice] {
            assert!(refused(&mut w, dead, account, t2));
        }
    }

    #[test]
    fn each_earlier_ticket_keeps_its_own_grace() {
        let mut w = World::new();
        let t0 = w.epoch;
        let grace = w.registry.config().previous_ticket_grace;
        let (account, joined, lost_reply, _, t1) =
            lost_resume_reply_then_lost_restart_notice(&mut w, t0);
        // The joined ticket's grace ran from the resume (t0), the lost reply's from the notice.
        let late = t0 + grace;
        assert!(late < t1 + grace);
        assert!(refused(&mut w, &joined, account, late));
        let back = w.conn();
        w.registry
            .resume_engineer(7, &lost_reply, account, &back, late)
            .expect("the ticket replaced by the notice is still inside its grace");
    }

    /// Review finding F7 (relay2): a slot released by DISCONNECT or ticket expiry keeps the
    /// driver's unused code (it may have been shared already); a kick replaces it.
    #[test]
    fn a_released_slot_keeps_the_unused_code_and_a_kick_replaces_it() {
        const TTL: Duration = Duration::from_secs(5);
        let mut w = World::with(|c| c.engineer_ticket_ttl = TTL);
        let t0 = w.epoch;
        let (cell, driver, _, code) = w.session(t0);
        let erin = w.conn();
        w.registry
            .join(&code, World::identity("erin"), &erin, t0)
            .expect("erin joined");
        drain(&erin);
        let shared = last_code(&drain(&driver)).expect("code for the second slot");
        // DISCONNECT: the same code is shown again and still works.
        w.registry.engineer_left(&cell, erin.id, t0);
        assert_eq!(last_code(&drain(&driver)).as_ref(), Some(&shared));
        let eddie = w.conn();
        w.registry
            .join(&shared, World::identity("eddie"), &eddie, t0)
            .expect("the shared code survived the DISCONNECT");
        drain(&eddie);
        let shared = last_code(&drain(&driver)).expect("fresh code after the redemption");
        // Ticket expiry: likewise.
        w.registry.engineer_detached(&cell, eddie.id, t0);
        w.registry.housekeeping(t0 + TTL);
        assert_eq!(last_code(&drain(&driver)).as_ref(), Some(&shared));
        let emma = w.conn();
        w.registry
            .join(&shared, World::identity("emma"), &emma, t0 + TTL)
            .expect("the shared code survived the expiry");
        drain(&emma);
        let unused = last_code(&drain(&driver)).expect("fresh code after the redemption");
        // Kick: the code may have leaked, so a fresh one replaces it.
        let kicked = w
            .registry
            .session_debug(7, t0 + TTL)
            .expect("session")
            .engineers[0]
            .connection_id;
        assert!(w.registry.kick(&cell, driver.id, kicked, t0 + TTL));
        let fresh = last_code(&drain(&driver)).expect("fresh code after the kick");
        assert_ne!(fresh, unused);
        let probe = w.conn();
        assert!(matches!(
            w.registry
                .join(&unused, World::identity("fred"), &probe, t0 + TTL),
            Err(JoinError::CodeInvalid)
        ));
    }

    #[test]
    fn slot_tickets_are_recognised_by_their_tag_after_a_relay_restart() {
        let mut old = World::new();
        let t0 = old.epoch;
        let (cell, _driver, _, code) = old.session(t0);
        let (erin, conn, previous, current) =
            engineer_with_two_tickets(&mut old, &cell, &code, "erin", t0);
        old.registry.engineer_detached(&cell, conn.id, t0);
        let stored = stored_session(&cell, wall_now());
        // Either ticket resumes after a restart (the tag is checked, no digest is stored).
        for ticket in [&previous, &current] {
            let mut new = World::new();
            let now = new.epoch;
            let link = new.conn();
            new.registry
                .restore_for_engineer(&stored, Duration::ZERO, ticket, erin, &link, now)
                .expect("resumed after the restart");
            assert_ne!(&ticket_of(&drain(&link)), ticket);
        }
        // And the grace works in the new process: the reply to the restored resume is lost, the
        // same ticket is presented again.
        let mut new = World::new();
        let now = new.epoch;
        let lost_link = new.conn();
        let cell2 = new
            .registry
            .restore_for_engineer(&stored, Duration::ZERO, &previous, erin, &lost_link, now)
            .expect("resumed");
        drain(&lost_link);
        new.registry.engineer_detached(&cell2, lost_link.id, now);
        let back = new.conn();
        new.registry
            .resume_engineer(7, &previous, erin, &back, now + Duration::from_secs(1))
            .expect("previous ticket by digest after the restart");

        // Review finding F6 (relay2), documented in PROTOCOL.md §6: the tag cannot tell the
        // tickets of one slot apart, so until the first resume after a restart every ticket
        // issued for the slot verifies, also one the old process already treated as dead. The
        // first resume ends that: only the presented ticket (within the grace) and the fresh one
        // work from then on.
        let back = old.conn();
        old.registry
            .resume_engineer(7, &current, erin, &back, t0)
            .expect("newest ticket used");
        let newest = ticket_of(&drain(&back));
        assert!(
            refused(&mut old, &previous, erin, t0),
            "dead in the old process"
        );
        old.registry.engineer_detached(&cell, back.id, t0);
        let stored = stored_session(&cell, wall_now());
        let mut new = World::new();
        let now = new.epoch;
        let link = new.conn();
        let cell3 = new
            .registry
            .restore_for_engineer(&stored, Duration::ZERO, &previous, erin, &link, now)
            .expect("any ticket of the slot verifies by its tag until the first resume");
        drain(&link);
        new.registry.engineer_detached(&cell3, link.id, now);
        for other in [&current, &newest] {
            assert!(refused(&mut new, other, erin, now));
        }
    }

    /// H1 (silent-failure review): engineers are told when the driver's connection drops, when it
    /// comes back, and on a join or resume while it is away; a stream that stopped never looks live.
    #[test]
    fn engineers_learn_when_the_driver_is_away_and_when_it_is_back() {
        let mut w = World::new();
        let now = w.epoch;
        let (cell, driver, token, code) = w.session(now);
        let erin = w.conn();
        w.registry
            .join(&code, World::identity("erin"), &erin, now)
            .expect("joined");
        let joined = drain(&erin);
        assert!(
            matches!(
                joined.last(),
                Some(Message::DriverPresence(p)) if p.present && p.session_id == 7
            ),
            "a join while the driver is attached says so: {joined:?}"
        );
        let next_code = last_code(&drain(&driver)).expect("a fresh code for the second slot");
        // The driver's PC goes offline: the session enters its grace period.
        let away_at = now + Duration::from_secs(5);
        assert!(w.registry.driver_detached(&cell, driver.id, away_at));
        let grace_ns = duration_ns(w.registry.config().driver_grace);
        match drain(&erin).as_slice() {
            [Message::DriverPresence(p)] => {
                assert!(!p.present);
                assert_eq!(p.session_id, 7);
                assert!(p.away_since_ns > 0);
                assert_eq!(p.grace_expires_at_ns, p.away_since_ns + grace_ns);
            }
            other => panic!("driver away not announced: {other:?}"),
        }
        // A second engineer redeems the code the driver shared before it went away: the join
        // works (the code is consumed), and it is told at once that the driver is away.
        let fay = w.conn();
        w.registry
            .join(
                &next_code,
                World::identity("fay"),
                &fay,
                away_at + Duration::from_secs(60),
            )
            .expect("a code is valid while the driver is away");
        let fay_msgs = drain(&fay);
        assert!(matches!(fay_msgs.first(), Some(Message::SessionInfo(_))));
        match fay_msgs.last() {
            Some(Message::DriverPresence(p)) => {
                assert!(!p.present);
                // Away for 60 s already (wall clock, so allow scheduling slack).
                let away_for = unix_now_ns().saturating_sub(p.away_since_ns);
                assert!(away_for >= 59_000_000_000, "away for {away_for} ns");
            }
            other => panic!("join while away not announced: {other:?}"),
        }
        // The driver resumes within its grace (same sequence space): both engineers learn it.
        let account = cell.inner.lock().driver.account;
        let back = w.conn();
        w.registry
            .resume_driver(
                7,
                account,
                &token,
                false,
                &back,
                away_at + Duration::from_secs(90),
            )
            .expect("resumed");
        for engineer in [&erin, &fay] {
            assert!(matches!(
                drain(engineer).as_slice(),
                [Message::DriverPresence(DriverPresence {
                    present: true,
                    ..
                })]
            ));
        }
    }

    #[test]
    fn driver_restart_keeps_engineers_and_resets_their_stream() {
        let mut w = World::new();
        let now = w.epoch;
        let (cell, driver, token, code) = w.session(now);
        let erin = w.conn();
        let erin_identity = World::identity("erin");
        w.registry
            .join(&code, erin_identity, &erin, now)
            .expect("joined");
        drain(&erin);
        let account = cell.inner.lock().driver.account;
        assert!(w.registry.driver_detached(&cell, driver.id, now));
        assert!(matches!(
            drain(&erin).as_slice(),
            [Message::DriverPresence(DriverPresence {
                present: false,
                ..
            })]
        ));
        let restarted = w.conn();
        w.registry
            .resume_driver(7, account, &token, true, &restarted, now)
            .expect("resumed");
        let msgs = drain(&erin);
        assert!(
            matches!(msgs.first(), Some(Message::SessionInfo(_))),
            "engineer reset first"
        );
        assert!(
            matches!(
                msgs.last(),
                Some(Message::DriverPresence(DriverPresence {
                    present: true,
                    ..
                }))
            ),
            "then told the driver is back: {msgs:?}"
        );
        assert!(!erin.outbox.is_closing(), "engineer stays connected");
        let driver_msgs = drain(&restarted);
        assert!(matches!(
            driver_msgs.first(),
            Some(Message::SessionInfo(SessionInfo { resume: ResumeCredential::DriverToken(t), .. })) if *t == token
        ));
        // A wrong token or account is refused.
        let other = w.conn();
        assert!(matches!(
            w.registry
                .resume_driver(7, account, &Secret::new("wrong"), false, &other, now),
            Err(ResumeError::NotAuthorized)
        ));
        assert!(matches!(
            w.registry
                .resume_driver(7, Uuid::new_v4(), &token, false, &other, now),
            Err(ResumeError::NotAuthorized)
        ));
        assert!(matches!(
            w.registry
                .resume_driver(8, account, &token, false, &other, now),
            Err(ResumeError::NotInMemory)
        ));
    }

    #[test]
    fn grace_expiry_closes_the_session() {
        let mut w = World::new();
        let t0 = w.epoch;
        let (cell, driver, token, code) = w.session(t0);
        let erin = w.conn();
        w.registry
            .join(&code, World::identity("erin"), &erin, t0)
            .expect("joined");
        drain(&erin);
        assert!(w.registry.driver_detached(&cell, driver.id, t0));
        assert!(
            w.registry
                .housekeeping(t0 + Duration::from_secs(599))
                .expired
                .is_empty()
        );
        let report = w.registry.housekeeping(t0 + Duration::from_secs(600));
        assert_eq!(report.expired, vec![7]);
        assert!(drain(&erin).iter().any(|m| matches!(
            m,
            Message::SessionClose(SessionClose {
                reason: SessionCloseReason::Expired,
                ..
            })
        )));
        assert!(cell.is_closed());
        let account = cell.inner.lock().driver.account;
        let back = w.conn();
        assert!(matches!(
            w.registry
                .resume_driver(7, account, &token, false, &back, t0),
            Err(ResumeError::Closed)
        ));
        let eddie = w.conn();
        assert!(matches!(
            w.registry.join(&code, World::identity("eddie"), &eddie, t0),
            Err(JoinError::SessionClosed)
        ));
    }

    #[test]
    fn forwarding_is_isolated_ordered_and_sequence_checked() {
        let mut w = World::new();
        let now = w.epoch;
        let (cell_a, driver_a, _, code_a) = w.session(now);
        let erin = w.conn();
        w.registry
            .join(&code_a, World::identity("erin"), &erin, now)
            .expect("joined");
        drain(&erin);
        let frame = |seq: u64| {
            Frame::build(
                MsgType::TelemetryDelta,
                &FrameMeta::stream(FramePriority::P0, 7, seq, seq),
                &[0u8; 40],
            )
            .expect("frame")
        };
        let out = w
            .registry
            .forward_driver_frame(
                &cell_a,
                driver_a.id,
                &frame(1),
                MsgType::TelemetryDelta,
                1,
                now,
            )
            .expect("forwarded");
        assert_eq!(out.receivers, 1);
        assert_eq!(
            w.registry.forward_driver_frame(
                &cell_a,
                driver_a.id,
                &frame(1),
                MsgType::TelemetryDelta,
                1,
                now
            ),
            Err(ForwardReject::StaleSequence { last: 1, got: 1 })
        );
        // Another connection (not the attached driver) cannot inject frames.
        let intruder = w.conn();
        assert_eq!(
            w.registry.forward_driver_frame(
                &cell_a,
                intruder.id,
                &frame(2),
                MsgType::TelemetryDelta,
                1,
                now
            ),
            Err(ForwardReject::NotBound)
        );
        let batch = erin.outbox.take_batch(usize::MAX);
        assert_eq!(batch.items.len(), 1);
        if let Outgoing::Forward { frame, replayed } = &batch.items[0] {
            assert!(!replayed);
            let mut buf = Vec::new();
            frame.encode_into(&mut buf, &protocol::RelayTrailer::default());
            let header = FrameHeader::parse(&buf).expect("header");
            assert_eq!(header.sequence, 1);
        } else {
            panic!("expected a forwarded frame");
        }
    }

    #[test]
    fn join_replays_the_cache_in_sequence_order() {
        let mut w = World::new();
        let now = w.epoch;
        let (cell, driver, _, code) = w.session(now);
        let send = |seq: u64, t: MsgType, p: FramePriority| {
            let frame =
                Frame::build(t, &FrameMeta::stream(p, 7, seq, seq), &[1u8; 8]).expect("frame");
            w.registry
                .forward_driver_frame(&cell, driver.id, &frame, t, 1, now)
                .expect("forwarded");
        };
        send(1, MsgType::SchemaNegotiate, FramePriority::Control);
        send(2, MsgType::TelemetrySnapshot, FramePriority::P0);
        send(3, MsgType::TelemetryDelta, FramePriority::P0);
        send(4, MsgType::ScoringUpdate, FramePriority::P0);
        send(5, MsgType::TelemetrySnapshot, FramePriority::P0);
        let erin = w.conn();
        w.registry
            .join(&code, World::identity("erin"), &erin, now)
            .expect("joined");
        let batch = erin.outbox.take_batch(usize::MAX);
        assert!(
            matches!(batch.items[0], Outgoing::Frame(_)),
            "SESSION_INFO first"
        );
        let replayed: Vec<(MsgType, u64, bool)> = batch.items[1..]
            .iter()
            .filter_map(|i| match i {
                Outgoing::Forward { frame, replayed } => {
                    Some((frame.msg_type, frame.sequence, *replayed))
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            replayed,
            vec![
                (MsgType::SchemaNegotiate, 1, true),
                (MsgType::ScoringUpdate, 4, true),
                (MsgType::TelemetrySnapshot, 5, true),
            ]
        );
        // The driver was asked for a fresh snapshot.
        let to_driver = driver.outbox.take_batch(usize::MAX);
        let requested = to_driver.items.iter().any(|i| match i {
            Outgoing::Frame(f) => f.msg_type() == Some(MsgType::SnapshotRequest),
            _ => false,
        });
        assert!(requested);
        // Snapshot and scoring age out of the cache; the schema stays.
        w.registry.housekeeping(now + Duration::from_secs(31));
        assert_eq!(
            w.registry
                .session_debug(7, now)
                .expect("session")
                .cached_frames,
            1
        );
    }

    #[test]
    fn engineer_snapshot_requests_are_rate_limited_and_re_encoded() {
        let mut w = World::new();
        let now = w.epoch;
        let (cell, driver, _, code) = w.session(now);
        let erin = w.conn();
        w.registry
            .join(&code, World::identity("erin"), &erin, now)
            .expect("joined");
        driver.outbox.take_batch(usize::MAX);
        let stranger = w.conn();
        let request = SnapshotRequest {
            reason: SnapshotReason::SequenceGap,
        };
        // The header of a newer-minor sender that appended 5 bytes: none of it reaches the driver.
        let header = |seq: u64| FrameHeader {
            minor: protocol::PROTOCOL_MINOR + 1,
            msg_type: MsgType::SnapshotRequest.id(),
            flags: protocol::frame::Flags::empty(),
            priority: FramePriority::Control,
            payload_len: 6,
            session_id: 7,
            sequence: seq,
            source_ts_ns: 42,
        };
        let forward = |conn_id: u64, seq: u64, at: Instant| {
            w.registry
                .forward_snapshot_request(&cell, conn_id, request, &header(seq), at)
        };
        assert_eq!(
            forward(erin.id, 1, now),
            Ok(SnapshotRequestOutcome::Forwarded)
        );
        assert_eq!(
            forward(erin.id, 2, now + Duration::from_millis(100)),
            Ok(SnapshotRequestOutcome::Coalesced)
        );
        assert_eq!(
            forward(erin.id, 3, now + Duration::from_millis(300)),
            Ok(SnapshotRequestOutcome::Forwarded)
        );
        assert_eq!(
            forward(erin.id, 3, now),
            Err(ForwardReject::StaleSequence { last: 3, got: 3 })
        );
        assert_eq!(forward(stranger.id, 9, now), Err(ForwardReject::NotBound));
        let batch = driver.outbox.take_batch(usize::MAX);
        assert_eq!(batch.items.len(), 2);
        let canonical = Message::SnapshotRequest(request)
            .to_frame(&FrameMeta::stream(FramePriority::Control, 7, 1, 42))
            .expect("frame");
        match &batch.items[0] {
            Outgoing::Frame(f) => {
                assert_eq!(f.as_bytes(), canonical.as_bytes(), "canonical re-encoding");
                assert_eq!(f.header().minor, protocol::PROTOCOL_MINOR);
            }
            _ => panic!("a SNAPSHOT_REQUEST frame expected"),
        }
    }

    #[test]
    fn engineer_snapshot_requests_never_drain_the_session_meter() {
        let mut w = World::new();
        let now = w.epoch;
        let (cell, driver, _, code) = w.session(now);
        let erin = w.conn();
        w.registry
            .join(&code, World::identity("erin"), &erin, now)
            .expect("joined");
        let request = SnapshotRequest {
            reason: SnapshotReason::Manual,
        };
        // Far more requests than the session meter's burst, all at the same instant.
        let burst = w.registry.cfg.session_rate.packets_per_sec
            * w.registry.cfg.session_rate.burst_secs.max(1);
        for seq in 1..=burst + 10 {
            let header = FrameHeader {
                minor: protocol::PROTOCOL_MINOR,
                msg_type: MsgType::SnapshotRequest.id(),
                flags: protocol::frame::Flags::empty(),
                priority: FramePriority::Control,
                payload_len: 1,
                session_id: 7,
                sequence: seq,
                source_ts_ns: 0,
            };
            let outcome = w
                .registry
                .forward_snapshot_request(&cell, erin.id, request, &header, now);
            assert!(
                outcome.is_ok(),
                "engineer requests are never session-limited"
            );
        }
        // The driver's telemetry still has the whole session budget.
        let delta = Frame::build(
            MsgType::TelemetryDelta,
            &FrameMeta::stream(FramePriority::P0, 7, 1, 0),
            b"delta",
        )
        .expect("frame");
        assert!(
            w.registry
                .forward_driver_frame(&cell, driver.id, &delta, MsgType::TelemetryDelta, 0, now)
                .is_ok()
        );
    }

    #[test]
    fn a_new_session_of_the_same_driver_replaces_the_old_one() {
        let mut w = World::new();
        let now = w.epoch;
        let (cell, driver, _, code) = w.session(now);
        let erin = w.conn();
        w.registry
            .join(&code, World::identity("erin"), &erin, now)
            .expect("joined");
        drain(&erin);
        let account = cell.inner.lock().driver.account;
        let second = w.conn();
        let (token, hash) = new_token().expect("rng");
        let created = w
            .registry
            .create_session(
                NewSession {
                    id: 8,
                    driver: Identity {
                        account,
                        username: "dave".into(),
                        display_name: "DAVE".into(),
                    },
                    meta: SessionMeta::default(),
                    started_at_ns: 2,
                    resume_token: token,
                    resume_hash: hash,
                },
                &second,
                now,
            )
            .expect("created");
        assert_eq!(created.replaced, vec![7]);
        assert!(cell.is_closed());
        assert!(driver.outbox.is_closing());
        assert!(
            drain(&erin)
                .iter()
                .any(|m| matches!(m, Message::SessionClose(_)))
        );
        assert!(!w.registry.contains(7));
        assert!(w.registry.contains(8));
    }

    #[test]
    fn session_limit_is_enforced() {
        let mut w = World::with(|c| c.max_sessions = 1);
        let now = w.epoch;
        let _first = w.session(now);
        let conn = w.conn();
        let (token, hash) = new_token().expect("rng");
        let result = w.registry.create_session(
            NewSession {
                id: 9,
                driver: World::identity("other"),
                meta: SessionMeta::default(),
                started_at_ns: 3,
                resume_token: token,
                resume_hash: hash,
            },
            &conn,
            now,
        );
        assert!(matches!(result, Err(RegistryError::Full)));
    }

    // ---------------------------------------------------------------------------------------
    // Engineer slots in the store and relay restarts
    // ---------------------------------------------------------------------------------------

    /// What the store holds for `cell` when the relay stops: the session record and every slot,
    /// each engineer connection recorded as dropped at `dropped_at`.
    fn stored_session(cell: &SessionCell, dropped_at: DateTime<Utc>) -> StoredSession {
        let s = cell.inner.lock();
        StoredSession {
            record: SessionRecord {
                id: s.id,
                driver_account: s.driver.account,
                resume_hash: Some(s.resume_hash),
                meta: s.meta.clone(),
                started_at: ns_to_datetime(s.started_at_ns),
                status: SessionStatus::Grace,
                close_reason: None,
                last_activity: dropped_at,
                ended_at: None,
                // What the store's high-water mark holds once every slot write landed.
                max_connection_id: s.next_connection_id.saturating_sub(1),
            },
            driver: s.driver.clone(),
            slots: s
                .engineers
                .iter()
                .map(|e| StoredSlot {
                    record: e.record(s.id, Some(dropped_at)),
                    engineer: e.engineer.clone(),
                })
                .collect(),
        }
    }

    fn last_engineer_list(msgs: &[Message]) -> EngineerList {
        msgs.iter()
            .rev()
            .find_map(|m| match m {
                Message::EngineerList(l) => Some(l.clone()),
                _ => None,
            })
            .expect("ENGINEER_LIST")
    }

    fn wall_now() -> DateTime<Utc> {
        ns_to_datetime(unix_now_ns())
    }

    #[test]
    fn slot_changes_are_queued_for_the_store_in_order() {
        let mut w = World::new();
        let t0 = w.epoch;
        let (cell, driver, _, code) = w.session(t0);
        assert!(cell.take_slot_writes().is_empty(), "no slots yet");
        let erin_identity = World::identity("erin");
        let erin = w.conn();
        w.registry
            .join(&code, erin_identity.clone(), &erin, t0)
            .expect("joined");
        let ticket = ticket_of(&drain(&erin));
        let joined = cell.take_slot_writes();
        let [SlotWrite::Put(slot)] = joined.as_slice() else {
            panic!("one Put expected, got {joined:?}");
        };
        assert_eq!(
            (slot.session_id, slot.account_id, slot.dropped_at),
            (7, erin_identity.account, None)
        );
        let connection_id = slot.connection_id;
        // A drop is stored: a restarted relay continues the ticket window from it.
        w.registry.engineer_detached(&cell, erin.id, t0);
        assert!(matches!(
            cell.take_slot_writes().as_slice(),
            [SlotWrite::Put(s)] if s.dropped_at.is_some() && s.connection_id == connection_id
        ));
        let back = w.conn();
        w.registry
            .resume_engineer(7, &ticket, erin_identity.account, &back, t0)
            .expect("resumed");
        assert!(matches!(
            cell.take_slot_writes().as_slice(),
            [SlotWrite::Put(s)] if s.dropped_at.is_none()
        ));
        // A kick deletes the row.
        assert!(w.registry.kick(&cell, driver.id, connection_id, t0));
        assert_eq!(
            cell.take_slot_writes(),
            vec![SlotWrite::Delete {
                session_id: 7,
                account: erin_identity.account,
                connection_id
            }]
        );
        // A ticket expiring in housekeeping deletes it, and the session is reported for a flush.
        let code = last_code(&drain(&driver)).expect("slot freed: fresh code");
        let eddie_identity = World::identity("eddie");
        let eddie = w.conn();
        w.registry
            .join(&code, eddie_identity.clone(), &eddie, t0)
            .expect("joined");
        w.registry.engineer_detached(&cell, eddie.id, t0);
        assert_eq!(cell.take_slot_writes().len(), 2, "joined, dropped");
        let report = w.registry.housekeeping(t0 + Duration::from_secs(600));
        assert_eq!(report.slot_writes.len(), 1);
        assert!(matches!(
            cell.take_slot_writes().as_slice(),
            [SlotWrite::Delete { account, .. }] if *account == eddie_identity.account
        ));
        // Closing the session drops queued writes: closing it in the store deletes its rows.
        let emma = w.conn();
        let code = last_code(&drain(&driver)).expect("fresh code");
        w.registry
            .join(&code, World::identity("emma"), &emma, t0)
            .expect("joined");
        assert!(w.registry.close_by_driver(&cell, driver.id, t0));
        assert!(cell.take_slot_writes().is_empty());
    }

    #[test]
    fn slot_write_backlog_drops_put_before_delete_and_prepend_restores_order() {
        let mut w = World::new();
        let t0 = w.epoch;
        let (cell, _, _, code) = w.session(t0);
        let erin = w.conn();
        w.registry
            .join(&code, World::identity("erin"), &erin, t0)
            .expect("joined");
        let base_writes = cell.take_slot_writes();
        let base_put = match base_writes.first().expect("base put") {
            SlotWrite::Put(p) => p.clone(),
            _ => panic!("expected put"),
        };

        let del = SlotWrite::Delete {
            session_id: 7,
            account: Uuid::new_v4(),
            connection_id: 999,
        };
        {
            let mut s = cell.inner.lock();
            for i in 0..(MAX_PENDING_SLOT_WRITES - 1) {
                let mut slot = base_put.clone();
                slot.connection_id = i as u64;
                s.queue_write(SlotWrite::Put(slot));
            }
            s.queue_write(del.clone());
            assert_eq!(s.pending_writes.len(), MAX_PENDING_SLOT_WRITES);

            // Adding another Put must drop an older Put, NOT the Delete (H3)
            let mut extra_put = base_put.clone();
            extra_put.connection_id = 1000;
            s.queue_write(SlotWrite::Put(extra_put));
            assert_eq!(s.pending_writes.len(), MAX_PENDING_SLOT_WRITES);
            assert!(
                s.pending_writes.iter().any(|w| matches!(
                    w,
                    SlotWrite::Delete {
                        connection_id: 999,
                        ..
                    }
                )),
                "Delete write was retained"
            );
        }

        // Prepending restored writes puts them back at the front
        let writes = cell.take_slot_writes();
        assert_eq!(writes.len(), MAX_PENDING_SLOT_WRITES);
        assert!(cell.take_slot_writes().is_empty());

        let (_head, tail) = writes.split_at(2);
        cell.prepend_slot_writes(tail.to_vec());
        let restored = cell.take_slot_writes();
        assert_eq!(restored.len(), MAX_PENDING_SLOT_WRITES - 2);
        assert_eq!(restored[0], tail[0]);
    }

    #[test]
    fn engineers_resume_with_their_ticket_after_a_relay_restart() {
        let mut old = World::new();
        let t0 = old.epoch;
        let (cell, _driver, token, code) = old.session(t0);
        let erin_identity = World::identity("erin");
        let erin = old.conn();
        old.registry
            .join(&code, erin_identity.clone(), &erin, t0)
            .expect("joined");
        let ticket = ticket_of(&drain(&erin));
        let stored = stored_session(&cell, wall_now());
        let connection_id = stored.slots[0].record.connection_id;

        // A new relay process with the same ticket key (same JWT_KEY_FILE).
        let mut new = World::new();
        let now = new.epoch;
        let intruder = new.conn();
        // Another account, or a ticket of another key: refused, and nothing is restored.
        assert!(matches!(
            new.registry.restore_for_engineer(
                &stored,
                Duration::ZERO,
                &ticket,
                Uuid::new_v4(),
                &intruder,
                now
            ),
            Err(ResumeError::NotAuthorized)
        ));
        let (foreign, _) = TicketSealer::new(&[1u8; 32])
            .expect("key")
            .issue(SlotRef {
                session_id: 7,
                account: erin_identity.account,
                connection_id,
            })
            .expect("rng");
        assert!(matches!(
            new.registry.restore_for_engineer(
                &stored,
                Duration::ZERO,
                &foreign,
                erin_identity.account,
                &intruder,
                now
            ),
            Err(ResumeError::NotAuthorized)
        ));
        assert!(
            !new.registry.contains(7),
            "a refused ticket restores nothing"
        );

        // The still-running engineer app resumes with its in-memory ticket, driver not yet back.
        let erin2 = new.conn();
        let cell2 = new
            .registry
            .restore_for_engineer(
                &stored,
                Duration::ZERO,
                &ticket,
                erin_identity.account,
                &erin2,
                now,
            )
            .expect("resumed after the restart");
        let fresh = ticket_of(&drain(&erin2));
        assert_ne!(fresh, ticket, "a new ticket is issued");
        assert!(matches!(
            cell2.take_slot_writes().as_slice(),
            [SlotWrite::Put(s)] if s.dropped_at.is_none() && s.connection_id == connection_id
        ));
        // The presented ticket is now the previous one: dead once the grace is over. The new one
        // works after the next blip.
        new.registry.engineer_detached(&cell2, erin2.id, now);
        let erin3 = new.conn();
        let after_grace = now + new.registry.config().previous_ticket_grace;
        assert!(matches!(
            new.registry
                .resume_engineer(7, &ticket, erin_identity.account, &erin3, after_grace),
            Err(ResumeError::NotAuthorized)
        ));
        new.registry
            .resume_engineer(7, &fresh, erin_identity.account, &erin3, after_grace)
            .expect("resumed");
        drain(&erin3);

        // The driver comes back to the same session with the engineer connected.
        let driver2 = new.conn();
        new.registry
            .resume_driver(7, stored.driver.account, &token, false, &driver2, now)
            .expect("driver resumed");
        let msgs = drain(&driver2);
        let list = last_engineer_list(&msgs);
        assert_eq!(list.engineers.len(), 1);
        assert_eq!(list.engineers[0].username, "erin");
        assert_eq!(list.engineers[0].connection_id, connection_id);
        assert_eq!(list.engineers[0].link, EngineerLink::Healthy);
        assert_eq!(
            last_connect_code(&msgs)
                .expect("CONNECT_CODE")
                .engineers_connected,
            1
        );
        // An engineer is waiting: the driver is asked for a fresh snapshot.
        assert!(
            msgs.iter()
                .any(|m| matches!(m, Message::SnapshotRequest(_)))
        );
    }

    /// The connection id of the single slot Put queued on `cell`.
    fn put_connection_id(cell: &SessionCell) -> u64 {
        let writes = cell.take_slot_writes();
        let [SlotWrite::Put(slot)] = writes.as_slice() else {
            panic!("one Put expected, got {writes:?}");
        };
        slot.connection_id
    }

    #[test]
    fn ended_slot_ids_are_never_reused_after_a_relay_restart() {
        let mut old = World::new();
        let t0 = old.epoch;
        let (cell, driver, token, code) = old.session(t0);
        let erin = old.conn();
        old.registry
            .join(&code, World::identity("erin"), &erin, t0)
            .expect("joined");
        assert_eq!(put_connection_id(&cell), 1);
        let second = last_code(&drain(&driver)).expect("second code");
        let eddie_identity = World::identity("eddie");
        let eddie = old.conn();
        old.registry
            .join(&second, eddie_identity.clone(), &eddie, t0)
            .expect("joined");
        assert_eq!(put_connection_id(&cell), 2);
        let kicked_ticket = ticket_of(&drain(&eddie));
        // Eddie is kicked: his row is deleted, only Erin's slot (id 1) survives in the store.
        assert!(old.registry.kick(&cell, driver.id, 2, t0));
        cell.take_slot_writes();
        let stored = stored_session(&cell, wall_now());
        assert_eq!(stored.slots.len(), 1);
        assert_eq!(stored.record.max_connection_id, 2);

        // Restart: the driver restores the session and Eddie rejoins with a new code.
        let mut mid = World::new();
        let now = mid.epoch;
        let driver2 = mid.conn();
        let cell2 = mid
            .registry
            .restore_session(&stored, &token, false, &driver2, now)
            .expect("restored");
        let code = last_code(&drain(&driver2)).expect("a slot is free");
        let eddie2 = mid.conn();
        mid.registry
            .join(&code, eddie_identity.clone(), &eddie2, now)
            .expect("rejoined");
        assert_eq!(
            put_connection_id(&cell2),
            3,
            "the kicked slot's id 2 is not handed out again"
        );
        let stored2 = stored_session(&cell2, wall_now());

        // Second restart: the kicked slot's ticket does not verify for Eddie's new slot.
        let mut new = World::new();
        let later = new.epoch;
        let intruder = new.conn();
        assert!(matches!(
            new.registry.restore_for_engineer(
                &stored2,
                Duration::ZERO,
                &kicked_ticket,
                eddie_identity.account,
                &intruder,
                later
            ),
            Err(ResumeError::NotAuthorized)
        ));
        assert!(!new.registry.contains(7), "nothing restored");
    }

    #[test]
    fn a_slot_connected_at_the_crash_is_charged_only_the_time_since_the_restart() {
        const TTL: Duration = Duration::from_secs(5);
        let mut old = World::with(|c| c.engineer_ticket_ttl = TTL);
        let t0 = old.epoch;
        let (cell, _driver, _token, code) = old.session(t0);
        let erin_identity = World::identity("erin");
        let erin = old.conn();
        old.registry
            .join(&code, erin_identity.clone(), &erin, t0)
            .expect("joined");
        let ticket = ticket_of(&drain(&erin));
        // Erin was connected when the relay died; the session's last activity is far older than
        // the ticket lifetime (the driver had been away, so it was not touched).
        let mut stored = stored_session(&cell, wall_now());
        stored.slots[0].record.dropped_at = None;
        let long_ago = unix_now_ns().saturating_sub(duration_ns(TTL * 20));
        stored.record.last_activity = ns_to_datetime(long_ago);

        // Right after the restart Erin is not charged the stale last activity.
        let mut new = World::with(|c| c.engineer_ticket_ttl = TTL);
        let now = new.epoch;
        let erin2 = new.conn();
        new.registry
            .restore_for_engineer(
                &stored,
                Duration::ZERO,
                &ticket,
                erin_identity.account,
                &erin2,
                now,
            )
            .expect("resumed right after the restart");

        // The time since the restart does count: past the ticket lifetime the window is over.
        let mut late = World::with(|c| c.engineer_ticket_ttl = TTL);
        let too_late = late.epoch + TTL + Duration::from_secs(1);
        let erin3 = late.conn();
        assert!(matches!(
            late.registry.restore_for_engineer(
                &stored,
                Duration::ZERO,
                &ticket,
                erin_identity.account,
                &erin3,
                too_late,
            ),
            Err(ResumeError::NotAuthorized)
        ));
    }

    #[test]
    fn a_driver_restore_rebuilds_the_slots_with_their_ticket_windows() {
        const TTL: Duration = Duration::from_secs(5);
        let mut old = World::with(|c| c.engineer_ticket_ttl = TTL);
        let t0 = old.epoch;
        let (cell, driver, token, code) = old.session(t0);
        let erin_identity = World::identity("erin");
        let erin = old.conn();
        old.registry
            .join(&code, erin_identity.clone(), &erin, t0)
            .expect("joined");
        let erin_ticket = ticket_of(&drain(&erin));
        let second = last_code(&drain(&driver)).expect("second code");
        let eddie_identity = World::identity("eddie");
        let eddie = old.conn();
        old.registry
            .join(&second, eddie_identity.clone(), &eddie, t0)
            .expect("joined");
        let eddie_ticket = ticket_of(&drain(&eddie));
        let mut stored = stored_session(&cell, wall_now());
        // Eddie's connection dropped longer ago than the ticket lifetime.
        let long_ago = unix_now_ns().saturating_sub(duration_ns(TTL + Duration::from_secs(1)));
        stored.slots[1].record.dropped_at = Some(ns_to_datetime(long_ago));
        let ids: Vec<u64> = stored
            .slots
            .iter()
            .map(|s| s.record.connection_id)
            .collect();

        let mut new = World::with(|c| c.engineer_ticket_ttl = TTL);
        let now = new.epoch;
        let driver2 = new.conn();
        let cell2 = new
            .registry
            .restore_session(&stored, &token, false, &driver2, now)
            .expect("restored");
        let msgs = drain(&driver2);
        let list = last_engineer_list(&msgs);
        assert_eq!(
            list.engineers
                .iter()
                .map(|e| (e.connection_id, e.link))
                .collect::<Vec<_>>(),
            vec![
                (ids[0], EngineerLink::Reconnecting),
                (ids[1], EngineerLink::Reconnecting)
            ]
        );
        let cc = last_connect_code(&msgs).expect("CONNECT_CODE");
        assert_eq!(cc.engineers_connected, 2);
        assert!(
            cc.code.is_none(),
            "no code while the restored slots fill the session"
        );

        // Erin resumes; Eddie's window is over.
        let erin2 = new.conn();
        new.registry
            .resume_engineer(7, &erin_ticket, erin_identity.account, &erin2, now)
            .expect("erin is back");
        let eddie2 = new.conn();
        assert!(matches!(
            new.registry
                .resume_engineer(7, &eddie_ticket, eddie_identity.account, &eddie2, now),
            Err(ResumeError::NotAuthorized)
        ));
        cell2.take_slot_writes();
        // Housekeeping frees Eddie's slot (and deletes its row); the driver gets a code again.
        new.registry.housekeeping(now);
        assert!(matches!(
            cell2.take_slot_writes().as_slice(),
            [SlotWrite::Delete { account, .. }] if *account == eddie_identity.account
        ));
        let cc = last_connect_code(&drain(&driver2)).expect("slot freed");
        assert_eq!(cc.engineers_connected, 1);
        let fresh = cc.code.expect("a fresh code").code;
        // A new engineer gets a connection id above every restored one.
        let emma = new.conn();
        new.registry
            .join(&fresh, World::identity("emma"), &emma, now)
            .expect("joined");
        let list = last_engineer_list(&drain(&driver2));
        let emma_id = list
            .engineers
            .iter()
            .find(|e| e.username == "emma")
            .expect("emma listed")
            .connection_id;
        assert!(ids.iter().all(|id| emma_id > *id));
    }

    #[test]
    fn a_restored_driver_grace_continues_from_before_the_restart() {
        let mut old = World::new();
        let t0 = old.epoch;
        let (cell, _driver, _token, code) = old.session(t0);
        let erin_identity = World::identity("erin");
        let erin = old.conn();
        old.registry
            .join(&code, erin_identity.clone(), &erin, t0)
            .expect("joined");
        let ticket = ticket_of(&drain(&erin));
        let stored = stored_session(&cell, wall_now());
        let mut new = World::new();
        let now = new.epoch;
        let erin2 = new.conn();
        // The driver has been away for 590 s of its 600 s grace already.
        new.registry
            .restore_for_engineer(
                &stored,
                Duration::from_secs(590),
                &ticket,
                erin_identity.account,
                &erin2,
                now,
            )
            .expect("resumed");
        drain(&erin2);
        assert!(
            new.registry
                .housekeeping(now + Duration::from_secs(9))
                .expired
                .is_empty()
        );
        assert_eq!(
            new.registry
                .housekeeping(now + Duration::from_secs(10))
                .expired,
            vec![7]
        );
        assert!(drain(&erin2).iter().any(|m| matches!(
            m,
            Message::SessionClose(SessionClose {
                reason: SessionCloseReason::Expired,
                ..
            })
        )));
    }
}
