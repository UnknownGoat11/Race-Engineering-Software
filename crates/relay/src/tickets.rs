//! Engineer resume tickets (D-022, PROTOCOL.md §6).
//!
//! A ticket is `base64url(r ‖ tag)`: `r` is 32 bytes (256 bits) from the OS CSPRNG and `tag` the
//! first [`TICKET_TAG_BYTES`] bytes of HMAC-SHA256(K, domain ‖ session id ‖ engineer account ‖
//! connection id ‖ r). Clients treat it as an opaque [`Secret`].
//!
//! * The running relay knows the tickets it issued by SHA-256 digest only ([`TokenHash`], relay
//!   memory) and accepts the newest ticket of a slot (tickets rotate on every SESSION_INFO) plus,
//!   each for `previous_ticket_grace` after the ticket that replaced it was issued, up to two
//!   earlier ones the engineer may still hold (`crate::registry`; the SESSION_INFO with a newer
//!   ticket may have been lost).
//! * After a relay restart those digests are gone. The tag lets the new process recognise a ticket
//!   of an engineer slot the store still lists ([`crate::store::EngineerSlotRecord`]: session
//!   metadata without any ticket material), so a running engineer app resumes without user action
//!   (D-019, D-020, D-022). Until the slot's first successful resume after the restart this accepts
//!   **any** ticket ever issued for the slot (same session, account and connection id), including
//!   ones that had died by grace or by use: it only resumes the same account into its own slot. A
//!   slot ends (DISCONNECT, ENGINEER_KICK, expiry, session end) by its store row being deleted, so
//!   a ticket of an ended slot never works again, restart or not.
//! * K is derived from the access-token signing key (`JWT_KEY_FILE`, [`TICKET_KEY_LABEL`]), so
//!   tickets survive a restart exactly when that key does. Tickets and their digests are never
//!   stored or logged.

use hmac::{Hmac, Mac};
use protocol::messages::Secret;
use sha2::Sha256;
use thiserror::Error;
use uuid::Uuid;

use crate::secrets::{RandomError, TokenHash, b64url, b64url_decode, random_bytes};

/// Random bytes of a ticket (256 bits).
pub const TICKET_RANDOM_BYTES: usize = 32;
/// Bytes of the (left-truncated) HMAC-SHA256 tag of a ticket.
pub const TICKET_TAG_BYTES: usize = 16;
/// Raw ticket length.
const TICKET_BYTES: usize = TICKET_RANDOM_BYTES + TICKET_TAG_BYTES;
/// Length of a ticket in unpadded base64url (48 bytes = 64 characters).
pub const TICKET_CHARS: usize = TICKET_BYTES / 3 * 4;
/// Label of the ticket key derived from the access-token signing key.
pub const TICKET_KEY_LABEL: &[u8] = b"lmu-relay/engineer-ticket-key/v1";
/// Domain separation of the ticket tag.
const TICKET_DOMAIN: &[u8] = b"lmu-relay/engineer-ticket/v1";

/// The engineer slot a ticket is bound to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SlotRef {
    pub session_id: u64,
    /// The engineer account holding the slot.
    pub account: Uuid,
    /// Relay-assigned connection id of the slot (ENGINEER_LIST, ENGINEER_KICK).
    pub connection_id: u64,
}

/// The ticket key could not be set up.
#[derive(Debug, Error)]
pub enum TicketKeyError {
    /// HMAC refused the key (impossible for HMAC-SHA256, which accepts any key length).
    #[error("the HMAC implementation refused the ticket key")]
    Key,
}

/// Issues and verifies engineer resume tickets.
#[derive(Clone)]
pub struct TicketSealer {
    /// HMAC state already keyed; cloned per ticket.
    keyed: Hmac<Sha256>,
}

impl std::fmt::Debug for TicketSealer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("TicketSealer(<key redacted>)")
    }
}

impl TicketSealer {
    /// A sealer keyed with `key` (a secret of at least 32 bytes).
    pub fn new(key: &[u8]) -> Result<Self, TicketKeyError> {
        let keyed = <Hmac<Sha256> as Mac>::new_from_slice(key).map_err(|_| TicketKeyError::Key)?;
        Ok(Self { keyed })
    }

    fn tag(&self, slot: SlotRef, random: &[u8]) -> Hmac<Sha256> {
        let mut mac = self.keyed.clone();
        mac.update(TICKET_DOMAIN);
        mac.update(&slot.session_id.to_le_bytes());
        mac.update(slot.account.as_bytes());
        mac.update(&slot.connection_id.to_le_bytes());
        mac.update(random);
        mac
    }

    /// A fresh ticket for `slot` and its digest (the only form the relay keeps).
    pub fn issue(&self, slot: SlotRef) -> Result<(Secret, TokenHash), RandomError> {
        let random = random_bytes::<TICKET_RANDOM_BYTES>()?;
        let full = self.tag(slot, &random).finalize().into_bytes();
        let mut raw = [0u8; TICKET_BYTES];
        raw[..TICKET_RANDOM_BYTES].copy_from_slice(&random);
        raw[TICKET_RANDOM_BYTES..].copy_from_slice(&full[..TICKET_TAG_BYTES]);
        let ticket = b64url(&raw);
        let digest = TokenHash::of(&ticket);
        Ok((Secret::new(ticket), digest))
    }

    /// Whether `ticket` was issued by this key for exactly `slot` (constant-time tag check).
    pub fn verify(&self, ticket: &Secret, slot: SlotRef) -> bool {
        let text = ticket.expose();
        if text.len() != TICKET_CHARS {
            return false;
        }
        let Some(raw) = b64url_decode(text) else {
            return false;
        };
        if raw.len() != TICKET_BYTES {
            return false;
        }
        let (random, tag) = raw.split_at(TICKET_RANDOM_BYTES);
        self.tag(slot, random).verify_truncated_left(tag).is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slot() -> SlotRef {
        SlotRef {
            session_id: 7,
            account: Uuid::from_u128(0x8f0e_7c52_6f3c_4a8e_9a4b_2d1f_0c9e_7b11),
            connection_id: 3,
        }
    }

    #[test]
    fn issued_tickets_verify_for_their_slot_only() {
        let sealer = TicketSealer::new(&[1u8; 32]).expect("key");
        let (ticket, digest) = sealer.issue(slot()).expect("rng");
        assert_eq!(ticket.expose().len(), TICKET_CHARS);
        assert!(digest.ct_eq(&TokenHash::of(ticket.expose())));
        assert!(sealer.verify(&ticket, slot()));
        for other in [
            SlotRef {
                session_id: 8,
                ..slot()
            },
            SlotRef {
                account: Uuid::from_u128(1),
                ..slot()
            },
            SlotRef {
                connection_id: 4,
                ..slot()
            },
        ] {
            assert!(!sealer.verify(&ticket, other));
        }
        // Every ticket is fresh, even for the same slot.
        let (again, _) = sealer.issue(slot()).expect("rng");
        assert_ne!(again, ticket);
        assert!(sealer.verify(&again, slot()));
    }

    #[test]
    fn foreign_keys_tampering_and_garbage_are_refused() {
        let sealer = TicketSealer::new(&[1u8; 32]).expect("key");
        let other = TicketSealer::new(&[2u8; 32]).expect("key");
        let (ticket, _) = sealer.issue(slot()).expect("rng");
        assert!(!other.verify(&ticket, slot()));
        // Flip one bit of the random part.
        let mut raw = b64url_decode(ticket.expose()).expect("base64url");
        raw[0] ^= 1;
        assert!(!sealer.verify(&Secret::new(b64url(&raw)), slot()));
        for garbage in [
            "",
            "not-a-ticket",
            &"A".repeat(TICKET_CHARS),
            &"!".repeat(64),
        ] {
            assert!(!sealer.verify(&Secret::new(garbage), slot()));
        }
        assert_eq!(
            format!("{sealer:?}"),
            "TicketSealer(<key redacted>)",
            "the key never reaches a log"
        );
    }
}
