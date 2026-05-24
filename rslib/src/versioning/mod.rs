// Copyright: Ankitects Pty Ltd and contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

//! Per-note version history on top of Doltlite.
//!
//! P0 scope: every editor session that mutates a note produces one Doltlite
//! commit tagged with `author` and a JSON metadata blob carrying the session
//! id. The history UI reads back via `dolt_history_notes` and restore writes
//! a new commit through the normal `update_note` path (revert semantics, not
//! reset).

mod commit;
mod history;
mod restore;
pub mod service;
pub mod session;

pub use history::NoteVersion;
pub use session::SessionHandle;
pub use session::SessionInfo;
pub use session::SessionKind;

#[cfg(test)]
mod tests;
