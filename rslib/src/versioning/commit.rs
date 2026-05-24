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
//!
//! Sessions without a recorded snapshot are no-ops on commit — see
//! `commit_versioning_session`. Callers that want a commit must call
//! `snapshot_note_for_session` first.

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
    /// changed.
    ///
    /// Callers MUST snapshot before committing: an absent snapshot means
    /// "no commit". For paths that add a brand-new note (Add Cards flow),
    /// use [`Self::mark_note_added_for_session`] instead, which records
    /// `prior: None` so the subsequent commit fires.
    pub fn snapshot_note_for_session(&mut self, session_id: &str, nid: NoteId) -> Result<()> {
        let prior = read_note_content(self, nid)?;
        self.state
            .versioning
            .store(session_id.to_string(), NoteSnapshot { nid, prior });
        Ok(())
    }

    /// Record that `session_id` intends to commit `nid` as a newly-added
    /// note. The snapshot's `prior` is forced to `None`, so the next
    /// `commit_versioning_session` will see `None → Some(current)` and
    /// stamp a commit. Call this after `add_note` (when `nid` is known).
    pub fn mark_note_added_for_session(
        &mut self,
        session_id: &str,
        nid: NoteId,
    ) -> Result<()> {
        self.state
            .versioning
            .store(session_id.to_string(), NoteSnapshot { nid, prior: None });
        Ok(())
    }

    /// Stamp a new commit if the snapshotted note actually changed. Returns
    /// the new commit hash, or `None` if nothing was committed (including
    /// the case where no snapshot was recorded for this session).
    ///
    /// Strict mode: a missing snapshot is treated as "no intent to commit".
    /// The previous fallback to a connection-wide `dolt_status` check
    /// folded unrelated dirty rows into whichever session happened to
    /// commit next, mis-attributing them in the version log.
    pub fn commit_versioning_session(&mut self, handle: SessionHandle) -> Result<Option<String>> {
        let session_id = handle.info.id.clone();
        let Some(snap) = self.state.versioning.take(&session_id) else {
            return Ok(None);
        };
        if !snapshot_differs_from_current(self, &snap)? {
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
    Ok(current != snap.prior)
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
