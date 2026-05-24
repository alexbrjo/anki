// Copyright: Ankitects Pty Ltd and contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

//! Restore a note's fields and tags to those of a prior commit.
//!
//! Doltlite v0.11.0 doesn't support `AS OF` syntax, so we read the historical
//! row from `dolt_history_notes WHERE id=? AND commit_hash=?` instead. The
//! restored values flow through the normal `update_note` path so the search
//! index, sort field, checksum and mtime are all refreshed correctly. A
//! follow-up `commit_versioning_session` then stamps the restore as its own
//! new commit (revert semantics — the in-between versions remain in history).

use rusqlite::params;

use super::session::SessionInfo;
use crate::prelude::*;
use crate::tags::split_tags;

impl Collection {
    /// Replace the current note's fields & tags with their values at
    /// `commit_hash`, producing a new Doltlite commit. Returns the hash of
    /// the new revert commit (or `None` if the historical content already
    /// matched the current state — rare but possible).
    pub fn restore_note_version(
        &mut self,
        nid: NoteId,
        commit_hash: &str,
        session: SessionInfo,
    ) -> Result<RestoreOutcome> {
        let (flds, tags) = load_historical_fields_and_tags(self, nid, commit_hash)?;

        // Snapshot the current row BEFORE we overwrite it. commit_session
        // diffs snapshot-vs-current to decide whether to stamp a commit —
        // without this, a restore-to-identical-content would not produce
        // any new commit (and otherwise the snapshot would be the post-
        // restore state, also producing no commit).
        self.snapshot_note_for_session(&session.id, nid)?;

        let mut note = self.storage.get_note(nid)?.or_not_found(nid)?;
        let new_fields: Vec<String> = flds.split('\x1f').map(Into::into).collect();
        let new_tags: Vec<String> = split_tags(&tags).map(Into::into).collect();
        *note.fields_mut() = new_fields;
        note.tags = new_tags;

        let update_out = self.update_note(&mut note)?;

        let handle = self.begin_versioning_session(session);
        let commit_hash = self.commit_versioning_session(handle)?;
        Ok(RestoreOutcome {
            commit_hash,
            changes: update_out.changes,
        })
    }
}

pub struct RestoreOutcome {
    pub commit_hash: Option<String>,
    pub changes: crate::ops::OpChanges,
}

fn load_historical_fields_and_tags(
    col: &Collection,
    nid: NoteId,
    commit_hash: &str,
) -> Result<(String, String)> {
    col.storage
        .db
        .prepare_cached(
            "SELECT flds, tags
             FROM dolt_history_notes
             WHERE id = ? AND commit_hash = ?",
        )?
        .query_row(params![nid, commit_hash], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })
        .map_err(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => AnkiError::NotFound {
                source: crate::error::NotFoundError {
                    type_name: "note version".to_string(),
                    identifier: format!("nid={nid} commit={commit_hash}"),
                    backtrace: None,
                },
            },
            other => other.into(),
        })
}
