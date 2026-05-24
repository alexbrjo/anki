// Copyright: Ankitects Pty Ltd and contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

//! Read-side: list prior versions of a single note from `dolt_history_notes`,
//! joined to `dolt_log` for the JSON metadata we stashed in the commit
//! message.

use chrono::NaiveDateTime;
use rusqlite::params;
use serde::Deserialize;

use crate::prelude::*;

fn split_fields(fields: &str) -> Vec<&str> {
    fields.split('\x1f').collect()
}

/// One row from the collection-wide audit log. Stripped down to the
/// metadata available from `dolt_log` alone — no per-note diff fields.
#[derive(Debug, Clone)]
pub struct CollectionVersion {
    pub commit_hash: String,
    pub timestamp_secs: i64,
    pub author: String,
    pub session_id: String,
    pub session_kind: String,
}

#[derive(Debug, Clone)]
pub struct NoteVersion {
    pub commit_hash: String,
    pub timestamp_secs: i64,
    pub author: String,
    pub session_id: String,
    pub session_kind: String,
    pub changed_fields: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct CommitMeta {
    #[serde(default)]
    session: String,
    #[serde(default)]
    kind: String,
}

struct HistoryRow {
    commit_hash: String,
    date: String,
    committer: String,
    message: String,
    mid: i64,
    flds: String,
}

impl Collection {
    /// Return all versions of `nid` newest first. Empty if the note has no
    /// history (e.g. the file was migrated from stock SQLite and never edited
    /// under Doltlite).
    pub fn list_note_versions(&mut self, nid: NoteId) -> Result<Vec<NoteVersion>> {
        let rows = load_history(self, nid)?;
        let field_names = self.field_names_for_notetype_of_newest(&rows)?;
        Ok(build_versions(&rows, field_names.as_deref()))
    }

    /// Audit-style listing of recent commits across the whole collection.
    /// `author` and `session_id` are exact-match filters (empty = no
    /// filter on that field). `limit` is clamped to 1..=1000, defaulting
    /// to 50 when zero. Returns rows newest-first by commit date.
    ///
    /// Unlike `list_note_versions`, this does not compute per-field
    /// diffs (that requires joining to `dolt_history_notes` per nid).
    /// The intent is "what has happened in this collection lately" —
    /// the per-note diff is one click away in the existing sidebar.
    pub fn list_recent_versions(
        &self,
        author: Option<&str>,
        session_id: Option<&str>,
        limit: u32,
    ) -> Result<Vec<CollectionVersion>> {
        let clamped = if limit == 0 { 50 } else { limit.min(1000) };
        load_recent_commits(self, author, clamped).map(|rows| filter_and_build(rows, session_id))
    }

    fn field_names_for_notetype_of_newest(
        &mut self,
        rows: &[HistoryRow],
    ) -> Result<Option<Vec<String>>> {
        let Some(newest) = rows.first() else {
            return Ok(None);
        };
        let ntid = NotetypeId(newest.mid);
        let Some(nt) = self.get_notetype(ntid)? else {
            return Ok(None);
        };
        Ok(Some(nt.fields.iter().map(|f| f.name.clone()).collect()))
    }
}

fn load_history(col: &Collection, nid: NoteId) -> Result<Vec<HistoryRow>> {
    // dolt_log.date is only second-resolution, so two commits made within
    // the same second tie. Walk every ancestor reachable from HEAD via
    // dolt_commit_ancestors (any parent_index) so merged-in side commits
    // are visible too; dedup by commit_hash and order by the shortest
    // path to HEAD (newest first).
    let mut stmt = col.storage.db.prepare_cached(
        "WITH RECURSIVE chain(commit_hash, depth) AS (
             SELECT hash, 0
             FROM dolt_branches
             WHERE name = (SELECT dolt_default_branch())
             UNION ALL
             SELECT a.parent_hash, c.depth + 1
             FROM chain c
             JOIN dolt_commit_ancestors a
               ON c.commit_hash = a.commit_hash
         )
         SELECT h.commit_hash, l.date, h.committer, l.message, h.mid, h.flds
         FROM dolt_history_notes h
         JOIN dolt_log l USING (commit_hash)
         JOIN (
             SELECT commit_hash, MIN(depth) AS depth
             FROM chain
             GROUP BY commit_hash
         ) c USING (commit_hash)
         WHERE h.id = ?
         ORDER BY c.depth ASC",
    )?;
    let rows = stmt
        .query_map(params![nid], |r| {
            Ok(HistoryRow {
                commit_hash: r.get(0)?,
                date: r.get(1)?,
                committer: r.get(2)?,
                message: r.get(3)?,
                mid: r.get(4)?,
                flds: r.get(5)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

fn build_versions(rows: &[HistoryRow], field_names: Option<&[String]>) -> Vec<NoteVersion> {
    let mut out = Vec::with_capacity(rows.len());
    for (i, row) in rows.iter().enumerate() {
        let prior = rows.get(i + 1);
        let meta = parse_meta(&row.message);
        let changed = changed_field_names(&row.flds, prior.map(|p| p.flds.as_str()), field_names);
        out.push(NoteVersion {
            commit_hash: row.commit_hash.clone(),
            timestamp_secs: parse_dolt_date(&row.date),
            author: row.committer.clone(),
            session_id: meta.session,
            session_kind: meta.kind,
            changed_fields: changed,
        });
    }
    out
}

fn parse_meta(message: &str) -> CommitMeta {
    // Try to parse as JSON; if it fails (e.g. the repo-init "Initialize data
    // repository" message), fall back to empty metadata.
    serde_json::from_str(message).unwrap_or(CommitMeta {
        session: String::new(),
        kind: String::new(),
    })
}

fn parse_dolt_date(s: &str) -> i64 {
    // dolt_log.date format: "YYYY-MM-DD HH:MM:SS" (UTC, second resolution).
    NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S")
        .map(|dt| dt.and_utc().timestamp())
        .unwrap_or(0)
}

struct RecentRow {
    commit_hash: String,
    date: String,
    committer: String,
    message: String,
}

fn load_recent_commits(
    col: &Collection,
    author: Option<&str>,
    limit: u32,
) -> Result<Vec<RecentRow>> {
    // `dolt_log.date` is only second-resolution, so multiple commits made
    // in the same second tie — and tests routinely produce that. Order
    // by depth-from-HEAD via `dolt_commit_ancestors` instead (same
    // pattern as `load_history`), which gives a stable newest-first
    // sequence regardless of timestamp resolution.
    let mut stmt = col.storage.db.prepare_cached(
        "WITH RECURSIVE chain(commit_hash, depth) AS (
             SELECT hash, 0
             FROM dolt_branches
             WHERE name = (SELECT dolt_default_branch())
             UNION ALL
             SELECT a.parent_hash, c.depth + 1
             FROM chain c
             JOIN dolt_commit_ancestors a
               ON c.commit_hash = a.commit_hash
         )
         SELECT l.commit_hash, l.date, l.committer, l.message
         FROM dolt_log l
         JOIN (
             SELECT commit_hash, MIN(depth) AS depth
             FROM chain
             GROUP BY commit_hash
         ) c USING (commit_hash)
         WHERE (?1 = '' OR l.committer = ?1)
         ORDER BY c.depth ASC
         LIMIT ?2",
    )?;
    let author_filter = author.unwrap_or("");
    let rows = stmt
        .query_map(params![author_filter, limit], |r| {
            Ok(RecentRow {
                commit_hash: r.get(0)?,
                date: r.get(1)?,
                committer: r.get(2)?,
                message: r.get(3)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

fn filter_and_build(rows: Vec<RecentRow>, session_filter: Option<&str>) -> Vec<CollectionVersion> {
    let session_filter = session_filter.unwrap_or("");
    rows.into_iter()
        .filter_map(|row| {
            let meta = parse_meta(&row.message);
            if !session_filter.is_empty() && meta.session != session_filter {
                return None;
            }
            Some(CollectionVersion {
                commit_hash: row.commit_hash,
                timestamp_secs: parse_dolt_date(&row.date),
                author: row.committer,
                session_id: meta.session,
                session_kind: meta.kind,
            })
        })
        .collect()
}

fn changed_field_names(
    current_flds: &str,
    prior_flds: Option<&str>,
    field_names: Option<&[String]>,
) -> Vec<String> {
    let Some(prior) = prior_flds else {
        return Vec::new();
    };
    diff_field_names(current_flds, prior, field_names)
}

/// Names of fields whose split-on-\x1f value differs between two field
/// blobs. Shared by per-note history listing and the two-sided
/// `DiffNoteBetweenVersions` RPC.
pub(super) fn diff_field_names(
    a_flds: &str,
    b_flds: &str,
    field_names: Option<&[String]>,
) -> Vec<String> {
    let a = split_fields(a_flds);
    let b = split_fields(b_flds);
    let mut changed = Vec::new();
    for i in 0..a.len().max(b.len()) {
        if a.get(i) != b.get(i) {
            let name = field_names
                .and_then(|names| names.get(i))
                .cloned()
                .unwrap_or_else(|| format!("Field {i}"));
            changed.push(name);
        }
    }
    changed
}
