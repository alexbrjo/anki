// Copyright: Ankitects Pty Ltd and contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

use super::session::SessionInfo;
use super::session::SessionKind;
use crate::prelude::*;
use crate::tests::NoteAdder;

fn dolt_log_len(col: &Collection) -> usize {
    col.storage
        .db
        .prepare("SELECT COUNT(*) FROM dolt_log")
        .unwrap()
        .query_row([], |r| r.get::<_, i64>(0))
        .unwrap() as usize
}

fn newest_commit(col: &Collection) -> (String, String, String) {
    col.storage
        .db
        .prepare("SELECT commit_hash, committer, message FROM dolt_log ORDER BY date DESC LIMIT 1")
        .unwrap()
        .query_row([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
}

fn session(author: &str) -> SessionInfo {
    SessionInfo {
        id: "deadbeefdeadbeefdeadbeefdeadbeef".to_string(),
        kind: SessionKind::Editor,
        author: author.to_string(),
    }
}

#[test]
fn commit_records_edits() {
    let mut col = Collection::new();
    let note = NoteAdder::basic(&mut col)
        .fields(&["front", "back"])
        .add(&mut col);
    col.mark_note_added_for_session(&session("human").id, note.id)
        .unwrap();

    let before = dolt_log_len(&col);
    let handle = col.begin_versioning_session(session("human"));
    let hash = col.commit_versioning_session(handle).unwrap();
    assert!(hash.is_some(), "expected a commit hash after note add");
    assert_eq!(dolt_log_len(&col), before + 1);

    let (commit_hash, committer, message) = newest_commit(&col);
    assert_eq!(commit_hash, hash.unwrap());
    assert_eq!(committer, "human");
    assert!(
        message.contains("\"session\":\"deadbeef"),
        "commit message should carry session metadata: {message}"
    );
    assert!(message.contains("\"kind\":\"editor\""));
    assert!(message.contains("\"actor\":\"human\""));
}

#[test]
fn snapshot_skips_commit_when_content_unchanged() {
    // This is the regression test for the "phantom commits" bug: opening
    // an editor session and closing it without touching the content should
    // produce zero commits, even if Anki rewrites the row internally (e.g.
    // HTML normalization on save).
    let mut col = Collection::new();
    let note = NoteAdder::basic(&mut col)
        .fields(&["unchanged", "back"])
        .add(&mut col);
    col.mark_note_added_for_session(&session("human").id, note.id)
        .unwrap();

    // Flush the add as a first commit so we have a baseline.
    let h = col.begin_versioning_session(session("human"));
    col.commit_versioning_session(h).unwrap().unwrap();

    let before = dolt_log_len(&col);
    // Simulate "open editor, do nothing, close editor" three times in a
    // row — exactly the scenario that produced 3 phantom commits.
    for _ in 0..3 {
        let sid = "0000000000000000000000000000000a".to_string();
        col.snapshot_note_for_session(&sid, note.id).unwrap();
        let info = SessionInfo {
            id: sid,
            kind: SessionKind::Editor,
            author: "human".to_string(),
        };
        let h = col.begin_versioning_session(info);
        let hash = col.commit_versioning_session(h).unwrap();
        assert!(
            hash.is_none(),
            "no-op editor session must not produce a commit"
        );
    }
    assert_eq!(dolt_log_len(&col), before, "expected no new commits");
}

#[test]
fn snapshot_commits_when_content_changes() {
    let mut col = Collection::new();
    let mut note = NoteAdder::basic(&mut col)
        .fields(&["before", "back"])
        .add(&mut col);
    col.mark_note_added_for_session(&session("human").id, note.id)
        .unwrap();
    let h = col.begin_versioning_session(session("human"));
    col.commit_versioning_session(h).unwrap().unwrap();

    let before = dolt_log_len(&col);
    let sid = "ffffffffffffffffffffffffffffffff".to_string();
    col.snapshot_note_for_session(&sid, note.id).unwrap();
    note.fields_mut()[0] = "after".to_string();
    col.update_note(&mut note).unwrap();
    let info = SessionInfo {
        id: sid,
        kind: SessionKind::Editor,
        author: "human".to_string(),
    };
    let h = col.begin_versioning_session(info);
    let hash = col.commit_versioning_session(h).unwrap();
    assert!(hash.is_some(), "real edit should produce a commit");
    assert_eq!(dolt_log_len(&col), before + 1);
}

#[test]
fn clean_session_makes_no_commit() {
    let mut col = Collection::new();
    let note = NoteAdder::basic(&mut col)
        .fields(&["front", "back"])
        .add(&mut col);
    col.mark_note_added_for_session(&session("human").id, note.id)
        .unwrap();

    // First commit to flush the add.
    let h = col.begin_versioning_session(session("human"));
    col.commit_versioning_session(h).unwrap();

    let before = dolt_log_len(&col);
    // Open + close a session with no mutations between.
    let h = col.begin_versioning_session(session("human"));
    let hash = col.commit_versioning_session(h).unwrap();
    assert!(hash.is_none(), "expected no commit when notes are clean");
    assert_eq!(dolt_log_len(&col), before);
}

#[test]
fn list_note_versions_returns_newest_first() {
    let mut col = Collection::new();
    let mut note = NoteAdder::basic(&mut col).fields(&["one", "back"]).note();
    col.add_note(&mut note, crate::decks::DeckId(1)).unwrap();
    col.mark_note_added_for_session(&session("human").id, note.id)
        .unwrap();
    let h = col.begin_versioning_session(session("human"));
    col.commit_versioning_session(h).unwrap().unwrap();

    col.snapshot_note_for_session(&session("human").id, note.id)
        .unwrap();
    note.fields_mut()[0] = "two".to_string();
    col.update_note(&mut note).unwrap();
    let h = col.begin_versioning_session(session("human"));
    col.commit_versioning_session(h).unwrap().unwrap();

    col.snapshot_note_for_session(&session("agent:bot").id, note.id)
        .unwrap();
    note.fields_mut()[1] = "back2".to_string();
    col.update_note(&mut note).unwrap();
    let h = col.begin_versioning_session(session("agent:bot"));
    col.commit_versioning_session(h).unwrap().unwrap();

    let versions = col.list_note_versions(note.id).unwrap();
    assert_eq!(versions.len(), 3, "expected 3 commits in history");
    // Newest first: the third commit was the agent edit of Back.
    assert_eq!(versions[0].author, "agent:bot");
    assert_eq!(versions[0].changed_fields, vec!["Back".to_string()]);
    // Middle commit changed Front.
    assert_eq!(versions[1].author, "human");
    assert_eq!(versions[1].changed_fields, vec!["Front".to_string()]);
    // Oldest commit (the initial add) has no prior to diff against.
    assert!(versions[2].changed_fields.is_empty());
    assert!(versions[0].timestamp_secs >= versions[1].timestamp_secs);
}

#[test]
fn restore_writes_a_new_commit_and_keeps_in_betweens() {
    let mut col = Collection::new();
    let mut note = NoteAdder::basic(&mut col).fields(&["v1", "back"]).note();
    col.add_note(&mut note, crate::decks::DeckId(1)).unwrap();
    col.mark_note_added_for_session(&session("human").id, note.id)
        .unwrap();
    let h = col.begin_versioning_session(session("human"));
    col.commit_versioning_session(h).unwrap().unwrap();

    col.snapshot_note_for_session(&session("agent:rogue").id, note.id)
        .unwrap();
    note.fields_mut()[0] = "v2".to_string();
    col.update_note(&mut note).unwrap();
    let h = col.begin_versioning_session(session("agent:rogue"));
    col.commit_versioning_session(h).unwrap().unwrap();

    let versions_before = col.list_note_versions(note.id).unwrap();
    assert_eq!(versions_before.len(), 2);
    let v1_hash = versions_before[1].commit_hash.clone();

    // Restore to v1.
    let restore_session = session("human");
    let new_hash = col
        .restore_note_version(note.id, &v1_hash, restore_session)
        .unwrap()
        .commit_hash
        .expect("restore should produce a new commit");

    // Note content reverted.
    let restored = col.storage.get_note(note.id).unwrap().unwrap();
    assert_eq!(restored.fields()[0], "v1");

    // History now has three commits and the v2 commit is still there.
    let versions_after = col.list_note_versions(note.id).unwrap();
    assert_eq!(
        versions_after.len(),
        3,
        "expected revert to append, not reset"
    );
    assert_eq!(versions_after[0].commit_hash, new_hash);
    let hashes: Vec<_> = versions_after
        .iter()
        .map(|v| v.commit_hash.as_str())
        .collect();
    assert!(hashes.contains(&versions_before[0].commit_hash.as_str()));
    assert!(hashes.contains(&v1_hash.as_str()));
}

#[test]
fn agent_author_round_trips() {
    let mut col = Collection::new();
    let note = NoteAdder::basic(&mut col).fields(&["f", "b"]).add(&mut col);
    col.mark_note_added_for_session(&session("agent:test-bot").id, note.id)
        .unwrap();

    let h = col.begin_versioning_session(session("agent:test-bot"));
    col.commit_versioning_session(h).unwrap().unwrap();

    let (_, committer, _) = newest_commit(&col);
    assert_eq!(committer, "agent:test-bot");
}

// =============================================================================
// Regression tests for known correctness/robustness issues from the code
// review. Tests marked #[ignore] currently fail and document the desired
// post-fix behavior; run them with `cargo test -- --ignored versioning`.
// =============================================================================

/// Issue 2 — qt/aqt/addcards.py calls commit_session without a prior
/// snapshot. The backend used to fall back to a connection-wide
/// `dolt_status` check, folding unrelated dirty rows into the addcards
/// commit and mis-attributing them. Fixed by requiring a snapshot.
#[test]
fn commit_without_snapshot_does_not_steal_unrelated_dirt() {
    let mut col = Collection::new();
    let mut note_a = NoteAdder::basic(&mut col).fields(&["A", "back"]).note();
    col.add_note(&mut note_a, crate::decks::DeckId(1)).unwrap();
    col.mark_note_added_for_session(&session("human").id, note_a.id)
        .unwrap();
    let h = col.begin_versioning_session(session("human"));
    col.commit_versioning_session(h).unwrap().unwrap();

    // Editor for note A edits it but never closes (no commit_session yet).
    note_a.fields_mut()[0] = "A-edited".to_string();
    col.update_note(&mut note_a).unwrap();

    // Meanwhile, addcards adds a brand-new note B and calls commit_session
    // without a snapshot — exactly the codepath in qt/aqt/addcards.py.
    let mut note_b = NoteAdder::basic(&mut col).fields(&["B", "back"]).note();
    col.add_note(&mut note_b, crate::decks::DeckId(1)).unwrap();
    let info = SessionInfo {
        id: "11111111111111111111111111111111".to_string(),
        kind: SessionKind::Editor,
        author: "addcards".to_string(),
    };
    let h = col.begin_versioning_session(info);
    // Strict mode: a session without a snapshot is a no-op. The unflushed
    // edit to note A remains pending; whichever session eventually snapshots
    // and commits A's nid will own it. Either way, addcards must not.
    assert!(col.commit_versioning_session(h).unwrap().is_none());

    // Note A's history must NOT contain a row authored by "addcards" — the
    // user never asked addcards to touch note A.
    let a_versions = col.list_note_versions(note_a.id).unwrap();
    let leaked: Vec<&str> = a_versions
        .iter()
        .filter(|v| v.author == "addcards")
        .map(|v| v.commit_hash.as_str())
        .collect();
    assert!(
        leaked.is_empty(),
        "addcards commit leaked into note A's history: {leaked:?}"
    );
}

/// TODO #1 — qt/aqt/addcards.py's on_success calls `commit_session`
/// immediately after `add_note`, with no prior snapshot and no
/// `mark_note_added` call. Strict mode treats that as a no-op, so the
/// brand-new note ends up with **no history entry** — silent data loss
/// in the version log. This test pins down the broken sequence.
#[test]
fn addcards_flow_without_mark_loses_new_note_history() {
    let mut col = Collection::new();
    // Bootstrap dolt_history_notes by committing an unrelated prior note.
    let bootstrap = NoteAdder::basic(&mut col)
        .fields(&["bootstrap", "back"])
        .add(&mut col);
    col.mark_note_added_for_session(&session("human").id, bootstrap.id)
        .unwrap();
    let handle = col.begin_versioning_session(session("human"));
    col.commit_versioning_session(handle).unwrap();

    let mut note = NoteAdder::basic(&mut col).fields(&["new", "back"]).note();
    col.add_note(&mut note, crate::decks::DeckId(1)).unwrap();
    // Pre-fix addcards.py sequence: commit_session immediately, with no
    // mark_note_added and no snapshot.
    let info = SessionInfo {
        id: "addcards00000000000000000000beef".to_string(),
        kind: SessionKind::Editor,
        author: "human".to_string(),
    };
    let handle = col.begin_versioning_session(info);
    assert!(
        col.commit_versioning_session(handle).unwrap().is_none(),
        "strict mode: commit without snapshot must be a no-op",
    );
    let versions = col.list_note_versions(note.id).unwrap();
    assert!(
        versions.is_empty(),
        "without the fix, the new note has no history",
    );
}

/// TODO #1 fix — addcards.py now calls the new `MarkNoteAdded` RPC
/// (Collection::mark_note_added_for_session) between `add_note` and
/// `commit_session`. The newly-added note ends up with exactly one
/// history entry attributed to the addcards session.
#[test]
fn addcards_flow_with_mark_records_new_note_history() {
    let mut col = Collection::new();
    let mut note = NoteAdder::basic(&mut col).fields(&["new", "back"]).note();
    col.add_note(&mut note, crate::decks::DeckId(1)).unwrap();
    let sid = "addcards00000000000000000000beef".to_string();
    // The fix.
    col.mark_note_added_for_session(&sid, note.id).unwrap();
    let info = SessionInfo {
        id: sid,
        kind: SessionKind::Editor,
        author: "human".to_string(),
    };
    let handle = col.begin_versioning_session(info);
    assert!(
        col.commit_versioning_session(handle).unwrap().is_some(),
        "marked add must produce a commit",
    );
    let versions = col.list_note_versions(note.id).unwrap();
    assert_eq!(
        versions.len(),
        1,
        "newly-added note must have exactly one history entry",
    );
    assert_eq!(versions[0].author, "human");
}

/// Issue 3 — `restore_note_version` calls `update_note` (which opens a
/// transaction via `Collection::transact`) and then `commit_versioning_session`
/// (which runs `dolt_commit`, an autocommit-only operation per
/// commit.rs:9). The current ordering works because `transact` has fully
/// released by the time the commit fires; this test locks that invariant
/// in so a future refactor that interleaves them gets caught.
#[test]
fn restore_leaves_connection_usable_for_subsequent_versioning() {
    let mut col = Collection::new();
    let mut note = NoteAdder::basic(&mut col).fields(&["v1", "back"]).note();
    col.add_note(&mut note, crate::decks::DeckId(1)).unwrap();
    col.mark_note_added_for_session(&session("human").id, note.id)
        .unwrap();
    let h = col.begin_versioning_session(session("human"));
    col.commit_versioning_session(h).unwrap().unwrap();

    col.snapshot_note_for_session(&session("human").id, note.id)
        .unwrap();
    note.fields_mut()[0] = "v2".to_string();
    col.update_note(&mut note).unwrap();
    let h = col.begin_versioning_session(session("human"));
    col.commit_versioning_session(h).unwrap().unwrap();

    let versions = col.list_note_versions(note.id).unwrap();
    let v1_hash = versions.last().unwrap().commit_hash.clone();

    col.restore_note_version(note.id, &v1_hash, session("human"))
        .unwrap();

    // Immediately after the restore, an ordinary edit + commit must work.
    // A leftover open transaction here would surface as either an error
    // from `dolt_commit` or a silently-dropped change.
    let sid = "22222222222222222222222222222222".to_string();
    col.snapshot_note_for_session(&sid, note.id).unwrap();
    let mut fresh = col.storage.get_note(note.id).unwrap().unwrap();
    fresh.fields_mut()[0] = "v3".to_string();
    col.update_note(&mut fresh).unwrap();
    let info = SessionInfo {
        id: sid,
        kind: SessionKind::Editor,
        author: "human".to_string(),
    };
    let h = col.begin_versioning_session(info);
    assert!(
        col.commit_versioning_session(h).unwrap().is_some(),
        "post-restore versioning session must produce a commit"
    );

    // And the new content is what's actually in the row.
    let after = col.storage.get_note(note.id).unwrap().unwrap();
    assert_eq!(after.fields()[0], "v3");
}

/// Issue 4a — baseline: missing snapshot + clean notes table = no commit.
/// This currently passes by luck (the fallback dirty check is false), but
/// belongs in the suite so the eventual fix doesn't regress it.
#[test]
fn missing_snapshot_with_clean_table_makes_no_commit() {
    let mut col = Collection::new();
    let note = NoteAdder::basic(&mut col)
        .fields(&["front", "back"])
        .add(&mut col);
    col.mark_note_added_for_session(&session("human").id, note.id)
        .unwrap();
    let h = col.begin_versioning_session(session("human"));
    col.commit_versioning_session(h).unwrap().unwrap();

    let before = dolt_log_len(&col);
    // No snapshot, no edits.
    let h = col.begin_versioning_session(session("human"));
    let hash = col.commit_versioning_session(h).unwrap();
    assert!(hash.is_none());
    assert_eq!(dolt_log_len(&col), before);
}

/// Issue 4b — qt/aqt/editor.py used to swallow `begin_session` failures
/// with a bare `print(...)`. With no snapshot recorded, the backend would
/// fall through to `notes_dirty()` and stamp a phantom commit covering
/// whatever unrelated dirt was on the connection. Fixed by requiring a
/// snapshot in `commit_versioning_session`.
#[test]
fn missing_snapshot_does_not_steal_unrelated_dirt() {
    let mut col = Collection::new();
    let mut note1 = NoteAdder::basic(&mut col).fields(&["one", "back"]).note();
    col.add_note(&mut note1, crate::decks::DeckId(1)).unwrap();
    col.mark_note_added_for_session(&session("human").id, note1.id)
        .unwrap();
    let h = col.begin_versioning_session(session("human"));
    col.commit_versioning_session(h).unwrap().unwrap();

    // Editor A: edits note1 but its commit_session hasn't run yet.
    note1.fields_mut()[0] = "one-edited".to_string();
    col.update_note(&mut note1).unwrap();

    // Editor B: begin_session "failed silently" — we skip the snapshot call.
    // Closing editor B fires commit_session.
    let before = dolt_log_len(&col);
    let info = SessionInfo {
        id: "abababababababababababababababab".to_string(),
        kind: SessionKind::Editor,
        author: "editor-b".to_string(),
    };
    let h = col.begin_versioning_session(info);
    let hash = col.commit_versioning_session(h).unwrap();
    assert!(
        hash.is_none(),
        "editor-b commit_session without a snapshot must be a no-op; \
         got commit {hash:?}, dolt_log grew {before} -> {}",
        dolt_log_len(&col)
    );
}

/// Issue 5 — `list_note_versions` used to walk dolt_commit_ancestors with
/// `parent_index = 0` only, hiding any commit reachable through a non-zero
/// parent (the merged-in side of a merge). Fixed by walking every ancestor
/// and deduping by commit_hash.
#[test]
fn merge_commits_appear_in_history() {
    let mut col = Collection::new();
    let mut note = NoteAdder::basic(&mut col).fields(&["root", "back"]).note();
    col.add_note(&mut note, crate::decks::DeckId(1)).unwrap();
    col.mark_note_added_for_session(&session("human").id, note.id)
        .unwrap();
    let h = col.begin_versioning_session(session("human"));
    col.commit_versioning_session(h).unwrap().unwrap();

    let default_branch: String = col
        .storage
        .db
        .query_row("SELECT dolt_default_branch()", [], |r| r.get(0))
        .expect("doltlite exposes dolt_default_branch()");

    col.storage
        .db
        .execute_batch(
            "SELECT dolt_branch('feature');\
             SELECT dolt_checkout('feature');",
        )
        .expect("doltlite supports dolt_branch + dolt_checkout");

    col.snapshot_note_for_session(&session("agent:feature").id, note.id)
        .unwrap();
    note.fields_mut()[0] = "feature-edit".to_string();
    col.update_note(&mut note).unwrap();
    let h = col.begin_versioning_session(session("agent:feature"));
    col.commit_versioning_session(h).unwrap().unwrap();

    // Force a non-fast-forward merge so the feature commit is reachable
    // only via parent_index = 1 of the merge commit.
    col.storage
        .db
        .execute_batch(&format!(
            "SELECT dolt_checkout('{default_branch}');\
             SELECT dolt_merge('--no-ff', 'feature');"
        ))
        .expect("doltlite supports dolt_merge --no-ff");

    let versions = col.list_note_versions(note.id).unwrap();
    let authors: Vec<&str> = versions.iter().map(|v| v.author.as_str()).collect();
    assert!(
        authors.contains(&"agent:feature"),
        "merge-side commit should appear in history; got {authors:?}"
    );
}

/// TODO #3 — `snapshot_note_for_session` stores snapshots in a
/// `HashMap<session_id, NoteSnapshot>`. Snapshotting a second note under
/// the same session id overwrites the first. The dirty-check at commit
/// time then inspects only the last snapshot, so a session that edits
/// note A but also snapshotted note B (where B happened to be unchanged)
/// is treated as a no-op. A's edit stays uncommitted in the working
/// table and gets swept into the next unrelated commit.
#[test]
fn multi_note_session_commits_when_any_snapshot_differs() {
    let mut col = Collection::new();

    let mut a = NoteAdder::basic(&mut col).fields(&["a", "back"]).note();
    col.add_note(&mut a, crate::decks::DeckId(1)).unwrap();
    col.mark_note_added_for_session("seed-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", a.id)
        .unwrap();
    let h = col.begin_versioning_session(SessionInfo {
        id: "seed-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
        kind: SessionKind::Editor,
        author: "human".into(),
    });
    col.commit_versioning_session(h).unwrap().unwrap();

    let mut b = NoteAdder::basic(&mut col).fields(&["b", "back"]).note();
    col.add_note(&mut b, crate::decks::DeckId(1)).unwrap();
    col.mark_note_added_for_session("seed-bbbbbbbbbbbbbbbbbbbbbbbbbbbbbb", b.id)
        .unwrap();
    let h = col.begin_versioning_session(SessionInfo {
        id: "seed-bbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".into(),
        kind: SessionKind::Editor,
        author: "human".into(),
    });
    col.commit_versioning_session(h).unwrap().unwrap();

    // Agent batch: snapshots both notes, edits only A, then commits.
    let sid = "agentbatch0000000000000000000000".to_string();
    col.snapshot_note_for_session(&sid, a.id).unwrap();
    col.snapshot_note_for_session(&sid, b.id).unwrap();
    a.fields_mut()[0] = "a-edited".to_string();
    col.update_note(&mut a).unwrap();

    let h = col.begin_versioning_session(SessionInfo {
        id: sid,
        kind: SessionKind::Agent,
        author: "agent:batch".into(),
    });
    let hash = col.commit_versioning_session(h).unwrap();
    assert!(
        hash.is_some(),
        "edit to A must be committed even though B was also snapshotted",
    );
    let versions_a = col.list_note_versions(a.id).unwrap();
    assert_eq!(
        versions_a[0].author, "agent:batch",
        "newest version of A should be the agent edit; got {versions_a:?}",
    );
    assert_eq!(versions_a[0].changed_fields, vec!["Front".to_string()]);
}
