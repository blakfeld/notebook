use std::collections::{BTreeMap, HashMap, HashSet};

use notebook_core::protocol::ErrorCode;

use super::*;
use crate::auth::store::{NewAccount, create_account};
use crate::db;

const MAX: usize = 1000;

struct Fixture {
    _dir: tempfile::TempDir,
    pool: SqlitePool,
}

const VAULT: [u8; 16] = [2; 16];

async fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let pool = db::connect(&dir.path().join("db")).await.unwrap();
    db::migrate(&pool).await.unwrap();
    add_vault(&pool, 1, VAULT).await;
    Fixture { _dir: dir, pool }
}

async fn add_vault(pool: &SqlitePool, user: u8, vault: [u8; 16]) {
    let mut tx = pool.begin().await.unwrap();
    create_account(
        &mut tx,
        &NewAccount {
            user_id: [user; 16],
            username: &format!("user{user}"),
            opaque_record: b"r",
            recovery_verifier_hash: [0; 32],
            is_admin: false,
            vault_id: vault,
            key_bundle: b"b",
            now: 0,
        },
        100 * 1024 * 1024,
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
}

fn note(n: u128) -> NoteId {
    NoteId::from_parts(1, n)
}

fn op(n: u128) -> OpId {
    OpId::from_parts(2, n)
}

fn key(n: u128) -> Bytes {
    Bytes(format!("key-{n}").into_bytes())
}

/// An update `o` for note `n`; carries the note's key.
fn upd(o: u128, n: u128, data: &[u8]) -> PushUpdate {
    PushUpdate {
        id: op(o),
        note: note(n),
        data: Bytes(data.to_vec()),
        note_key: Some(key(n)),
    }
}

fn keyless(o: u128, n: u128, data: &[u8]) -> PushUpdate {
    PushUpdate {
        note_key: None,
        ..upd(o, n, data)
    }
}

impl Fixture {
    async fn push(&self, updates: &[PushUpdate]) -> Result<Pushed, SyncError> {
        push(&self.pool, &VAULT, updates, MAX).await
    }

    async fn seq(&self, update: PushUpdate) -> u64 {
        self.push(&[update]).await.unwrap().seqs[0]
    }

    async fn head(&self) -> u64 {
        head(&self.pool, &VAULT).await.unwrap()
    }

    async fn count(&self, sql: &'static str) -> i64 {
        sqlx::query_scalar(sql).fetch_one(&self.pool).await.unwrap()
    }

    async fn pull(&self, after: u64) -> ChangesResponse {
        changes(&self.pool, &VAULT, after).await.unwrap()
    }
}

fn update_seqs(page: &ChangesResponse) -> Vec<u64> {
    page.entries
        .iter()
        .filter(|e| matches!(e, Entry::Update { .. }))
        .map(Entry::seq)
        .collect()
}

// --- push ---

#[tokio::test]
async fn sequence_numbers_increase() {
    let f = fixture().await;
    let a = f.seq(upd(1, 1, b"a")).await;
    let b = f.seq(keyless(2, 1, b"b")).await;
    assert!(b > a);
    let batch = f
        .push(&[keyless(3, 1, b"c"), upd(4, 2, b"d"), keyless(5, 2, b"e")])
        .await
        .unwrap();
    assert_eq!(batch.seqs, [b + 1, b + 2, b + 3]);
    assert_eq!(batch.head, b + 3);
    assert_eq!(batch.delta, 3);
    assert_eq!(f.head().await, b + 3);
}

#[tokio::test]
async fn retry_returns_the_same_seq() {
    let f = fixture().await;
    let first = f.push(&[upd(1, 1, b"a")]).await.unwrap();
    let retry = f
        .push(&[upd(1, 1, b"a"), keyless(2, 1, b"b")])
        .await
        .unwrap();
    assert_eq!(retry.seqs[0], first.seqs[0]);
    assert_eq!(retry.entries.len(), 1, "only the new update is relayed");
    assert_eq!(f.count("SELECT count(*) FROM ops").await, 2);
}

#[tokio::test]
async fn reused_op_id_with_different_content_conflicts() {
    let f = fixture().await;
    f.push(&[upd(1, 1, b"a"), upd(9, 2, b"z")]).await.unwrap();
    let head = f.head().await;
    for bad in [keyless(1, 1, b"different"), keyless(1, 2, b"a")] {
        let err = f.push(&[keyless(3, 1, b"new"), bad]).await.unwrap_err();
        assert_eq!(err, SyncError::OP_ID_CONFLICT);
    }
    assert_eq!(f.head().await, head);
    assert_eq!(f.count("SELECT count(*) FROM ops").await, 2);
}

#[tokio::test]
async fn note_keys_are_required_and_write_once() {
    let f = fixture().await;
    assert_eq!(
        f.push(&[keyless(1, 1, b"a")]).await.unwrap_err(),
        SyncError::MISSING_NOTE_KEY
    );
    let pushed = f.push(&[upd(1, 1, b"a")]).await.unwrap();
    assert!(matches!(
        &pushed.entries[0],
        Entry::Update { note_key: Some(k), .. } if *k == key(1)
    ));
    // The same key again is fine; a different one is refused.
    f.push(&[upd(2, 1, b"b")]).await.unwrap();
    let mut bad = keyless(3, 1, b"c");
    bad.note_key = Some(Bytes(b"other".to_vec()));
    assert_eq!(
        f.push(&[bad]).await.unwrap_err(),
        SyncError::NOTE_KEY_CONFLICT
    );
    let stored: Vec<u8> = sqlx::query_scalar("SELECT wrapped_key FROM sync_notes")
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert_eq!(stored, key(1).0);
}

#[tokio::test]
async fn pushes_for_a_purged_note_are_refused() {
    let f = fixture().await;
    f.push(&[upd(1, 1, b"a")]).await.unwrap();
    purge(&f.pool, &VAULT, note(1)).await.unwrap();
    for update in [keyless(2, 1, b"b"), upd(3, 1, b"c")] {
        assert_eq!(f.push(&[update]).await.unwrap_err(), SyncError::NOTE_PURGED);
    }
}

#[tokio::test]
async fn size_limit_is_inclusive() {
    let f = fixture().await;
    f.push(&[upd(1, 1, &[0; MAX])]).await.unwrap();
    assert_eq!(
        f.push(&[keyless(2, 1, b"ok"), keyless(3, 1, &[0; MAX + 1])])
            .await
            .unwrap_err(),
        SyncError::UPDATE_TOO_LARGE
    );
    assert_eq!(f.count("SELECT count(*) FROM ops").await, 1);
}

#[tokio::test]
async fn empty_and_duplicate_batches_are_bad_requests() {
    let f = fixture().await;
    assert_eq!(f.push(&[]).await.unwrap_err(), SyncError::BAD_REQUEST);
    assert_eq!(
        f.push(&[upd(1, 1, b"a"), upd(1, 1, b"a")])
            .await
            .unwrap_err(),
        SyncError::BAD_REQUEST
    );
    assert_eq!(f.head().await, 0);
}

#[tokio::test]
async fn failure_mid_batch_stores_nothing() {
    let f = fixture().await;
    f.push(&[upd(1, 1, b"a")]).await.unwrap();
    let head = f.head().await;
    // A new note registers, then a later update in the batch fails.
    let err = f
        .push(&[upd(2, 2, b"b"), keyless(3, 1, b"c"), keyless(4, 3, b"d")])
        .await
        .unwrap_err();
    assert_eq!(err, SyncError::MISSING_NOTE_KEY);
    assert_eq!(f.head().await, head);
    assert_eq!(f.count("SELECT count(*) FROM ops").await, 1);
    assert_eq!(f.count("SELECT count(*) FROM sync_notes").await, 1);
}

// --- snapshot ---

/// Note 1 with updates at 3, 5, and 9; other notes fill the gaps.
async fn three_five_nine(f: &Fixture) {
    f.push(&[upd(1, 2, b"x"), keyless(2, 2, b"x")])
        .await
        .unwrap();
    assert_eq!(f.seq(upd(3, 1, b"n3")).await, 3);
    f.seq(keyless(4, 2, b"x")).await;
    assert_eq!(f.seq(keyless(5, 1, b"n5")).await, 5);
    f.push(&[
        keyless(6, 2, b"x"),
        keyless(7, 2, b"x"),
        keyless(8, 2, b"x"),
    ])
    .await
    .unwrap();
    assert_eq!(f.seq(keyless(9, 1, b"n9")).await, 9);
}

async fn note_seqs(f: &Fixture, n: u128) -> Vec<i64> {
    sqlx::query_scalar("SELECT seq FROM ops WHERE note_id = ? ORDER BY seq")
        .bind(note(n).to_bytes().as_slice())
        .fetch_all(&f.pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn snapshot_compacts_covered_updates() {
    let f = fixture().await;
    three_five_nine(&f).await;
    let others = note_seqs(&f, 2).await;
    let delta = snapshot(&f.pool, &VAULT, note(1), 5, b"snap-5")
        .await
        .unwrap();
    assert_eq!(delta, 6 - 4);
    assert_eq!(note_seqs(&f, 1).await, [9]);
    assert_eq!(note_seqs(&f, 2).await, others, "other notes untouched");
    assert_eq!(f.head().await, 9, "snapshots take no sequence number");

    let delta = snapshot(&f.pool, &VAULT, note(1), 9, b"s9").await.unwrap();
    assert_eq!(delta, 2 - 6 - 2);
    assert!(note_seqs(&f, 1).await.is_empty());
    assert_eq!(
        f.count("SELECT count(*) FROM snapshots WHERE covers = 9")
            .await,
        1
    );
    assert_eq!(f.count("SELECT count(*) FROM snapshots").await, 1);
}

#[tokio::test]
async fn stale_and_equal_snapshots_are_refused() {
    let f = fixture().await;
    three_five_nine(&f).await;
    snapshot(&f.pool, &VAULT, note(1), 5, b"s5").await.unwrap();
    for covers in [3, 5] {
        assert_eq!(
            snapshot(&f.pool, &VAULT, note(1), covers, b"old")
                .await
                .unwrap_err(),
            SyncError::SNAPSHOT_STALE
        );
    }
    let stored: Vec<u8> = sqlx::query_scalar("SELECT payload FROM snapshots")
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert_eq!(stored, b"s5");
}

#[tokio::test]
async fn snapshot_bounds_are_checked() {
    let f = fixture().await;
    three_five_nine(&f).await;
    // Above the head, at zero, and before the note was registered (3).
    for covers in [10, 0, 2] {
        assert_eq!(
            snapshot(&f.pool, &VAULT, note(1), covers, b"s")
                .await
                .unwrap_err(),
            SyncError::BAD_REQUEST,
            "{covers}"
        );
    }
    assert_eq!(note_seqs(&f, 1).await, [3, 5, 9]);
    assert_eq!(
        snapshot(&f.pool, &VAULT, note(7), 5, b"s")
            .await
            .unwrap_err(),
        SyncError::UNKNOWN_NOTE
    );
    purge(&f.pool, &VAULT, note(2)).await.unwrap();
    assert_eq!(
        snapshot(&f.pool, &VAULT, note(2), 5, b"s")
            .await
            .unwrap_err(),
        SyncError::NOTE_PURGED
    );
    assert_eq!(f.count("SELECT count(*) FROM snapshots").await, 0);
}

// --- purge ---

#[tokio::test]
async fn purge_removes_data_and_leaves_a_tombstone() {
    let f = fixture().await;
    three_five_nine(&f).await;
    snapshot(&f.pool, &VAULT, note(1), 5, b"s5").await.unwrap();
    let purged = purge(&f.pool, &VAULT, note(1)).await.unwrap();
    assert_eq!(
        purged.entry,
        Some(Entry::Purge {
            seq: 10,
            note: note(1)
        })
    );
    assert_eq!(purged.delta, -(2 + 2));
    assert_eq!(f.head().await, 10);
    assert!(note_seqs(&f, 1).await.is_empty());
    assert_eq!(f.count("SELECT count(*) FROM snapshots").await, 0);
    let row: (Option<Vec<u8>>, Option<i64>) =
        sqlx::query_as("SELECT wrapped_key, purged_seq FROM sync_notes WHERE note_id = ?")
            .bind(note(1).to_bytes().as_slice())
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_eq!(row, (None, Some(10)));

    let again = purge(&f.pool, &VAULT, note(1)).await.unwrap();
    assert_eq!(again.entry, None);
    assert_eq!(again.delta, 0);
    assert_eq!(f.head().await, 10);

    assert_eq!(
        f.push(&[keyless(20, 1, b"back")]).await.unwrap_err(),
        SyncError::NOTE_PURGED
    );
    assert_eq!(
        purge(&f.pool, &VAULT, note(99)).await.unwrap_err(),
        SyncError::UNKNOWN_NOTE
    );
}

// --- quota ---

async fn set_quota(pool: &SqlitePool, vault: [u8; 16], quota: i64) {
    sqlx::query("UPDATE vaults SET quota_bytes = ? WHERE vault_id = ?")
        .bind(quota)
        .bind(vault.as_slice())
        .execute(pool)
        .await
        .unwrap();
}

async fn used(pool: &SqlitePool, vault: [u8; 16]) -> i64 {
    sqlx::query_scalar("SELECT used_bytes FROM vaults WHERE vault_id = ?")
        .bind(vault.as_slice())
        .fetch_one(pool)
        .await
        .unwrap()
}

/// The vault's payload bytes, recomputed from what is stored.
async fn stored(pool: &SqlitePool, vault: [u8; 16]) -> i64 {
    sqlx::query_scalar(
        "SELECT (SELECT coalesce(sum(length(payload)), 0) FROM ops WHERE vault_id = ?1) \
              + (SELECT coalesce(sum(length(payload)), 0) FROM snapshots WHERE vault_id = ?1)",
    )
    .bind(vault.as_slice())
    .fetch_one(pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn pushes_over_quota_store_nothing() {
    let f = fixture().await;
    set_quota(&f.pool, VAULT, 100).await;
    f.push(&[upd(1, 1, &[0; 60])]).await.unwrap();
    assert_eq!(used(&f.pool, VAULT).await, 60);
    for over in [
        vec![upd(2, 2, &[0; 41])],
        vec![keyless(2, 1, &[0; 20]), upd(3, 2, &[0; 21])],
    ] {
        assert_eq!(f.push(&over).await.unwrap_err(), SyncError::QUOTA_EXCEEDED);
    }
    assert_eq!(f.head().await, 1);
    assert_eq!(f.count("SELECT count(*) FROM ops").await, 1);
    assert_eq!(f.count("SELECT count(*) FROM sync_notes").await, 1);
    assert_eq!(used(&f.pool, VAULT).await, 60);

    // Exactly to the quota is allowed; a retry adds nothing.
    let at = f.push(&[keyless(2, 1, &[0; 40])]).await.unwrap();
    assert_eq!(used(&f.pool, VAULT).await, 100);
    let retry = f.push(&[keyless(2, 1, &[0; 40])]).await.unwrap();
    assert_eq!(retry.seqs, at.seqs);
    assert_eq!(used(&f.pool, VAULT).await, 100);
}

#[tokio::test]
async fn compaction_and_purge_work_at_or_over_quota() {
    let f = fixture().await;
    f.push(&[
        upd(1, 1, &[0; 60]),
        keyless(2, 1, &[0; 40]),
        upd(3, 2, &[0; 10]),
    ])
    .await
    .unwrap();
    set_quota(&f.pool, VAULT, 110).await;

    // Covering only the first update with a larger snapshot grows the vault.
    assert_eq!(
        snapshot(&f.pool, &VAULT, note(1), 1, &[0; 70])
            .await
            .unwrap_err(),
        SyncError::QUOTA_EXCEEDED
    );
    assert_eq!(note_seqs(&f, 1).await, [1, 2]);
    assert_eq!(f.count("SELECT count(*) FROM snapshots").await, 0);
    assert_eq!(used(&f.pool, VAULT).await, 110);

    // Covering both shrinks it, at quota.
    let delta = snapshot(&f.pool, &VAULT, note(1), 2, &[0; 30])
        .await
        .unwrap();
    assert_eq!(delta, -70);
    assert_eq!(used(&f.pool, VAULT).await, 40);

    // Lowered below usage: growth is refused, purges are not.
    set_quota(&f.pool, VAULT, 10).await;
    assert_eq!(
        f.push(&[keyless(4, 2, b"x")]).await.unwrap_err(),
        SyncError::QUOTA_EXCEEDED
    );
    assert_eq!(purge(&f.pool, &VAULT, note(1)).await.unwrap().delta, -30);
    assert_eq!(used(&f.pool, VAULT).await, 10);
    f.push(&[keyless(4, 2, b"")]).await.unwrap();
    assert_eq!(
        f.push(&[keyless(5, 2, b"x")]).await.unwrap_err(),
        SyncError::QUOTA_EXCEEDED
    );
}

#[tokio::test]
async fn usage_stays_exact_across_vaults() {
    let f = fixture().await;
    const OTHER: [u8; 16] = [3; 16];
    add_vault(&f.pool, 2, OTHER).await;
    let check = || async {
        for vault in [VAULT, OTHER] {
            assert_eq!(used(&f.pool, vault).await, stored(&f.pool, vault).await);
        }
    };
    for (i, vault) in [VAULT, OTHER].into_iter().enumerate() {
        let i = i as u128 * 100;
        push(
            &f.pool,
            &vault,
            &[upd(i + 1, 1, b"aaaa"), upd(i + 2, 2, b"bbbbbbbb")],
            MAX,
        )
        .await
        .unwrap();
        check().await;
        push(&f.pool, &vault, &[keyless(i + 3, 1, b"cc")], MAX)
            .await
            .unwrap();
        // A retry stores nothing new.
        push(&f.pool, &vault, &[keyless(i + 3, 1, b"cc")], MAX)
            .await
            .unwrap();
        check().await;
        snapshot(&f.pool, &vault, note(1), 1, b"s1").await.unwrap();
        check().await;
        // Replaces the first snapshot and compacts the rest of note 1.
        snapshot(&f.pool, &vault, note(1), 3, b"snapshot-3")
            .await
            .unwrap();
        check().await;
        push(&f.pool, &vault, &[keyless(i + 4, 2, b"dddd")], MAX)
            .await
            .unwrap();
        check().await;
    }
    purge(&f.pool, &VAULT, note(2)).await.unwrap();
    check().await;
    purge(&f.pool, &OTHER, note(1)).await.unwrap();
    check().await;
    assert_eq!(used(&f.pool, VAULT).await, 10);
    assert_eq!(used(&f.pool, OTHER).await, 12);
}

// --- changes ---

#[tokio::test]
async fn incremental_pull() {
    let f = fixture().await;
    f.push(&[upd(1, 1, b"a")]).await.unwrap();
    let cursor = f.pull(0).await.cursor;
    let u1 = f.seq(keyless(2, 1, b"u1")).await;
    let u2 = f.seq(keyless(3, 1, b"u2")).await;
    let page = f.pull(cursor).await;
    assert_eq!(update_seqs(&page), [u1, u2]);
    assert!(
        page.notes.is_empty(),
        "the note was registered before the cursor"
    );
    assert!(page.cursor >= u2);
    assert!(!page.more);
    assert!(matches!(
        &page.entries[0],
        Entry::Update { id, data, .. } if *id == op(2) && data.0 == b"u1"
    ));
}

#[tokio::test]
async fn nothing_new() {
    let f = fixture().await;
    f.push(&[upd(1, 1, b"a")]).await.unwrap();
    let head = f.head().await;
    let page = f.pull(head).await;
    assert!(page.notes.is_empty() && page.entries.is_empty());
    assert_eq!(page.cursor, head);
    assert!(!page.more);
}

#[tokio::test]
async fn new_notes_bring_their_keys() {
    let f = fixture().await;
    f.push(&[upd(1, 1, b"a")]).await.unwrap();
    let cursor = f.head().await;
    f.push(&[upd(2, 2, b"b")]).await.unwrap();
    let page = f.pull(cursor).await;
    assert_eq!(
        page.notes,
        [NoteEntry {
            note: note(2),
            key: Some(key(2)),
            snapshot: None
        }]
    );
    assert_eq!(page.entries.len(), 1);
}

#[tokio::test]
async fn client_behind_and_past_a_compaction() {
    let f = fixture().await;
    three_five_nine(&f).await;
    snapshot(&f.pool, &VAULT, note(1), 5, b"s5").await.unwrap();

    // Behind: the snapshot replaces 3 and 5; 9 follows.
    let page = f.pull(4).await;
    let n1: Vec<&NoteEntry> = page.notes.iter().filter(|n| n.note == note(1)).collect();
    assert_eq!(
        n1[0].snapshot,
        Some(Snapshot {
            covers: 5,
            data: Bytes(b"s5".to_vec())
        })
    );
    assert_eq!(n1[0].key, None, "registered before the cursor");
    let seqs: Vec<u64> = page
        .entries
        .iter()
        .filter(|e| matches!(e, Entry::Update { note: n, .. } if *n == note(1)))
        .map(Entry::seq)
        .collect();
    assert_eq!(seqs, [9]);

    // Past: no snapshot.
    let page = f.pull(5).await;
    assert!(page.notes.is_empty());
    assert_eq!(update_seqs(&page), [6, 7, 8, 9]);
}

#[tokio::test]
async fn bootstrap_is_full_state_without_purged_notes() {
    let f = fixture().await;
    three_five_nine(&f).await;
    snapshot(&f.pool, &VAULT, note(1), 5, b"s5").await.unwrap();
    f.push(&[upd(10, 3, b"gone")]).await.unwrap();
    purge(&f.pool, &VAULT, note(3)).await.unwrap();

    let page = f.pull(0).await;
    assert!(!page.more);
    assert_eq!(page.cursor, f.head().await);
    let by_note: HashMap<NoteId, &NoteEntry> = page.notes.iter().map(|n| (n.note, n)).collect();
    assert_eq!(by_note.len(), 2);
    assert_eq!(by_note[&note(1)].key, Some(key(1)));
    assert_eq!(by_note[&note(1)].snapshot.as_ref().unwrap().covers, 5);
    assert_eq!(by_note[&note(2)].key, Some(key(2)));
    assert_eq!(by_note[&note(2)].snapshot, None);
    assert_eq!(update_seqs(&page), [1, 2, 4, 6, 7, 8, 9]);
    assert!(page.entries.iter().all(|e| match e {
        Entry::Update { note: n, .. } => *n != note(3),
        Entry::Purge { note: n, seq } => *n == note(3) && *seq == 11,
    }));
}

#[tokio::test]
async fn cursor_ahead_is_refused() {
    let f = fixture().await;
    f.push(&[upd(1, 1, b"a")]).await.unwrap();
    assert_eq!(
        changes(&f.pool, &VAULT, 2).await.unwrap_err(),
        SyncError::CURSOR_AHEAD
    );
    assert_eq!(SyncError::CURSOR_AHEAD.0, ErrorCode::CursorAhead);
}

/// Pages from `after` until `more` is false.
async fn pull_all(f: &Fixture, after: u64, budget: usize) -> Vec<ChangesResponse> {
    let mut pages = Vec::new();
    let mut cursor = after;
    loop {
        let page = changes_with_budget(&f.pool, &VAULT, cursor, budget)
            .await
            .unwrap();
        assert!(page.cursor > cursor || !page.more, "no progress");
        cursor = page.cursor;
        let more = page.more;
        pages.push(page);
        if !more {
            return pages;
        }
    }
}

#[tokio::test]
async fn multi_page_bootstrap_yields_everything_once() {
    let f = fixture().await;
    for n in 0..20u128 {
        f.push(&[
            upd(n * 2, n % 4, &[n as u8; 40]),
            keyless(n * 2 + 1, n % 4, b"x"),
        ])
        .await
        .unwrap();
        if n % 5 == 4 {
            let head = f.head().await;
            snapshot(&f.pool, &VAULT, note(n % 4), head, b"snapshot")
                .await
                .unwrap();
        }
    }
    let pages = pull_all(&f, 0, 400).await;
    assert!(pages.len() > 3, "{}", pages.len());
    let single = f.pull(0).await;

    let mut seqs = Vec::new();
    let mut notes = Vec::new();
    for page in &pages {
        seqs.extend(page.entries.iter().map(Entry::seq));
        notes.extend(page.notes.iter().cloned());
    }
    let mut sorted = seqs.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(seqs, sorted, "ascending and unique");
    assert_eq!(
        seqs,
        single.entries.iter().map(Entry::seq).collect::<Vec<_>>()
    );
    let mut paged = notes.clone();
    paged.sort_by_key(|n| n.note);
    let mut whole = single.notes.clone();
    whole.sort_by_key(|n| n.note);
    // A note's key and snapshot may arrive on different pages.
    let merge = |list: Vec<NoteEntry>| {
        let mut out: BTreeMap<NoteId, (Option<Bytes>, Option<Snapshot>)> = BTreeMap::new();
        for n in list {
            let e = out.entry(n.note).or_default();
            assert!(n.key.is_none() || e.0.is_none(), "key twice");
            assert!(n.snapshot.is_none() || e.1.is_none(), "snapshot twice");
            e.0 = e.0.take().or(n.key);
            e.1 = e.1.take().or(n.snapshot);
        }
        out
    };
    assert_eq!(merge(paged), merge(whole));
}

#[tokio::test]
async fn oversize_item_comes_alone() {
    let f = fixture().await;
    f.push(&[upd(1, 1, b"a")]).await.unwrap();
    let start = f.head().await;
    f.push(&[keyless(2, 1, &[7; 900]), keyless(3, 1, b"b")])
        .await
        .unwrap();
    let page = changes_with_budget(&f.pool, &VAULT, start, 100)
        .await
        .unwrap();
    assert_eq!(update_seqs(&page), [start + 1]);
    assert!(page.more);
    assert_eq!(page.cursor, start + 1);
    let page = changes_with_budget(&f.pool, &VAULT, page.cursor, 100)
        .await
        .unwrap();
    assert_eq!(update_seqs(&page), [start + 2]);
    assert!(!page.more);
}

#[tokio::test]
async fn vaults_are_isolated() {
    let f = fixture().await;
    let other = [3u8; 16];
    add_vault(&f.pool, 2, other).await;
    f.push(&[upd(1, 1, b"mine")]).await.unwrap();
    push(&f.pool, &other, &[upd(1, 1, b"theirs")], MAX)
        .await
        .unwrap();
    let page = f.pull(0).await;
    assert_eq!(page.entries.len(), 1);
    assert!(matches!(&page.entries[0], Entry::Update { data, .. } if data.0 == b"mine"));
}

// --- property: random interleavings ---

/// A small deterministic generator (SplitMix64), so failures reproduce.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

/// What the test knows independently of the store.
#[derive(Default)]
struct Model {
    /// Every update ever stored: seq → note.
    history: BTreeMap<u64, u128>,
    registered: HashMap<u128, u64>,
    purged: HashMap<u128, u64>,
    next_op: u128,
}

async fn random_write(f: &Fixture, rng: &mut Rng, model: &mut Model) {
    let n = u128::from(rng.below(6));
    match rng.below(10) {
        0..=6 => {
            let count = 1 + rng.below(3);
            let updates: Vec<PushUpdate> = (0..count)
                .map(|_| {
                    model.next_op += 1;
                    let size = rng.below(120) as usize;
                    upd(model.next_op, n, &vec![model.next_op as u8; size])
                })
                .collect();
            if let Ok(pushed) = f.push(&updates).await {
                for seq in pushed.seqs {
                    model.history.insert(seq, n);
                }
                model.registered.entry(n).or_insert(pushed.entries[0].seq());
            }
        }
        7 | 8 => {
            let head = f.head().await;
            let covers = 1 + rng.below(head.max(1));
            let _ = snapshot(&f.pool, &VAULT, note(n), covers, b"snapshot").await;
        }
        _ => {
            if let Ok(Purged {
                entry: Some(entry), ..
            }) = purge(&f.pool, &VAULT, note(n)).await
            {
                model.purged.insert(n, entry.seq());
            }
        }
    }
}

#[tokio::test]
async fn random_interleavings_never_leave_gaps() {
    for trial in 0..12u64 {
        let f = fixture().await;
        let mut rng = Rng(trial);
        let mut model = Model::default();
        for _ in 0..40 {
            random_write(&f, &mut rng, &mut model).await;
        }

        for _ in 0..4 {
            let head = f.head().await;
            let start = rng.below(head + 1);
            let budget = 100 + rng.below(400) as usize;

            // Page through, with more writes landing between pages.
            let mut updates: HashSet<u64> = HashSet::new();
            let mut snapshots: HashMap<NoteId, u64> = HashMap::new();
            let mut keys: HashSet<NoteId> = HashSet::new();
            let mut purges: HashSet<NoteId> = HashSet::new();
            let mut cursor = start;
            loop {
                let page = changes_with_budget(&f.pool, &VAULT, cursor, budget)
                    .await
                    .unwrap();
                for n in &page.notes {
                    if n.key.is_some() {
                        assert!(keys.insert(n.note), "trial {trial}: key twice");
                    }
                    if let Some(s) = &n.snapshot {
                        assert!(s.covers > cursor);
                        let best = snapshots.entry(n.note).or_default();
                        *best = (*best).max(s.covers);
                    }
                }
                for e in &page.entries {
                    assert!(e.seq() > cursor && e.seq() <= page.cursor);
                    match e {
                        Entry::Update { seq, note: n, .. } => {
                            assert!(updates.insert(*seq), "trial {trial}: seq {seq} twice");
                            let unpurged = !model.purged.contains_key(&(n_of(*n)));
                            if unpurged && model.registered[&n_of(*n)] > start {
                                assert!(keys.contains(n), "trial {trial}: data before key");
                            }
                        }
                        Entry::Purge { note: n, .. } => {
                            purges.insert(*n);
                        }
                    }
                }
                cursor = page.cursor;
                if !page.more {
                    break;
                }
                if rng.below(3) == 0 {
                    random_write(&f, &mut rng, &mut model).await;
                }
            }

            for (&seq, &n) in model.history.range(start + 1..) {
                if model.purged.contains_key(&n) {
                    continue;
                }
                let covered = snapshots.get(&note(n)).is_some_and(|&c| c >= seq);
                assert!(
                    updates.contains(&seq) || covered,
                    "trial {trial}: from {start}, update {seq} of note {n} missing"
                );
            }
            for (&n, &created) in &model.registered {
                if created > start && !model.purged.contains_key(&n) {
                    assert!(keys.contains(&note(n)), "trial {trial}: key of {n} missing");
                }
            }
            for (&n, &seq) in &model.purged {
                if seq > start {
                    assert!(
                        purges.contains(&note(n)),
                        "trial {trial}: purge of {n} missing"
                    );
                }
            }
        }
    }
}

fn n_of(id: NoteId) -> u128 {
    (0..6).find(|&n| note(n) == id).unwrap()
}

#[tokio::test]
async fn no_seq_reuse_after_compacting_through_the_head() {
    let f = fixture().await;
    f.push(&[upd(1, 1, b"a"), keyless(2, 1, b"b")])
        .await
        .unwrap();
    snapshot(&f.pool, &VAULT, note(1), 2, b"s").await.unwrap();
    assert_eq!(f.count("SELECT count(*) FROM ops").await, 0);
    assert_eq!(f.seq(keyless(3, 1, b"c")).await, 3);
    purge(&f.pool, &VAULT, note(1)).await.unwrap();
    assert_eq!(f.seq(upd(4, 2, b"d")).await, 5, "the purge took 4");
}

#[tokio::test]
async fn batch_size_and_key_length_are_bounded() {
    let f = fixture().await;
    let too_many: Vec<PushUpdate> = (0..=MAX_PUSH_UPDATES as u128)
        .map(|o| upd(o, 1, b""))
        .collect();
    assert_eq!(f.push(&too_many).await.unwrap_err(), SyncError::BAD_REQUEST);
    f.push(&too_many[..MAX_PUSH_UPDATES]).await.unwrap();

    let mut big_key = upd(5000, 2, b"x");
    big_key.note_key = Some(Bytes(vec![0; MAX_NOTE_KEY_BYTES + 1]));
    assert_eq!(
        f.push(&[big_key]).await.unwrap_err(),
        SyncError::BAD_REQUEST
    );
    let mut ok_key = upd(5001, 2, b"x");
    ok_key.note_key = Some(Bytes(vec![0; MAX_NOTE_KEY_BYTES]));
    f.push(&[ok_key]).await.unwrap();
}

#[tokio::test]
async fn pages_span_windows_and_gaps() {
    let f = fixture().await;
    // Note 1 gets 2,500 updates, mostly compacted away, so past the first
    // sizing window the keys are sparse; note 2 gets one per round.
    let mut o = 0u128;
    for round in 0..5u8 {
        for _ in 0..5 {
            let batch: Vec<PushUpdate> = (0..100)
                .map(|_| {
                    o += 1;
                    upd(o, 1, b"n1")
                })
                .collect();
            f.push(&batch).await.unwrap();
        }
        o += 1;
        f.push(&[upd(o, 2, &[round; 30])]).await.unwrap();
        if round % 2 == 0 {
            let head = f.head().await;
            let data = format!("s{round}");
            snapshot(&f.pool, &VAULT, note(1), head - 1, data.as_bytes())
                .await
                .unwrap();
        }
    }
    let head = f.head().await;
    assert!(head > 2 * 1024);

    let covers = |page: &ChangesResponse| -> Vec<u64> {
        page.notes
            .iter()
            .filter_map(|n| n.snapshot.as_ref())
            .map(|s| s.covers)
            .collect()
    };
    for start in [0, 7, 1500, head - 3] {
        let single = f.pull(start).await;
        assert!(!single.more);
        let pages = pull_all(&f, start, 2_000).await;
        let seqs: Vec<u64> = pages
            .iter()
            .flat_map(|p| p.entries.iter().map(Entry::seq))
            .collect();
        let expected: Vec<u64> = single.entries.iter().map(Entry::seq).collect();
        assert_eq!(seqs, expected, "from {start}");
        let paged: Vec<u64> = pages.iter().flat_map(covers).collect();
        assert_eq!(paged, covers(&single), "from {start}");
        assert_eq!(pages.last().unwrap().cursor, head);
    }
}
