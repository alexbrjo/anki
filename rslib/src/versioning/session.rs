// Copyright: Ankitects Pty Ltd and contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

use std::collections::HashMap;

use rand::RngCore;

use crate::prelude::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionKind {
    Editor,
    Agent,
    App,
}

impl SessionKind {
    pub fn as_str(self) -> &'static str {
        match self {
            SessionKind::Editor => "editor",
            SessionKind::Agent => "agent",
            SessionKind::App => "app",
        }
    }
}

/// Caller-supplied identity for a versioning session. The dolt_log
/// committer string is *derived* here from `kind` + `actor_name`, not
/// taken verbatim from the client — that's what stops an agent or app
/// from masquerading as the user.
#[derive(Debug, Clone)]
pub struct SessionInfo {
    pub id: String,
    pub kind: SessionKind,
    /// Suffix used for Agent / App commits. Empty (and ignored) for
    /// Editor. Required (non-empty) for Agent / App — `validate` checks.
    pub actor_name: String,
}

impl SessionInfo {
    pub fn editor_user() -> Self {
        Self {
            id: new_session_id(),
            kind: SessionKind::Editor,
            actor_name: String::new(),
        }
    }

    /// Final committer string written to `dolt_log.committer`. Editor
    /// commits are always literally "user"; Agent / App commits are
    /// `kind:actor_name`.
    pub fn author(&self) -> String {
        match self.kind {
            SessionKind::Editor => "user".to_string(),
            SessionKind::Agent => format!("agent:{}", self.actor_name),
            SessionKind::App => format!("app:{}", self.actor_name),
        }
    }

    /// Reject sessions that can't produce a meaningful author. Called
    /// from `commit_versioning_session` and `restore_note_version`
    /// before any dolt commit fires.
    pub fn validate(&self) -> Result<()> {
        match self.kind {
            SessionKind::Editor => Ok(()),
            SessionKind::Agent | SessionKind::App => {
                if self.actor_name.is_empty() {
                    crate::invalid_input!(
                        "versioning session of kind {} requires a non-empty actor_name",
                        self.kind.as_str()
                    );
                }
                Ok(())
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct SessionHandle {
    pub info: SessionInfo,
}

fn new_session_id() -> String {
    let mut bytes = [0u8; 16];
    rand::rng().fill_bytes(&mut bytes);
    let mut s = String::with_capacity(32);
    for b in bytes {
        s.push_str(&format!("{:02x}", b));
    }
    s
}

/// Snapshot of a single note's editable content, taken at the start of a
/// versioning session and compared against the current state at session
/// end. The comparison runs against the database (not against the editor's
/// in-memory note object) so Anki's HTML normalization / sort-field
/// recomputation that happens inside `update_note` is reflected on both
/// sides — a no-op editor open then close leaves the snapshot equal to
/// the current state and we skip the commit.
///
/// `prior` is `None` when the note did not exist at snapshot time
/// (e.g. the Add Cards flow snapshots before the row is created). The
/// commit then fires iff `prior != current` — including the
/// None→Some(content) transition for an add.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoteSnapshot {
    pub nid: NoteId,
    pub prior: Option<(String, String)>,
}

/// In-memory map of session-id → per-note snapshots, lives on
/// `CollectionState`. A single session can snapshot multiple notes
/// (agent batch edits); re-snapshotting the same nid within a session
/// overwrites that nid's entry but leaves other notes alone.
#[derive(Default, Debug)]
pub struct VersioningState {
    snapshots: HashMap<String, HashMap<NoteId, NoteSnapshot>>,
}

impl VersioningState {
    pub(crate) fn store(&mut self, session_id: String, snapshot: NoteSnapshot) {
        self.snapshots
            .entry(session_id)
            .or_default()
            .insert(snapshot.nid, snapshot);
    }

    pub(crate) fn take(&mut self, session_id: &str) -> Vec<NoteSnapshot> {
        self.snapshots
            .remove(session_id)
            .map(|m| m.into_values().collect())
            .unwrap_or_default()
    }
}
