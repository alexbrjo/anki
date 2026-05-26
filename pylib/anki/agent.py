# Copyright: Ankitects Pty Ltd and contributors
# License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

"""Agentic chat over the user's collection.

Tools wrap the standard Collection API so every edit flows through the
normal write path — and therefore lands as a Doltlite commit that the
versioning sidebar can revert.

The collection is not thread-safe. When the agent runs off the main
thread (e.g. inside a Flask request handler), the caller must pass an
``on_main`` marshaller that runs the given closure on whichever thread
owns the collection and returns its result synchronously. The default
identity marshaller is correct for single-threaded use (CLI, tests).
"""

from __future__ import annotations

import asyncio
import os
import secrets
from collections.abc import AsyncIterator, Callable
from dataclasses import dataclass, field
from typing import Any, TypeVar

from pydantic_ai import Agent, RunContext
from pydantic_ai.models.openai import (
    OpenAIResponsesModel,
    OpenAIResponsesModelSettings,
)
from pydantic_ai.providers.openai import OpenAIProvider

from anki.collection import Collection
from anki.decks import DeckId
from anki.notes import NoteId
from anki.versioning_pb2 import SessionKind

T = TypeVar("T")
OnMain = Callable[[Callable[[], Any]], Any]


def _identity_on_main(fn: Callable[[], T]) -> T:
    return fn()


def _snapshot_before_write(deps: AgentDeps, nid: int, *, added: bool) -> None:
    """Snapshot a note in the shared session (or open a one-shot session)."""
    sid = deps.session_id
    if sid is None:
        # standalone mode (CLI): own session per edit, committed inline below
        sid = secrets.token_hex(16)
        deps.session_id = sid
        deps.snapshotted.add(nid)
        _do_snapshot(deps.col, sid, nid, added=added)
        # mark this as a one-shot so _maybe_close_own_session knows to commit
        deps._one_shot = True  # type: ignore[attr-defined]
        return
    if nid in deps.snapshotted:
        return
    deps.snapshotted.add(nid)
    _do_snapshot(deps.col, sid, nid, added=added)


def _do_snapshot(col: Collection, sid: str, nid: int, *, added: bool) -> None:
    if added:
        col._backend.mark_note_added(session_id=sid, nid=nid)
    else:
        col._backend.begin_session(session_id=sid, nid=nid)


def _maybe_close_own_session(deps: AgentDeps) -> None:
    """Commit and clear a one-shot session (standalone mode only)."""
    if not getattr(deps, "_one_shot", False):
        return
    sid = deps.session_id
    assert sid is not None
    deps.col._backend.commit_session(
        session_id=sid,
        kind=SessionKind.SESSION_KIND_AGENT,
        actor_name=deps.actor_name,
    )
    deps.session_id = None
    deps.snapshotted.clear()
    deps._one_shot = False  # type: ignore[attr-defined]


SYSTEM_PROMPT = """You are an assistant embedded in the Anki spaced-repetition app.

You have tools to search, read, edit, and create notes in the user's collection.
Every edit is version-controlled — the user can revert any change from the
versioning sidebar, so prefer making a useful attempt over asking many
clarifying questions.

When referring to a note, cite its numeric id. Keep responses concise. Anki
search syntax: `deck:"Spanish"`, `tag:verbs`, `"exact phrase"`, `is:due`,
`prop:lapses>3`.
"""


@dataclass
class AgentDeps:
    col: Collection
    on_main: OnMain = field(default=_identity_on_main)
    # Becomes part of the commit author string ("agent:<actor_name>") so the
    # version-history sidebar can attribute edits to a specific model.
    actor_name: str = "agent"
    # If set, all edits within this run join this session and produce a single
    # dolt_commit at end-of-turn. If None, each edit makes its own mini-session
    # (CLI / one-off usage).
    session_id: str | None = None
    # NoteIds we've already snapshotted in the shared session; prevents a
    # second update on the same note from clobbering its pristine prior state.
    snapshotted: set[int] = field(default_factory=set)


def build_agent(api_key: str, model: str) -> Agent[AgentDeps, str]:
    # Use the Responses API rather than Chat Completions. For reasoning
    # models (gpt-5, o-series) this is the only path that returns reasoning
    # summary items in the stream; for non-reasoning models the setting is
    # a no-op so it's safe as a default for all OpenAI models.
    settings = OpenAIResponsesModelSettings(
        openai_reasoning_summary="auto",
    )
    llm = OpenAIResponsesModel(
        model,
        provider=OpenAIProvider(api_key=api_key),
        settings=settings,
    )
    agent: Agent[AgentDeps, str] = Agent(
        llm, deps_type=AgentDeps, system_prompt=SYSTEM_PROMPT
    )

    @agent.tool
    def search_notes(
        ctx: RunContext[AgentDeps], query: str, limit: int = 25
    ) -> list[dict[str, Any]]:
        """Search notes with Anki query syntax. Returns id + first two fields + tags."""

        def _do() -> list[dict[str, Any]]:
            nids = ctx.deps.col.find_notes(query)
            out: list[dict[str, Any]] = []
            for nid in list(nids)[:limit]:
                note = ctx.deps.col.get_note(nid)
                out.append(
                    {
                        "id": int(nid),
                        "fields": list(note.fields[:2]),
                        "tags": list(note.tags),
                    }
                )
            return out

        return ctx.deps.on_main(_do)

    @agent.tool
    def get_note(ctx: RunContext[AgentDeps], note_id: int) -> dict[str, Any]:
        """Read a note in full: all named fields, tags, and notetype."""

        def _do() -> dict[str, Any]:
            note = ctx.deps.col.get_note(NoteId(note_id))
            nt = note.note_type()
            fields: dict[str, str] = {}
            if nt:
                for fld, val in zip(nt["flds"], note.fields):
                    fields[fld["name"]] = val
            return {
                "id": int(note.id),
                "notetype": nt["name"] if nt else "?",
                "fields": fields,
                "tags": list(note.tags),
            }

        return ctx.deps.on_main(_do)

    @agent.tool
    def update_note(
        ctx: RunContext[AgentDeps],
        note_id: int,
        fields: dict[str, str],
        tags: list[str] | None = None,
    ) -> str:
        """Update a note. Pass only the fields you want to change, keyed by field name."""

        def _do() -> str:
            col = ctx.deps.col
            note = col.get_note(NoteId(note_id))
            nt = note.note_type()
            if nt:
                for i, fld in enumerate(nt["flds"]):
                    if fld["name"] in fields:
                        note.fields[i] = fields[fld["name"]]
            if tags is not None:
                note.tags = tags
            _snapshot_before_write(ctx.deps, int(note.id), added=False)
            col.update_note(note)
            _maybe_close_own_session(ctx.deps)
            return f"updated note {note_id}"

        return ctx.deps.on_main(_do)

    @agent.tool
    def add_note(
        ctx: RunContext[AgentDeps],
        notetype_name: str,
        deck_name: str,
        fields: dict[str, str],
        tags: list[str] | None = None,
    ) -> str:
        """Create a new note in the given deck using the given notetype."""

        def _do() -> str:
            col = ctx.deps.col
            nt = col.models.by_name(notetype_name)
            if not nt:
                return f"error: no notetype named {notetype_name!r}"
            deck_id = col.decks.id_for_name(deck_name)
            if deck_id is None:
                return f"error: no deck named {deck_name!r}"
            note = col.new_note(nt)
            for i, fld in enumerate(nt["flds"]):
                if fld["name"] in fields:
                    note.fields[i] = fields[fld["name"]]
            if tags:
                note.tags = tags
            col.add_note(note, DeckId(deck_id))
            _snapshot_before_write(ctx.deps, int(note.id), added=True)
            _maybe_close_own_session(ctx.deps)
            return f"created note {int(note.id)}"

        return ctx.deps.on_main(_do)

    @agent.tool
    def card_stats(
        ctx: RunContext[AgentDeps], note_id: int
    ) -> list[dict[str, Any]]:
        """Per-card review stats for a note: reps, lapses, interval, ease."""

        def _do() -> list[dict[str, Any]]:
            note = ctx.deps.col.get_note(NoteId(note_id))
            out: list[dict[str, Any]] = []
            for card in note.cards():
                out.append(
                    {
                        "card_id": int(card.id),
                        "reps": card.reps,
                        "lapses": card.lapses,
                        "interval_days": card.ivl,
                        "ease_pct": card.factor // 10 if card.factor else 0,
                    }
                )
            return out

        return ctx.deps.on_main(_do)

    return agent


def _translate_event(ev: Any) -> list[dict[str, Any]]:
    """Map a PydanticAI stream event to zero or more wire events."""
    from pydantic_ai import messages as M

    kind = getattr(ev, "event_kind", None)
    if kind == "part_start":
        part = ev.part
        if isinstance(part, M.ThinkingPart):
            return [{"type": "thinking_start"}]
        return []
    if kind == "part_delta":
        d = ev.delta
        if isinstance(d, M.TextPartDelta):
            return [{"type": "text_delta", "text": d.content_delta}]
        if isinstance(d, M.ThinkingPartDelta):
            text = d.content_delta or ""
            if not text:
                return []
            return [{"type": "thinking_delta", "text": text}]
        # ToolCallPartDelta — partial JSON args, intentionally suppressed
        return []
    if kind == "part_end":
        part = ev.part
        if isinstance(part, M.ThinkingPart):
            return [{"type": "thinking_end"}]
        return []
    if kind == "function_tool_call":
        p = ev.part
        return [
            {
                "type": "tool_call",
                "name": getattr(p, "tool_name", "?"),
                "args": getattr(p, "args", None),
                "tool_call_id": getattr(p, "tool_call_id", None),
            }
        ]
    if kind == "function_tool_result":
        p = ev.part
        return [
            {
                "type": "tool_result",
                "name": getattr(p, "tool_name", "?"),
                "content": str(getattr(p, "content", ""))[:500],
                "tool_call_id": getattr(p, "tool_call_id", None),
            }
        ]
    return []


async def chat(
    col: Collection,
    api_key: str,
    model: str,
    message: str,
    on_main: OnMain = _identity_on_main,
    actor_name: str | None = None,
) -> AsyncIterator[dict[str, Any]]:
    """Run one user message through the agent, yielding event dicts.

    Event shapes:
      {"type": "text_delta", "text": str}
      {"type": "thinking_start"} / {"type": "thinking_delta", "text": str} /
        {"type": "thinking_end"}
      {"type": "tool_call",  "name": str, "args": Any, "tool_call_id": str|None}
      {"type": "tool_result","name": str, "content": str, "tool_call_id": str|None}
      {"type": "done"}
      {"type": "error", "message": str}

    Events come out in true model-stream order: reasoning, tool calls, results,
    and answer text interleave as the model produces them.
    """
    # Open one versioning session for the whole turn — any edits made by tools
    # join it, and we commit once at the end. Without this, each tool call
    # would produce its own dolt_commit and noisily pollute every note's
    # version history with "(no change)" entries.
    sid = secrets.token_hex(16)
    deps = AgentDeps(
        col=col,
        on_main=on_main,
        actor_name=actor_name or model,
        session_id=sid,
    )
    try:
        agent = build_agent(api_key, model)
        async with agent.iter(message, deps=deps) as run:
            async for node in run:
                if Agent.is_model_request_node(node) or Agent.is_call_tools_node(
                    node
                ):
                    async with node.stream(run.ctx) as stream:
                        async for ev in stream:
                            for out in _translate_event(ev):
                                yield out
    except Exception as e:  # surface errors over the stream rather than crashing
        yield {"type": "error", "message": f"{type(e).__name__}: {e}"}
    finally:
        # Commit the turn's batched edits. Empty sessions return None and
        # don't produce a commit, so it's safe to always call.
        if deps.snapshotted:
            try:
                on_main(
                    lambda: col._backend.commit_session(
                        session_id=sid,
                        kind=SessionKind.SESSION_KIND_AGENT,
                        actor_name=deps.actor_name,
                    )
                )
            except Exception as e:
                yield {
                    "type": "error",
                    "message": f"versioning commit failed: {type(e).__name__}: {e}",
                }
    yield {"type": "done"}


def main() -> None:
    """CLI entry point for headless verification: `python -m anki.agent ...`."""
    import argparse

    parser = argparse.ArgumentParser(description="Chat with your Anki collection.")
    parser.add_argument("message", help="Message to send to the agent.")
    parser.add_argument(
        "--collection",
        required=True,
        help="Path to collection.anki2",
    )
    parser.add_argument("--model", default="gpt-4o-mini")
    args = parser.parse_args()

    api_key = os.environ.get("OPENAI_API_KEY")
    if not api_key:
        raise SystemExit("set OPENAI_API_KEY")

    col = Collection(args.collection)

    async def _run() -> None:
        in_thinking = False
        async for event in chat(col, api_key, args.model, args.message):
            kind = event["type"]
            if kind == "text_delta":
                if in_thinking:
                    print("", flush=True)
                    in_thinking = False
                print(event["text"], end="", flush=True)
            elif kind == "thinking_start":
                print("\n[thinking] ", end="", flush=True)
                in_thinking = True
            elif kind == "thinking_delta":
                print(event["text"], end="", flush=True)
            elif kind == "thinking_end":
                print("", flush=True)
                in_thinking = False
            elif kind == "tool_call":
                print(f"\n[tool] {event['name']}({event['args']})", flush=True)
            elif kind == "tool_result":
                print(f"\n[result] {event['content']}", flush=True)
            elif kind == "error":
                print(f"\n[error] {event['message']}", flush=True)
            elif kind == "done":
                print(flush=True)

    try:
        asyncio.run(_run())
    finally:
        col.close()


if __name__ == "__main__":
    main()
