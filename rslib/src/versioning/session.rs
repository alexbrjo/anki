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

#[derive(Debug, Clone)]
pub struct SessionInfo {
    pub id: String,
    pub kind: SessionKind,
    pub author: String,
}

impl SessionInfo {
    pub fn editor_human() -> Self {
        Self {
            id: new_session_id(),
            kind: SessionKind::Editor,
            author: "human".to_string(),
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoteSnapshot {
    pub nid: NoteId,
    pub flds: String,
    pub tags: String,
}

/// In-memory map of session-id → snapshot, lives on `CollectionState`.
/// One slot per active editor session; multiple editors simultaneously
/// open just use distinct session ids.
#[derive(Default, Debug)]
pub struct VersioningState {
    snapshots: HashMap<String, NoteSnapshot>,
}

impl VersioningState {
    pub(crate) fn store(&mut self, session_id: String, snapshot: NoteSnapshot) {
        self.snapshots.insert(session_id, snapshot);
    }

    pub(crate) fn take(&mut self, session_id: &str) -> Option<NoteSnapshot> {
        self.snapshots.remove(session_id)
    }
}
