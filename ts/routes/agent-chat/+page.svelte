<!--
Copyright: Ankitects Pty Ltd and contributors
License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html
-->
<script lang="ts">
    import { onMount } from "svelte";
    import { marked } from "marked";

    type Block =
        | { kind: "user"; text: string }
        | { kind: "assistant"; text: string }
        | { kind: "tool"; name: string; args: unknown; result: string | null };

    type Config = {
        has_key: boolean;
        model: string;
        model_options: string[];
    };

    let blocks: Block[] = [];
    let input = "";
    let busy = false;
    let error: string | null = null;
    let transcriptEl: HTMLElement;

    let config: Config | null = null;
    let settingsOpen = false;
    let apiKeyInput = "";
    let selectedModel = "";
    let saving = false;
    let testing = false;
    let testResult: { ok: boolean; error?: string } | null = null;

    onMount(async () => {
        await loadConfig();
        if (config && !config.has_key) {
            settingsOpen = true;
        }
    });

    async function loadConfig(): Promise<void> {
        try {
            const r = await fetch("/agent-chat/config");
            config = (await r.json()) as Config;
            selectedModel = config.model;
        } catch (e) {
            error = `failed to load config: ${e}`;
        }
    }

    async function saveSettings(): Promise<void> {
        if (saving) {
            return;
        }
        saving = true;
        testResult = null;
        const body: { api_key?: string; model?: string } = { model: selectedModel };
        if (apiKeyInput.trim()) {
            body.api_key = apiKeyInput.trim();
        }
        try {
            await fetch("/agent-chat/config", {
                method: "POST",
                headers: { "Content-Type": "application/json" },
                body: JSON.stringify(body),
            });
            apiKeyInput = "";
            await loadConfig();
        } catch (e) {
            error = String(e);
        } finally {
            saving = false;
        }
    }

    async function testConnection(): Promise<void> {
        if (testing) {
            return;
        }
        // save first so the test uses the latest input
        if (apiKeyInput.trim() || selectedModel !== config?.model) {
            await saveSettings();
        }
        testing = true;
        testResult = null;
        try {
            const r = await fetch("/agent-chat/test", { method: "POST" });
            testResult = await r.json();
        } catch (e) {
            testResult = { ok: false, error: String(e) };
        } finally {
            testing = false;
        }
    }

    function scrollToBottom(): void {
        queueMicrotask(() => {
            if (transcriptEl) {
                transcriptEl.scrollTop = transcriptEl.scrollHeight;
            }
        });
    }

    function appendAssistantText(delta: string): void {
        const last = blocks[blocks.length - 1];
        if (last && last.kind === "assistant") {
            last.text += delta;
            blocks = blocks;
        } else {
            blocks = [...blocks, { kind: "assistant", text: delta }];
        }
        scrollToBottom();
    }

    function appendToolCall(name: string, args: unknown): void {
        blocks = [...blocks, { kind: "tool", name, args, result: null }];
        scrollToBottom();
    }

    function appendToolResult(name: string, content: string): void {
        for (let i = blocks.length - 1; i >= 0; i--) {
            const b = blocks[i];
            if (b.kind === "tool" && b.name === name && b.result === null) {
                b.result = content;
                blocks = blocks;
                scrollToBottom();
                return;
            }
        }
        blocks = [...blocks, { kind: "tool", name, args: null, result: content }];
        scrollToBottom();
    }

    async function send(): Promise<void> {
        const message = input.trim();
        if (!message || busy) {
            return;
        }
        if (!config?.has_key) {
            error = "Add an API key in settings first.";
            settingsOpen = true;
            return;
        }
        input = "";
        error = null;
        busy = true;
        blocks = [...blocks, { kind: "user", text: message }];
        scrollToBottom();

        try {
            const resp = await fetch("/agent-chat/stream", {
                method: "POST",
                headers: { "Content-Type": "application/json" },
                body: JSON.stringify({ message }),
            });
            if (!resp.ok || !resp.body) {
                error = `${resp.status} ${resp.statusText}: ${await resp.text()}`;
                return;
            }
            const reader = resp.body.getReader();
            const decoder = new TextDecoder();
            let buffer = "";
            let streamDone = false;
            while (!streamDone) {
                const { value, done } = await reader.read();
                streamDone = done;
                if (!value) {
                    continue;
                }
                buffer += decoder.decode(value, { stream: true });
                let idx: number;
                while ((idx = buffer.indexOf("\n\n")) !== -1) {
                    const frame = buffer.slice(0, idx);
                    buffer = buffer.slice(idx + 2);
                    if (!frame.startsWith("data:")) {
                        continue;
                    }
                    const payload = frame.slice(5).trimStart();
                    let event: { type: string; [k: string]: unknown };
                    try {
                        event = JSON.parse(payload);
                    } catch {
                        continue;
                    }
                    if (event.type === "text_delta") {
                        appendAssistantText(event.text as string);
                    } else if (event.type === "tool_call") {
                        appendToolCall(event.name as string, event.args);
                    } else if (event.type === "tool_result") {
                        appendToolResult(event.name as string, event.content as string);
                    } else if (event.type === "error") {
                        error = event.message as string;
                    }
                }
            }
        } catch (e) {
            error = String(e);
        } finally {
            busy = false;
        }
    }

    function onKeydown(e: KeyboardEvent): void {
        if (e.key === "Enter" && !e.shiftKey) {
            e.preventDefault();
            send();
        }
    }
</script>

<svelte:head>
    <title>Anki AI</title>
</svelte:head>

<div class="chat">
    <header class="chat-header">
        <span class="model-label" title="Active model">
            {config?.model ?? "…"}
            {#if config && !config.has_key}<span class="warn">· no key</span>{/if}
        </span>
        <button
            type="button"
            class="gear"
            title="Settings"
            on:click={() => (settingsOpen = !settingsOpen)}
        >
            ⚙
        </button>
    </header>

    {#if settingsOpen}
        <section class="settings">
            <label>
                <span>OpenAI API key{config?.has_key ? " (saved)" : ""}</span>
                <input
                    type="password"
                    autocomplete="off"
                    placeholder={config?.has_key ? "leave blank to keep current" : "sk-…"}
                    bind:value={apiKeyInput}
                />
            </label>
            <label>
                <span>Model</span>
                <select bind:value={selectedModel}>
                    {#each config?.model_options ?? [] as m}
                        <option value={m}>{m}</option>
                    {/each}
                    {#if config && !config.model_options.includes(config.model)}
                        <option value={config.model}>{config.model}</option>
                    {/if}
                </select>
            </label>
            <div class="settings-actions">
                <button
                    type="button"
                    on:click={saveSettings}
                    disabled={saving || (!apiKeyInput.trim() && selectedModel === config?.model)}
                >
                    {saving ? "Saving…" : "Save"}
                </button>
                <button type="button" on:click={testConnection} disabled={testing || !config?.has_key}>
                    {testing ? "Testing…" : "Test connection"}
                </button>
                {#if testResult}
                    <span class="test-result" class:ok={testResult.ok} class:bad={!testResult.ok}>
                        {testResult.ok ? "✓ ok" : `✗ ${testResult.error ?? "failed"}`}
                    </span>
                {/if}
            </div>
            <p class="hint">
                Your key is stored in your local Anki profile only and never leaves your
                machine except to call OpenAI directly.
            </p>
        </section>
    {/if}

    <div class="transcript" bind:this={transcriptEl}>
        {#if blocks.length === 0 && !settingsOpen}
            <div class="hint">
                Ask the agent to search, read, or edit your notes. Every edit is
                versioned — revert from the editor's versions sidebar.
            </div>
        {/if}
        {#each blocks as block (block)}
            {#if block.kind === "user"}
                <div class="msg user">{block.text}</div>
            {:else if block.kind === "assistant"}
                <div class="msg assistant">
                    {@html marked.parse(block.text)}
                </div>
            {:else}
                <details class="msg tool">
                    <summary>
                        <code>{block.name}</code>
                        {#if block.result === null}<span class="pending">…</span>{/if}
                    </summary>
                    {#if block.args !== null}
                        <div class="tool-section">
                            <strong>args</strong>
                            <pre>{JSON.stringify(block.args, null, 2)}</pre>
                        </div>
                    {/if}
                    {#if block.result !== null}
                        <div class="tool-section">
                            <strong>result</strong>
                            <pre>{block.result}</pre>
                        </div>
                    {/if}
                </details>
            {/if}
        {/each}
        {#if error}
            <div class="msg error">{error}</div>
        {/if}
    </div>

    <form class="composer" on:submit|preventDefault={send}>
        <textarea
            bind:value={input}
            on:keydown={onKeydown}
            placeholder={busy ? "Thinking…" : "Message the agent (Enter to send, Shift+Enter for newline)"}
            disabled={busy}
            rows="3"
        ></textarea>
        <button type="submit" disabled={busy || !input.trim()}>Send</button>
    </form>
</div>

<style lang="scss">
    .chat {
        display: flex;
        flex-direction: column;
        height: 100vh;
        font-family: system-ui, -apple-system, sans-serif;
        font-size: 14px;
        background: var(--canvas, #fff);
        color: var(--fg, #111);
    }

    .chat-header {
        display: flex;
        align-items: center;
        justify-content: space-between;
        padding: 6px 10px;
        border-bottom: 1px solid rgba(0, 0, 0, 0.1);
        font-size: 12px;

        .model-label {
            color: var(--fg-subtle, #555);
            font-family: ui-monospace, monospace;
        }
        .warn {
            color: #b45309;
            margin-left: 6px;
        }
        .gear {
            background: none;
            border: none;
            cursor: pointer;
            font-size: 16px;
            padding: 2px 6px;
            color: var(--fg-subtle, #555);

            &:hover {
                color: var(--fg, #111);
            }
        }
    }

    .settings {
        display: flex;
        flex-direction: column;
        gap: 8px;
        padding: 10px;
        background: rgba(0, 0, 0, 0.03);
        border-bottom: 1px solid rgba(0, 0, 0, 0.1);

        label {
            display: flex;
            flex-direction: column;
            gap: 3px;
            font-size: 12px;
            color: var(--fg-subtle, #555);

            input,
            select {
                font: inherit;
                font-size: 13px;
                padding: 5px 7px;
                border: 1px solid rgba(0, 0, 0, 0.2);
                border-radius: 4px;
                color: var(--fg, #111);
                background: var(--canvas, #fff);
            }
        }

        .settings-actions {
            display: flex;
            gap: 6px;
            align-items: center;

            button {
                padding: 4px 10px;
                border: 1px solid rgba(0, 0, 0, 0.2);
                border-radius: 4px;
                background: var(--canvas, #fff);
                cursor: pointer;
                font: inherit;
                font-size: 12px;

                &:disabled {
                    opacity: 0.5;
                    cursor: not-allowed;
                }
            }
            .test-result {
                font-size: 12px;
                font-family: ui-monospace, monospace;
            }
            .test-result.ok {
                color: #15803d;
            }
            .test-result.bad {
                color: #b91c1c;
            }
        }

        .hint {
            font-size: 11px;
            color: var(--fg-subtle, #888);
            margin: 0;
        }
    }

    .transcript {
        flex: 1;
        overflow-y: auto;
        padding: 12px;
        display: flex;
        flex-direction: column;
        gap: 8px;
    }

    .hint {
        color: var(--fg-subtle, #888);
        text-align: center;
        padding: 24px 12px;
        font-size: 13px;
    }

    .msg {
        padding: 8px 12px;
        border-radius: 8px;
        max-width: 95%;
        line-height: 1.4;
        word-wrap: break-word;
        white-space: pre-wrap;

        :global(p) {
            margin: 0 0 8px;
        }
        :global(p:last-child) {
            margin-bottom: 0;
        }
        :global(pre) {
            background: rgba(0, 0, 0, 0.06);
            padding: 6px 8px;
            border-radius: 4px;
            overflow-x: auto;
        }
        :global(code) {
            font-family: ui-monospace, monospace;
            font-size: 12px;
        }
    }

    .msg.user {
        align-self: flex-end;
        background: #2563eb;
        color: white;
        white-space: pre-wrap;
    }

    .msg.assistant {
        align-self: flex-start;
        background: rgba(0, 0, 0, 0.04);
        white-space: normal;
    }

    .msg.tool {
        align-self: flex-start;
        background: rgba(0, 0, 0, 0.02);
        border: 1px solid rgba(0, 0, 0, 0.08);
        padding: 6px 10px;
        font-size: 12px;

        summary {
            cursor: pointer;
            color: var(--fg-subtle, #555);
        }
        .tool-section {
            margin-top: 6px;
        }
        pre {
            background: rgba(0, 0, 0, 0.06);
            padding: 4px 6px;
            border-radius: 4px;
            overflow-x: auto;
            margin: 4px 0 0;
            white-space: pre-wrap;
        }
        .pending {
            color: var(--fg-subtle, #888);
        }
    }

    .msg.error {
        align-self: stretch;
        background: #fee2e2;
        color: #991b1b;
        font-family: ui-monospace, monospace;
        font-size: 12px;
    }

    .composer {
        display: flex;
        gap: 6px;
        padding: 8px;
        border-top: 1px solid rgba(0, 0, 0, 0.1);

        textarea {
            flex: 1;
            resize: vertical;
            font: inherit;
            padding: 6px 8px;
            border: 1px solid rgba(0, 0, 0, 0.2);
            border-radius: 6px;
        }
        button {
            padding: 0 14px;
            border: none;
            border-radius: 6px;
            background: #2563eb;
            color: white;
            cursor: pointer;
            font: inherit;

            &:disabled {
                opacity: 0.5;
                cursor: not-allowed;
            }
        }
    }
</style>
