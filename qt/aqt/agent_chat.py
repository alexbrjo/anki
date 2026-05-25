# Copyright: Ankitects Pty Ltd and contributors
# License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

"""Standalone always-available chat window backed by the Anki agent."""

from __future__ import annotations

import aqt
import aqt.main
from aqt.qt import (
    QDialog,
    Qt,
    QVBoxLayout,
)
from aqt.utils import restoreGeom, saveGeom
from aqt.webview import AnkiWebView, AnkiWebViewKind


class AgentChatWindow(QDialog):
    """Non-modal window hosting the Svelte-based agent chat."""

    TITLE = "agentChat"
    silentlyClose = True

    def __init__(self, mw: aqt.main.AnkiQt) -> None:
        QDialog.__init__(self, mw, Qt.WindowType.Window)
        self.mw = mw
        mw.garbage_collect_on_dialog_finish(self)
        self.setWindowTitle("Anki AI")
        restoreGeom(self, self.TITLE, default_size=(520, 720))

        self.web = AnkiWebView(kind=AnkiWebViewKind.AGENT_CHAT)
        self.web.load_sveltekit_page("agent-chat")
        layout = QVBoxLayout(self)
        layout.setContentsMargins(0, 0, 0, 0)
        layout.addWidget(self.web)

        self.show()
        self.activateWindow()

    def reject(self) -> None:
        self.web.cleanup()
        self.web = None  # type: ignore
        saveGeom(self, self.TITLE)
        aqt.dialogs.markClosed("AgentChat")
        QDialog.reject(self)


def open_agent_chat(mw: aqt.main.AnkiQt) -> None:
    """Slot for the Tools → AI Chat menu entry.

    The window always opens; if no API key is configured, the chat UI itself
    shows the settings panel expanded so the user can paste one in.
    """
    aqt.dialogs.open("AgentChat", mw)
