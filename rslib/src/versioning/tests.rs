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
    NoteAdder::basic(&mut col)
        .fields(&["front", "back"])
        .add(&mut col);

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
    NoteAdder::basic(&mut col)
        .fields(&["front", "back"])
        .add(&mut col);

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
    let h = col.begin_versioning_session(session("human"));
    col.commit_versioning_session(h).unwrap().unwrap();

    note.fields_mut()[0] = "two".to_string();
    col.update_note(&mut note).unwrap();
    let h = col.begin_versioning_session(session("human"));
    col.commit_versioning_session(h).unwrap().unwrap();

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
    let h = col.begin_versioning_session(session("human"));
    col.commit_versioning_session(h).unwrap().unwrap();

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
    NoteAdder::basic(&mut col).fields(&["f", "b"]).add(&mut col);

    let h = col.begin_versioning_session(session("agent:test-bot"));
    col.commit_versioning_session(h).unwrap().unwrap();

    let (_, committer, _) = newest_commit(&col);
    assert_eq!(committer, "agent:test-bot");
}
