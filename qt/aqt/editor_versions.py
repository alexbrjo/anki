# Copyright: Ankitects Pty Ltd and contributors
# License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

"""Note-version history dialog.

A minimal P0 UI for the per-note version log: opens a modal listing prior
commits of a single note newest-first, with a Restore button per row that
calls into the Rust ``RestoreNoteVersion`` RPC. Restoring writes a new
revert commit (in-between versions remain in history) and re-loads the
note in the editor so the user sees the rolled-back content.
"""

from __future__ import annotations

import secrets
from datetime import datetime, timezone
from typing import TYPE_CHECKING

from anki.notes import NoteId
from anki.versioning_pb2 import NoteVersion, SessionKind
from aqt.operations import CollectionOp
from aqt.qt import (
    QDialog,
    QDialogButtonBox,
    QHBoxLayout,
    QLabel,
    QListWidget,
    QListWidgetItem,
    QMessageBox,
    QPushButton,
    QSizePolicy,
    QVBoxLayout,
    QWidget,
)
from aqt.utils import tooltip

if TYPE_CHECKING:
    from aqt.main import AnkiQt


class VersionsDialog(QDialog):
    def __init__(self, parent: QWidget, mw: AnkiQt, nid: NoteId) -> None:
        super().__init__(parent)
        self.mw = mw
        self.nid = nid
        self._on_restored_callback = None
        self.setWindowTitle("Versions")
        self.resize(560, 420)
        self._build_ui()
        self._reload()

    def on_restored(self, callback) -> "VersionsDialog":
        """Set a callback to fire after a successful Restore (e.g. to re-load
        the editor's note view). Returns self for chaining."""
        self._on_restored_callback = callback
        return self

    def _build_ui(self) -> None:
        layout = QVBoxLayout(self)
        header = QLabel(
            "Prior versions of this note, newest first. Restore writes a new"
            " commit — in-between versions stay in history."
        )
        header.setWordWrap(True)
        layout.addWidget(header)

        self.list = QListWidget()
        self.list.setAlternatingRowColors(True)
        layout.addWidget(self.list, 1)

        buttons = QDialogButtonBox(QDialogButtonBox.StandardButton.Close)
        buttons.rejected.connect(self.reject)
        buttons.accepted.connect(self.accept)
        layout.addWidget(buttons)

    def _reload(self) -> None:
        self.list.clear()
        try:
            versions = self.mw.col._backend.list_note_versions(nid=int(self.nid))
        except Exception as e:
            self.list.addItem(f"Could not load versions: {e}")
            return

        if not versions:
            self.list.addItem("No prior versions recorded for this note.")
            return

        last_index = len(versions) - 1
        for i, v in enumerate(versions):
            self._add_row(v, is_oldest=(i == last_index))

    def _add_row(self, v: NoteVersion, is_oldest: bool) -> None:
        when = _format_timestamp(v.timestamp_secs)
        who = v.author or "?"
        if v.changed_fields:
            fields = ", ".join(v.changed_fields)
        elif is_oldest:
            # The oldest row has no prior to diff against.
            fields = "(initial)"
        else:
            # Shouldn't happen with snapshot-based dirty detection in place,
            # but if a commit somehow lands without a field change, label it
            # honestly rather than mislabeling it as the initial version.
            fields = "(no field change)"
        text = f"{when}  ·  {who}  ·  {fields}"

        row_widget = QWidget()
        row_layout = QHBoxLayout(row_widget)
        row_layout.setContentsMargins(8, 4, 8, 4)
        label = QLabel(text)
        label.setSizePolicy(QSizePolicy.Policy.Expanding, QSizePolicy.Policy.Preferred)
        row_layout.addWidget(label, 1)
        restore_btn = QPushButton("Restore")
        restore_btn.clicked.connect(
            lambda _checked=False, commit=v.commit_hash: self._restore(commit)
        )
        row_layout.addWidget(restore_btn)

        item = QListWidgetItem(self.list)
        item.setSizeHint(row_widget.sizeHint())
        self.list.addItem(item)
        self.list.setItemWidget(item, row_widget)

    def _restore(self, commit_hash: str) -> None:
        confirm = QMessageBox.question(
            self,
            "Restore this version?",
            "This will replace the note's current content with this version."
            " The current content stays accessible in the version log.",
            QMessageBox.StandardButton.Ok | QMessageBox.StandardButton.Cancel,
        )
        if confirm != QMessageBox.StandardButton.Ok:
            return

        # Wrap in CollectionOp so the underlying update_note's OpChanges
        # propagate through operation_did_execute — that's what marks the
        # reviewer / browser dirty so they redraw the changed note. Bypassing
        # CollectionOp (calling _backend directly) leaves the reviewer
        # stuck on the pre-restore render until it happens to reload the
        # card from db on its own.
        def do_restore(col):
            return col._backend.restore_note_version(
                nid=int(self.nid),
                commit_hash=commit_hash,
                session_id=secrets.token_hex(16),
                kind=SessionKind.SESSION_KIND_EDITOR,
                author="human",
            )

        CollectionOp(self, do_restore).success(self._on_restore_success).run_in_background()

    def _on_restore_success(self, response) -> None:
        if response.commit_hash:
            tooltip("Restored to selected version", parent=self)
            # Even with CollectionOp firing operation_did_execute, the
            # reviewer only redraws on main-window focus. The Versions
            # dialog (modal) is on top, so force the redraw now if the
            # reviewer is showing this very note.
            self._refresh_reviewer_if_showing_this_note()
        else:
            tooltip(
                "This version is identical to the current note — nothing to restore.",
                parent=self,
                period=2000,
            )
        if self._on_restored_callback is not None:
            try:
                self._on_restored_callback()
            except Exception:
                pass
        self._reload()


    def _refresh_reviewer_if_showing_this_note(self) -> None:
        reviewer = getattr(self.mw, "reviewer", None)
        if reviewer is None:
            return
        card = getattr(reviewer, "card", None)
        if card is None or card.nid != self.nid:
            return
        try:
            reviewer.refresh_if_needed()
        except Exception as e:
            print(f"versioning: reviewer refresh failed: {e}")


def _format_timestamp(secs: int) -> str:
    if secs <= 0:
        return "(unknown time)"
    dt = datetime.fromtimestamp(secs, tz=timezone.utc).astimezone()
    return dt.strftime("%Y-%m-%d %H:%M:%S")


def open_versions_dialog(
    parent: QWidget,
    mw: AnkiQt,
    nid: NoteId,
    on_restored=None,
) -> VersionsDialog:
    dlg = VersionsDialog(parent, mw, nid)
    if on_restored is not None:
        dlg.on_restored(on_restored)
    dlg.show()
    return dlg
