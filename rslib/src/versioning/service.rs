// Copyright: Ankitects Pty Ltd and contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

use anki_proto::versioning as pb;

use super::history::CollectionVersion;
use super::history::NoteVersion;
use super::restore::load_historical_fields_and_tags;
use super::restore::NoteDiff;
use super::session::SessionInfo;
use super::session::SessionKind;
use crate::collection::Collection;
use crate::error;
use crate::notes::NoteId;
use crate::tags::split_tags;

impl crate::services::VersioningService for Collection {
    fn begin_session(
        &mut self,
        input: pb::BeginSessionRequest,
    ) -> error::Result<pb::BeginSessionResponse> {
        self.snapshot_note_for_session(&input.session_id, NoteId(input.nid))?;
        Ok(pb::BeginSessionResponse {})
    }

    fn mark_note_added(
        &mut self,
        input: pb::MarkNoteAddedRequest,
    ) -> error::Result<pb::MarkNoteAddedResponse> {
        self.mark_note_added_for_session(&input.session_id, NoteId(input.nid))?;
        Ok(pb::MarkNoteAddedResponse {})
    }

    fn commit_session(
        &mut self,
        input: pb::CommitSessionRequest,
    ) -> error::Result<pb::CommitSessionResponse> {
        let kind = match pb::SessionKind::try_from(input.kind).unwrap_or(pb::SessionKind::Editor) {
            pb::SessionKind::Editor => SessionKind::Editor,
            pb::SessionKind::Agent => SessionKind::Agent,
            pb::SessionKind::App => SessionKind::App,
        };
        let info = SessionInfo {
            id: input.session_id,
            kind,
            actor_name: input.actor_name,
        };
        let handle = self.begin_versioning_session(info);
        let hash = self.commit_versioning_session(handle)?.unwrap_or_default();
        Ok(pb::CommitSessionResponse { commit_hash: hash })
    }

    fn list_note_versions(
        &mut self,
        input: pb::NoteVersionsRequest,
    ) -> error::Result<pb::NoteVersionsResponse> {
        let versions = self.list_note_versions(NoteId(input.nid))?;
        Ok(pb::NoteVersionsResponse {
            versions: versions.into_iter().map(into_pb).collect(),
        })
    }

    fn list_recent_versions(
        &mut self,
        input: pb::ListRecentVersionsRequest,
    ) -> error::Result<pb::ListRecentVersionsResponse> {
        let author = if input.author.is_empty() {
            None
        } else {
            Some(input.author.as_str())
        };
        let session_id = if input.session_id.is_empty() {
            None
        } else {
            Some(input.session_id.as_str())
        };
        let versions = Collection::list_recent_versions(self, author, session_id, input.limit)?;
        Ok(pb::ListRecentVersionsResponse {
            versions: versions.into_iter().map(collection_into_pb).collect(),
        })
    }

    fn get_note_at_version(
        &mut self,
        input: pb::GetNoteAtVersionRequest,
    ) -> error::Result<pb::GetNoteAtVersionResponse> {
        let (flds, tags) =
            load_historical_fields_and_tags(self, NoteId(input.nid), &input.commit_hash)?;
        let fields: Vec<String> = flds.split('\x1f').map(Into::into).collect();
        let tags: Vec<String> = split_tags(&tags).map(Into::into).collect();
        Ok(pb::GetNoteAtVersionResponse { fields, tags })
    }

    fn diff_note_between_versions(
        &mut self,
        input: pb::DiffNoteBetweenVersionsRequest,
    ) -> error::Result<pb::DiffNoteBetweenVersionsResponse> {
        let diff = self.diff_note_between_versions(
            NoteId(input.nid),
            &input.from_commit_hash,
            &input.to_commit_hash,
        )?;
        Ok(diff_into_pb(diff))
    }

    fn restore_note_version(
        &mut self,
        input: pb::RestoreNoteVersionRequest,
    ) -> error::Result<pb::RestoreNoteVersionResponse> {
        let kind = match pb::SessionKind::try_from(input.kind).unwrap_or(pb::SessionKind::Editor) {
            pb::SessionKind::Editor => SessionKind::Editor,
            pb::SessionKind::Agent => SessionKind::Agent,
            pb::SessionKind::App => SessionKind::App,
        };
        let session = SessionInfo {
            id: input.session_id,
            kind,
            actor_name: input.actor_name,
        };
        let outcome = self.restore_note_version(NoteId(input.nid), &input.commit_hash, session)?;
        Ok(pb::RestoreNoteVersionResponse {
            commit_hash: outcome.commit_hash.unwrap_or_default(),
            changes: Some(outcome.changes.into()),
        })
    }
}

fn diff_into_pb(d: NoteDiff) -> pb::DiffNoteBetweenVersionsResponse {
    pb::DiffNoteBetweenVersionsResponse {
        from_fields: d.from_fields,
        to_fields: d.to_fields,
        from_tags: d.from_tags,
        to_tags: d.to_tags,
        changed_fields: d.changed_fields,
    }
}

fn collection_into_pb(v: CollectionVersion) -> pb::CollectionVersion {
    pb::CollectionVersion {
        commit_hash: v.commit_hash,
        timestamp_secs: v.timestamp_secs,
        author: v.author,
        session_id: v.session_id,
        session_kind: v.session_kind,
    }
}

fn into_pb(v: NoteVersion) -> pb::NoteVersion {
    pb::NoteVersion {
        commit_hash: v.commit_hash,
        timestamp_secs: v.timestamp_secs,
        author: v.author,
        session_id: v.session_id,
        session_kind: v.session_kind,
        changed_fields: v.changed_fields,
    }
}
