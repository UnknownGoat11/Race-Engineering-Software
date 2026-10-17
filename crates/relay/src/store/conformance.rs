//! Store conformance tests, run against [`MemoryStore`] always and against PostgreSQL when
//! `LMU_RELAY_TEST_DATABASE_URL` points at a disposable database (`cargo test -p lmu-relay --
//! --ignored postgres`).

use chrono::{DateTime, Duration as ChronoDuration, Utc};
use uuid::Uuid;

use super::{
    Account, AccountRole, AccountStore, EngineerSlotRecord, MemoryStore, NewRefreshToken, PgStore,
    RotateOutcome, SessionMeta, SessionRecord, SessionStatus, SessionStore, Store, StoreError,
};
use crate::secrets::{TokenHash, random_u64};

fn t(offset_s: i64) -> DateTime<Utc> {
    // A fixed, microsecond-aligned base (PostgreSQL stores microseconds).
    DateTime::from_timestamp(1_790_000_000 + offset_s, 0).unwrap_or(DateTime::<Utc>::MIN_UTC)
}

fn account(username: &str, role: AccountRole) -> Account {
    Account {
        id: Uuid::new_v4(),
        username: username.to_owned(),
        display_name: username.to_uppercase(),
        role,
        password_hash: "$argon2id$v=19$m=64,t=1,p=1$c2FsdHNhbHQ$aGFzaA".to_owned(),
        disabled: false,
        created_at: t(0),
    }
}

fn token(family: Uuid, account: Uuid, label: &str, created: i64) -> NewRefreshToken {
    NewRefreshToken {
        id: Uuid::new_v4(),
        family_id: family,
        account_id: account,
        token_hash: TokenHash::of(&format!("{label}-{}", Uuid::new_v4())),
        created_at: t(created),
        expires_at: t(created + 90 * 86_400),
    }
}

/// Runs every conformance check against `store`. Usernames are randomized so the checks can run
/// repeatedly against one database.
pub(crate) async fn run_all(store: &Store) {
    let suffix = Uuid::new_v4().simple().to_string();
    accounts(store, &suffix).await;
    refresh_tokens(store, &suffix).await;
    inactive_logins(store, &suffix).await;
    sessions(store, &suffix).await;
    engineer_slots(store, &suffix).await;
}

async fn accounts(store: &Store, suffix: &str) {
    let alice = account(&format!("alice{suffix}"), AccountRole::Engineer);
    store.create_account(&alice).await.expect("create");
    let mut dup = account(&alice.username, AccountRole::Driver);
    dup.id = Uuid::new_v4();
    assert!(matches!(
        store.create_account(&dup).await,
        Err(StoreError::Conflict(_))
    ));
    let by_name = store
        .account_by_username(&alice.username)
        .await
        .expect("query")
        .expect("found");
    assert_eq!(by_name, alice);
    assert_eq!(
        store.account_by_id(alice.id).await.expect("query"),
        Some(alice.clone())
    );
    assert_eq!(
        store
            .account_by_username("nobody-here")
            .await
            .expect("query"),
        None
    );
    store.record_login(alice.id, t(10)).await.expect("login");
}

async fn refresh_tokens(store: &Store, suffix: &str) {
    let bob = account(&format!("bob{suffix}"), AccountRole::Driver);
    store.create_account(&bob).await.expect("create");
    let family = Uuid::new_v4();
    store
        .create_token_family(family, bob.id, t(0))
        .await
        .expect("family");
    assert_eq!(store.family_revoked(family).await.expect("q"), Some(false));
    assert_eq!(store.family_revoked(Uuid::new_v4()).await.expect("q"), None);

    let first = token(family, bob.id, "first", 0);
    store.insert_refresh_token(&first, 4).await.expect("insert");
    let found = store
        .refresh_token_by_hash(&first.token_hash)
        .await
        .expect("q")
        .expect("found");
    assert_eq!(found.id, first.id);
    assert_eq!(found.rotated_at, None);

    // Rotation succeeds once; a second rotation of the same token loses.
    let second = token(family, bob.id, "second", 5);
    assert_eq!(
        store
            .rotate_refresh_token(first.id, &second, t(5))
            .await
            .expect("rotate"),
        RotateOutcome::Rotated
    );
    let third = token(family, bob.id, "third", 6);
    assert_eq!(
        store
            .rotate_refresh_token(first.id, &third, t(6))
            .await
            .expect("rotate"),
        RotateOutcome::Lost
    );
    assert_eq!(
        store
            .refresh_token_by_hash(&third.token_hash)
            .await
            .expect("q"),
        None,
        "a lost rotation inserts nothing"
    );
    let rotated = store
        .refresh_token_by_hash(&first.token_hash)
        .await
        .expect("q")
        .expect("found");
    assert_eq!(rotated.rotated_at, Some(t(5)));

    // Family cap: keep the newest 2 active tokens.
    let a = token(family, bob.id, "a", 10);
    let b = token(family, bob.id, "b", 11);
    store.insert_refresh_token(&a, 2).await.expect("insert");
    store.insert_refresh_token(&b, 2).await.expect("insert");
    let second_now = store
        .refresh_token_by_hash(&second.token_hash)
        .await
        .expect("q")
        .expect("found");
    assert!(
        second_now.revoked_at.is_some(),
        "oldest active token revoked by the cap"
    );
    for kept in [&a, &b] {
        let rec = store
            .refresh_token_by_hash(&kept.token_hash)
            .await
            .expect("q")
            .expect("found");
        assert_eq!(rec.revoked_at, None);
    }

    // Rotating a live token supersedes every other live token of the family exactly like a use
    // (one live refresh token per login); used and revoked tokens keep their state.
    let state = |rec: Option<super::RefreshTokenRecord>| {
        rec.map(|r| (r.rotated_at, r.revoked_at))
            .expect("token found")
    };
    let successor = token(family, bob.id, "successor", 15);
    assert_eq!(
        store
            .rotate_refresh_token(a.id, &successor, t(15))
            .await
            .expect("rotate"),
        RotateOutcome::Rotated
    );
    for old in [&a, &b] {
        let rec = store
            .refresh_token_by_hash(&old.token_hash)
            .await
            .expect("q");
        assert_eq!(state(rec), (Some(t(15)), None), "rotated / superseded");
    }
    let rec = store
        .refresh_token_by_hash(&first.token_hash)
        .await
        .expect("q");
    assert_eq!(
        state(rec),
        (Some(t(5)), None),
        "a used token keeps its use time"
    );
    let rec = store
        .refresh_token_by_hash(&second.token_hash)
        .await
        .expect("q");
    assert!(state(rec).1.is_some(), "a revoked token stays revoked");
    let rec = store
        .refresh_token_by_hash(&successor.token_hash)
        .await
        .expect("q");
    assert_eq!(state(rec), (None, None), "the successor is the live token");
    // A superseded token cannot be rotated (it is used); nothing changes.
    let late = token(family, bob.id, "late", 16);
    assert_eq!(
        store
            .rotate_refresh_token(b.id, &late, t(16))
            .await
            .expect("rotate"),
        RotateOutcome::Lost
    );
    // A tolerated use (insert without superseding) leaves the live token live.
    let tolerated = token(family, bob.id, "tolerated", 17);
    store
        .insert_refresh_token(&tolerated, 4)
        .await
        .expect("insert");
    for live in [&successor, &tolerated] {
        let rec = store
            .refresh_token_by_hash(&live.token_hash)
            .await
            .expect("q");
        assert_eq!(state(rec), (None, None), "both live");
    }
    // A failed rotation (duplicate digest) changes nothing.
    let mut duplicate = token(family, bob.id, "duplicate", 18);
    duplicate.token_hash = tolerated.token_hash;
    assert!(
        store
            .rotate_refresh_token(successor.id, &duplicate, t(18))
            .await
            .is_err()
    );
    for live in [&successor, &tolerated] {
        let rec = store
            .refresh_token_by_hash(&live.token_hash)
            .await
            .expect("q");
        assert_eq!(
            state(rec),
            (None, None),
            "unchanged after a failed rotation"
        );
    }

    // Concurrent writers of one family (a rotation and a tolerated insert) neither fail nor
    // deadlock (review relay2 F4; PostgreSQL: both lock the family row before any token row).
    // At most three tokens are live at any point, below the cap of 4, so none is revoked.
    const RACES: u32 = 8;
    let mut live = successor.clone();
    for i in 0..RACES {
        let next = token(family, bob.id, &format!("race-rotate-{i}"), 19);
        let extra = token(family, bob.id, &format!("race-insert-{i}"), 19);
        let (rotated, inserted) = tokio::join!(
            store.rotate_refresh_token(live.id, &next, t(19)),
            store.insert_refresh_token(&extra, 4),
        );
        assert_eq!(rotated.expect("rotate"), RotateOutcome::Rotated);
        inserted.expect("insert");
        live = next;
    }
    let rec = store
        .refresh_token_by_hash(&live.token_hash)
        .await
        .expect("q");
    assert_eq!(state(rec), (None, None), "the last successor is live");

    // Family revocation revokes every token and marks the family.
    store.revoke_family(family, t(20)).await.expect("revoke");
    assert_eq!(store.family_revoked(family).await.expect("q"), Some(true));
    let rec = store
        .refresh_token_by_hash(&b.token_hash)
        .await
        .expect("q")
        .expect("found");
    assert_eq!(rec.revoked_at, Some(t(20)));

    // Account-wide revocation covers other families.
    let family2 = Uuid::new_v4();
    store
        .create_token_family(family2, bob.id, t(30))
        .await
        .expect("family");
    let c = token(family2, bob.id, "c", 30);
    store.insert_refresh_token(&c, 4).await.expect("insert");
    assert_eq!(
        store
            .revoke_account_tokens(bob.id, t(40))
            .await
            .expect("revoke"),
        1
    );
    assert_eq!(store.family_revoked(family2).await.expect("q"), Some(true));

    // Purge removes expired tokens only.
    let purged = store
        .purge_refresh_tokens(t(91 * 86_400))
        .await
        .expect("purge");
    assert!(purged >= 5);
    assert_eq!(
        store.refresh_token_by_hash(&c.token_hash).await.expect("q"),
        None
    );
}

async fn inactive_logins(store: &Store, suffix: &str) {
    let carol = account(&format!("carol{suffix}"), AccountRole::Engineer);
    let mut dan = account(&format!("dan{suffix}"), AccountRole::Driver);
    dan.disabled = true;
    for a in [&carol, &dan] {
        store.create_account(a).await.expect("create");
    }
    let (active, revoked, disabled, unknown) = (
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::new_v4(),
    );
    for (family, owner) in [(active, carol.id), (revoked, carol.id), (disabled, dan.id)] {
        store
            .create_token_family(family, owner, t(0))
            .await
            .expect("family");
    }
    assert!(store.inactive_families(&[]).await.expect("q").is_empty());
    assert!(
        store
            .inactive_families(&[active])
            .await
            .expect("q")
            .is_empty()
    );
    store.revoke_family(revoked, t(5)).await.expect("revoke");
    let mut inactive = store
        .inactive_families(&[active, revoked, disabled, unknown])
        .await
        .expect("q");
    inactive.sort();
    let mut expected = vec![revoked, disabled, unknown];
    expected.sort();
    assert_eq!(inactive, expected, "revoked, disabled and unknown logins");
    // Logout everywhere deactivates the remaining login too.
    store
        .revoke_account_tokens(carol.id, t(6))
        .await
        .expect("revoke");
    assert_eq!(
        store.inactive_families(&[active]).await.expect("q"),
        vec![active]
    );
}

async fn sessions(store: &Store, suffix: &str) {
    let driver = account(&format!("dave{suffix}"), AccountRole::Driver);
    store.create_account(&driver).await.expect("create");
    let id = (random_u64().expect("rng") >> 1).max(1);
    let record = SessionRecord {
        id,
        driver_account: driver.id,
        resume_hash: Some(TokenHash::of("resume")),
        meta: SessionMeta {
            car: Some("Ferrari 499P".into()),
            vehicle_class: Some("Hypercar".into()),
            track: Some("Le Mans".into()),
            session_type: Some(10),
            lmu_version: None,
            app_version: "0.1.0".into(),
            schema_version: 3,
        },
        started_at: t(0),
        status: SessionStatus::Live,
        close_reason: None,
        last_activity: t(0),
        ended_at: None,
        max_connection_id: 0,
    };
    store.insert_session(&record).await.expect("insert");
    assert!(matches!(
        store.insert_session(&record).await,
        Err(StoreError::Conflict(_))
    ));
    assert_eq!(store.session(id).await.expect("q"), Some(record.clone()));

    store
        .set_session_status(id, SessionStatus::Grace, t(50))
        .await
        .expect("status");
    store.touch_sessions(&[id], t(60)).await.expect("touch");
    let s = store.session(id).await.expect("q").expect("found");
    assert_eq!(s.status, SessionStatus::Grace);
    assert_eq!(s.last_activity, t(60));

    // Expiry closes idle open sessions and clears the resume digest.
    assert_eq!(
        store.expire_sessions(t(59), t(70)).await.expect("expire"),
        0
    );
    assert_eq!(
        store.expire_sessions(t(61), t(70)).await.expect("expire"),
        1
    );
    let s = store.session(id).await.expect("q").expect("found");
    assert_eq!(s.status, SessionStatus::Closed);
    assert_eq!(s.resume_hash, None);
    assert_eq!(s.close_reason.as_deref(), Some(super::CLOSE_REASON_EXPIRED));

    // A closed session stays closed.
    store
        .set_session_status(id, SessionStatus::Live, t(80))
        .await
        .expect("status");
    assert_eq!(
        store.session(id).await.expect("q").expect("found").status,
        SessionStatus::Closed
    );
    assert_eq!(store.session(u64::MAX).await.expect("q"), None);
    store.ping().await.expect("ping");

    // close_session on an open session.
    let id2 = id.wrapping_add(1).max(1) & (u64::MAX >> 1);
    let mut r2 = record.clone();
    r2.id = id2.max(1);
    store.insert_session(&r2).await.expect("insert");
    store
        .close_session(r2.id, super::CLOSE_REASON_DRIVER_STOPPED, t(90))
        .await
        .expect("close");
    let s = store.session(r2.id).await.expect("q").expect("found");
    assert_eq!(s.status, SessionStatus::Closed);
    assert_eq!(s.ended_at, Some(t(90)));

    // Retention: closed sessions that ended before the cutoff are deleted; open ones never are,
    // however old their activity.
    let mut r3 = open_session(id.wrapping_add(2).max(1) & (u64::MAX >> 1), driver.id, 0);
    r3.id = r3.id.max(1);
    store.insert_session(&r3).await.expect("insert");
    assert!(store.purge_closed_sessions(t(80)).await.expect("purge") >= 1);
    assert_eq!(store.session(id).await.expect("q"), None, "ended at t(70)");
    assert!(
        store.session(r2.id).await.expect("q").is_some(),
        "ended at t(90), after the cutoff"
    );
    store.purge_closed_sessions(t(1_000)).await.expect("purge");
    assert_eq!(store.session(r2.id).await.expect("q"), None);
    assert!(
        store.session(r3.id).await.expect("q").is_some(),
        "open sessions are kept"
    );
    store
        .close_session(r3.id, super::CLOSE_REASON_DRIVER_STOPPED, t(1_000))
        .await
        .expect("close");
    let _ = ChronoDuration::zero();
}

fn open_session(id: u64, driver: Uuid, last_activity: i64) -> SessionRecord {
    SessionRecord {
        id,
        driver_account: driver,
        resume_hash: Some(TokenHash::of(&format!("resume-{id}"))),
        meta: SessionMeta {
            app_version: "0.1.0".into(),
            schema_version: 1,
            ..SessionMeta::default()
        },
        started_at: t(0),
        status: SessionStatus::Live,
        close_reason: None,
        last_activity: t(last_activity),
        ended_at: None,
        max_connection_id: 0,
    }
}

async fn engineer_slots(store: &Store, suffix: &str) {
    let driver = account(&format!("dora{suffix}"), AccountRole::Driver);
    let erin = account(&format!("erin{suffix}"), AccountRole::Engineer);
    let eddie = account(&format!("eddie{suffix}"), AccountRole::Engineer);
    for a in [&driver, &erin, &eddie] {
        store.create_account(a).await.expect("create");
    }
    let id = (random_u64().expect("rng") >> 1).max(1);
    let slot = |account: Uuid, connection_id: u64, dropped: Option<i64>| EngineerSlotRecord {
        session_id: id,
        account_id: account,
        connection_id,
        connected_since: t(1),
        dropped_at: dropped.map(t),
    };
    // No slot is stored for an unknown session.
    store
        .put_engineer_slot(&slot(erin.id, 1, None))
        .await
        .expect("put");
    assert!(store.engineer_slots(id).await.expect("q").is_empty());

    store
        .insert_session(&open_session(id, driver.id, 0))
        .await
        .expect("insert");
    store
        .put_engineer_slot(&slot(eddie.id, 2, Some(5)))
        .await
        .expect("put");
    store
        .put_engineer_slot(&slot(erin.id, 1, None))
        .await
        .expect("put");
    assert_eq!(
        store.engineer_slots(id).await.expect("q"),
        vec![slot(erin.id, 1, None), slot(eddie.id, 2, Some(5))],
        "ordered by connection id"
    );
    // Put replaces the account's slot: a drop is recorded, a takeover moves the connection id.
    store
        .put_engineer_slot(&slot(erin.id, 1, Some(7)))
        .await
        .expect("put");
    store
        .put_engineer_slot(&slot(eddie.id, 3, None))
        .await
        .expect("put");
    assert_eq!(
        store.engineer_slots(id).await.expect("q"),
        vec![slot(erin.id, 1, Some(7)), slot(eddie.id, 3, None)]
    );
    // Deleting an older connection's slot keeps the account's newer slot.
    store
        .delete_engineer_slot(id, eddie.id, 2)
        .await
        .expect("delete");
    assert_eq!(store.engineer_slots(id).await.expect("q").len(), 2);
    store
        .delete_engineer_slot(id, eddie.id, 3)
        .await
        .expect("delete");
    assert_eq!(
        store.engineer_slots(id).await.expect("q"),
        vec![slot(erin.id, 1, Some(7))]
    );
    // The connection-id high-water mark survives the deletion of the slot that set it, and a
    // put of a lower id never lowers it.
    let high_water = |record: Option<SessionRecord>| record.expect("record").max_connection_id;
    assert_eq!(high_water(store.session(id).await.expect("q")), 3);
    store
        .put_engineer_slot(&slot(erin.id, 1, None))
        .await
        .expect("put");
    assert_eq!(high_water(store.session(id).await.expect("q")), 3);
    // Closing the session deletes its slots, and a later put is ignored.
    store
        .close_session(id, super::CLOSE_REASON_DRIVER_STOPPED, t(20))
        .await
        .expect("close");
    assert!(store.engineer_slots(id).await.expect("q").is_empty());
    store
        .put_engineer_slot(&slot(erin.id, 1, None))
        .await
        .expect("put");
    assert!(store.engineer_slots(id).await.expect("q").is_empty());

    // Expiry deletes the slots of the sessions it closes.
    let id2 = (random_u64().expect("rng") >> 1).max(1);
    store
        .insert_session(&open_session(id2, driver.id, 0))
        .await
        .expect("insert");
    store
        .put_engineer_slot(&EngineerSlotRecord {
            session_id: id2,
            ..slot(erin.id, 4, None)
        })
        .await
        .expect("put");
    assert_eq!(store.engineer_slots(id2).await.expect("q").len(), 1);
    assert!(store.expire_sessions(t(10), t(30)).await.expect("expire") >= 1);
    assert!(store.engineer_slots(id2).await.expect("q").is_empty());
}

#[tokio::test]
async fn memory_store_conformance() {
    run_all(&Store::Memory(MemoryStore::new())).await;
}

#[tokio::test]
#[ignore = "needs a disposable PostgreSQL database in LMU_RELAY_TEST_DATABASE_URL"]
async fn postgres_store_conformance() {
    let Ok(url) = std::env::var("LMU_RELAY_TEST_DATABASE_URL") else {
        panic!("set LMU_RELAY_TEST_DATABASE_URL to run this test");
    };
    let store = PgStore::connect(&protocol::messages::Secret::new(url), 4)
        .await
        .expect("connect");
    store.migrate().await.expect("migrate");
    run_all(&Store::Postgres(store)).await;
}
