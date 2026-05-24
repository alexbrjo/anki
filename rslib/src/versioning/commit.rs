// Copyright: Ankitects Pty Ltd and contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

//! Doltlite commit wrapper.
//!
//! `dolt_commit('-m', ?, '--author', 'Name <email>')` parses the author into
//! a `committer` and `email` column on `dolt_log`. We encode session metadata
//! as a single JSON line in the commit message so it round-trips losslessly
//! without a side table.
//!
//! Doltlite v0.11.0 finding (verified at build time, see history of
//! `rslib/src/versioning/`): calling `dolt_commit` while a SQL transaction is
//! open implicitly closes that transaction. Callers must therefore invoke
//! `commit_versioning_session` only when the connection is in autocommit
//! mode — i.e. between `Collection::transact` calls.
//!
//! Dirty detection uses a content snapshot stored on `CollectionState`
//! (see [`super::session::VersioningState`]). The earlier approach of
//! `SELECT 1 FROM dolt_status WHERE table_name='notes'` was too permissive:
//! Anki's `update_note_inner` rewrites the row whenever the post-normalization
//! field bytes differ from the stored bytes, which can happen even when the
//! user typed nothing (HTML round-trip). Comparing the actual row bytes at
//! session start vs. session end is the authoritative signal.

use rusqlite::params;
use rusqlite::OptionalExtension;
use serde::Serialize;

use super::session::NoteSnapshot;
use super::session::SessionHandle;
use super::session::SessionInfo;
use crate::prelude::*;

#[derive(Debug, Serialize)]
struct CommitMessageMeta<'a> {
    session: &'a str,
    kind: &'a str,
    actor: &'a str,
}

fn format_author(info: &SessionInfo) -> String {
    // Doltlite parses "Name <email>"; we have no real email, so use a
    // placeholder. The author string is the only field we display.
    format!("{} <local>", info.author)
}

fn format_message(info: &SessionInfo) -> String {
    let meta = CommitMessageMeta {
        session: &info.id,
        kind: info.kind.as_str(),
        actor: &info.author,
    };
    serde_json::to_string(&meta).expect("metadata serialization is infallible")
}

impl Collection {
    /// Begin a new versioning session. P0 keeps no per-session server-side
    /// state here beyond the handle — the snapshot for change-detection is
    /// recorded separately via [`Collection::snapshot_note_for_session`].
    pub fn begin_versioning_session(&mut self, info: SessionInfo) -> SessionHandle {
        SessionHandle { info }
    }

    /// Record the note's current row contents under `session_id`. The next
    /// `commit_versioning_session` for the same session id will compare
    /// against this snapshot and skip the commit if nothing actually
    /// changed. Pass the id of an existing note — if the note doesn't
    /// exist yet (Add Cards flow), don't snapshot at all and the commit
    /// will fall back to the older `dolt_status` check.
    pub fn snapshot_note_for_session(&mut self, session_id: &str, nid: NoteId) -> Result<()> {
        let row = read_note_content(self, nid)?;
        let Some((flds, tags)) = row else {
            // Note not present (e.g. it was deleted between begin and now).
            // Treat as no snapshot — caller will get a fall-through commit
            // or a no-op depending on dolt_status.
            return Ok(());
        };
        self.state
            .versioning
            .store(session_id.to_string(), NoteSnapshot { nid, flds, tags });
        Ok(())
    }

    /// Stamp a new commit if the snapshotted note actually changed (or if
    /// no snapshot was taken and the notes table is dirty at the engine
    /// level). Returns the new commit hash, or `None` if nothing was
    /// committed.
    pub fn commit_versioning_session(&mut self, handle: SessionHandle) -> Result<Option<String>> {
        let session_id = handle.info.id.clone();
        let snapshot = self.state.versioning.take(&session_id);

        let should_commit = match snapshot {
            Some(snap) => snapshot_differs_from_current(self, &snap)?,
            // No snapshot recorded for this session — fall back to the
            // engine-level dirty check so legacy callers (and the Add Cards
            // flow, where there's no prior content to snapshot) still work.
            None => notes_dirty(self)?,
        };
        if !should_commit {
            return Ok(None);
        }
        stage_all(self)?;
        let hash = make_commit(self, &handle.info)?;
        Ok(Some(hash))
    }
}

fn read_note_content(col: &Collection, nid: NoteId) -> Result<Option<(String, String)>> {
    Ok(col
        .storage
        .db
        .prepare_cached("SELECT flds, tags FROM notes WHERE id = ?")?
        .query_row(params![nid], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })
        .optional()?)
}

fn snapshot_differs_from_current(col: &Collection, snap: &NoteSnapshot) -> Result<bool> {
    let current = read_note_content(col, snap.nid)?;
    Ok(match current {
        // Note was deleted during the session — counts as a change worth
        // recording, so the deletion shows up in history.
        None => true,
        Some((flds, tags)) => flds != snap.flds || tags != snap.tags,
    })
}

fn notes_dirty(col: &Collection) -> Result<bool> {
    let dirty = col
        .storage
        .db
        .prepare_cached("SELECT 1 FROM dolt_status WHERE table_name='notes' LIMIT 1")?
        .exists([])?;
    Ok(dirty)
}

fn stage_all(col: &Collection) -> Result<()> {
    col.storage
        .db
        .prepare_cached("SELECT dolt_add('-A')")?
        .query_row([], |_| Ok(()))?;
    Ok(())
}

fn make_commit(col: &Collection, info: &SessionInfo) -> Result<String> {
    let author = format_author(info);
    let message = format_message(info);
    let hash: String = col
        .storage
        .db
        .prepare_cached("SELECT dolt_commit('-m', ?, '--author', ?)")?
        .query_row(params![message, author], |r| r.get(0))?;
    Ok(hash)
}
