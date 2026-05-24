# Copyright: Ankitects Pty Ltd and contributors
# License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

"""Note-version history sidebar.

A collapsible right-hand panel attached to the editor. Lists all commits
for the note newest-first; the newest row is labeled ``(current)`` and
represents the live note. Selecting an older row loads its content into
the editor as a non-destructive preview. The ``Revert to selected`` button
writes a new commit equal to the selected version (append-only —
in-between versions remain in history).
"""

from __future__ import annotations

import secrets
import traceback
from datetime import datetime, timezone
from typing import TYPE_CHECKING, Callable

from anki.notes import NoteId
from anki.versioning_pb2 import NoteVersion, SessionKind
from aqt.operations import CollectionOp
from aqt.qt import (
    QFrame,
    QHBoxLayout,
    QLabel,
    QListWidget,
    QListWidgetItem,
    QMessageBox,
    QPushButton,
    QSize,
    Qt,
    QVBoxLayout,
    QWidget,
)
from aqt.utils import tooltip

if TYPE_CHECKING:
    from aqt.editor import Editor


SIDEBAR_WIDTH = 280


class VersionsSidebar(QFrame):
    """Right-hand panel listing prior versions of the loaded note.

    Lifecycle: created once per ``Editor``, hidden by default. The editor
    calls ``reload(nid)`` when the loaded note changes or the sidebar is
    shown. The newest row is labeled ``(current)`` and represents the live
    note — selecting it exits preview and disables Revert. Selecting an
    older row previews that version in the editor.
    """

    def __init__(self, parent: QWidget, editor: Editor) -> None:
        super().__init__(parent)
        self.editor = editor
        self.mw = editor.mw
        self.nid: NoteId | None = None
        self._current_hash: str | None = None
        self._on_restored_callback: Callable[[], None] | None = None
        self._suppress_selection_change = False
        self.setFrameShape(QFrame.Shape.StyledPanel)
        self.setFixedWidth(SIDEBAR_WIDTH)
        self._build_ui()

    def on_restored(self, callback: Callable[[], None]) -> None:
        self._on_restored_callback = callback

    def _build_ui(self) -> None:
        layout = QVBoxLayout(self)
        layout.setContentsMargins(6, 6, 6, 6)
        layout.setSpacing(6)

        header = QLabel("<b>Versions</b>")
        layout.addWidget(header)

        hint = QLabel(
            "Click a row to preview that version in the editor. Revert writes"
            " a new commit; in-between versions stay in history."
        )
        hint.setWordWrap(True)
        hint.setStyleSheet("color: gray; font-size: 11px;")
        layout.addWidget(hint)

        self.list = QListWidget(self)
        self.list.setAlternatingRowColors(True)
        self.list.setSelectionMode(QListWidget.SelectionMode.SingleSelection)
        self.list.currentItemChanged.connect(self._on_current_changed)
        layout.addWidget(self.list, 1)

        button_row = QHBoxLayout()
        button_row.setContentsMargins(0, 0, 0, 0)
        self.revert_btn = QPushButton("Revert to selected", self)
        self.revert_btn.setEnabled(False)
        self.revert_btn.clicked.connect(self._revert_selected)
        button_row.addWidget(self.revert_btn, 1)
        layout.addLayout(button_row)

    def sizeHint(self) -> QSize:  # type: ignore[override]
        return QSize(SIDEBAR_WIDTH, 400)

    def reload(self, nid: NoteId | None) -> None:
        """Repopulate the list for ``nid`` (or clear if ``None``).

        The newest version is selected and labeled ``(current)``; selecting
        it exits any active preview in the editor.
        """
        self.nid = nid
        self._current_hash = None
        self._suppress_selection_change = True
        try:
            self.list.clear()
            self.revert_btn.setEnabled(False)
            if nid is None:
                self.list.addItem("No note loaded.")
                return
            try:
                versions = self.mw.col._backend.list_note_versions(nid=int(nid))
            except Exception as e:
                self.list.addItem(f"Could not load versions: {e}")
                return
            if not versions:
                self.list.addItem("No version history for this note.")
                return
            last_index = len(versions) - 1
            self._current_hash = versions[0].commit_hash
            for i, v in enumerate(versions):
                self._add_row(v, is_current=(i == 0), is_oldest=(i == last_index))
            self.list.setCurrentRow(0)
        finally:
            self._suppress_selection_change = False

    def _add_row(self, v: NoteVersion, is_current: bool, is_oldest: bool) -> None:
        when = _format_timestamp(v.timestamp_secs)
        who = v.author or "?"
        if v.changed_fields:
            fields = ", ".join(v.changed_fields)
        elif is_oldest:
            fields = "(initial)"
        else:
            fields = "(no field change)"
        label = "(current) " if is_current else ""
        text = f"{label}{when}\n{who} · {fields}"
        item = QListWidgetItem(text, self.list)
        item.setData(Qt.ItemDataRole.UserRole, v.commit_hash)
        item.setToolTip(f"{when}\n{who}\n{fields}\ncommit {v.commit_hash[:12]}")

    def _on_current_changed(
        self, current: QListWidgetItem | None, _previous: QListWidgetItem | None
    ) -> None:
        if self._suppress_selection_change:
            return
        if current is None or self.nid is None:
            self.revert_btn.setEnabled(False)
            return
        commit_hash = current.data(Qt.ItemDataRole.UserRole)
        if not commit_hash:
            self.revert_btn.setEnabled(False)
            return
        if commit_hash == self._current_hash:
            # The newest row IS the live note; no preview, no revert target.
            self.revert_btn.setEnabled(False)
            self.editor.exit_version_preview()
            return
        self.revert_btn.setEnabled(True)
        try:
            resp = self.mw.col._backend.get_note_at_version(
                nid=int(self.nid), commit_hash=commit_hash
            )
        except Exception as e:
            tooltip(f"Could not load this version: {e}", parent=self, period=3000)
            return
        self.editor.preview_note_version(list(resp.fields), list(resp.tags))

    def _revert_selected(self) -> None:
        item = self.list.currentItem()
        if item is None or self.nid is None:
            return
        commit_hash = item.data(Qt.ItemDataRole.UserRole)
        if not commit_hash or commit_hash == self._current_hash:
            return
        confirm = QMessageBox.question(
            self,
            "Revert to this version?",
            "This will replace the note's current content with the selected"
            " version. The current content stays accessible in the version log.",
            QMessageBox.StandardButton.Ok | QMessageBox.StandardButton.Cancel,
        )
        if confirm != QMessageBox.StandardButton.Ok:
            return

        nid = self.nid

        def do_restore(col):
            return col._backend.restore_note_version(
                nid=int(nid),
                commit_hash=commit_hash,
                session_id=secrets.token_hex(16),
                kind=SessionKind.SESSION_KIND_EDITOR,
                actor_name="",
            )

        CollectionOp(self, do_restore).success(
            self._on_restore_success
        ).run_in_background()

    def _on_restore_success(self, response) -> None:
        if response.commit_hash:
            tooltip("Reverted to selected version", parent=self)
            self._refresh_reviewer_if_showing_this_note()
        else:
            tooltip(
                "This version is identical to the current note — nothing to revert.",
                parent=self,
                period=2000,
            )
        if self._on_restored_callback is not None:
            try:
                self._on_restored_callback()
            except Exception:
                pass
        self.reload(self.nid)

    def _refresh_reviewer_if_showing_this_note(self) -> None:
        reviewer = getattr(self.mw, "reviewer", None)
        if reviewer is None or self.nid is None:
            return
        card = getattr(reviewer, "card", None)
        if card is None or card.nid != self.nid:
            return
        try:
            reviewer.refresh_if_needed()
        except Exception:
            traceback.print_exc()


def _format_timestamp(secs: int) -> str:
    if secs <= 0:
        return "(unknown time)"
    dt = datetime.fromtimestamp(secs, tz=timezone.utc).astimezone()
    return dt.strftime("%Y-%m-%d %H:%M:%S")
