import { useCallback, useEffect, useId, useRef, useState } from "react";
import { api } from "../api.js";
import type { OpsSession } from "../types.js";

const POLL_MS = 4000;

function statusLabel(status: OpsSession["status"]): string {
  return status === "parked" ? "Parked" : "Running";
}

export type CockpitChatRole = "user" | "assistant" | "system";

export type CockpitChatMessage = {
  id: string;
  role: CockpitChatRole;
  text: string;
  streaming?: boolean;
};

/**
 * Extract displayable assistant text from one ops-chat `agent` SSE line
 * (Cursor/Claude stream-json or plain stdout). Returns null to skip.
 */
export function opsChatLineText(line: string): string | null {
  const trimmed = line.trim();
  if (!trimmed) return null;
  if (!trimmed.startsWith("{")) return trimmed;

  try {
    const obj = JSON.parse(trimmed) as Record<string, unknown>;

    if (obj.type === "thinking") return null;
    if (obj.type === "tool_call" || obj.event === "step_update") return null;
    if (
      typeof obj.type === "string" &&
      [
        "message_start",
        "message_delta",
        "message_stop",
        "content_block_stop",
        "ping",
        "system",
        "result",
      ].includes(obj.type)
    ) {
      return null;
    }

    if (obj.type === "content_block_start") {
      const cb = obj.content_block as { type?: string; text?: string } | undefined;
      if (cb?.type === "text" && cb.text) return cb.text;
      return null;
    }

    if (obj.type === "content_block_delta") {
      const delta = obj.delta as { type?: string; text?: string } | undefined;
      if (delta?.text) return delta.text;
      return null;
    }

    if (obj.type === "error" || obj.error) {
      const err = obj.error;
      const msg =
        typeof err === "string"
          ? err
          : err && typeof err === "object" && "message" in err
            ? String((err as { message: unknown }).message)
            : JSON.stringify(err ?? obj);
      return `Error: ${msg}`;
    }

    const content =
      (obj.message as { content?: unknown } | undefined)?.content ?? obj.content;
    if (Array.isArray(content)) {
      const parts: string[] = [];
      for (const item of content) {
        if (
          item &&
          typeof item === "object" &&
          (item as { type?: string }).type === "text" &&
          typeof (item as { text?: unknown }).text === "string"
        ) {
          parts.push((item as { text: string }).text);
        }
      }
      return parts.length > 0 ? parts.join("") : null;
    }
    if (typeof content === "string" && content.trim()) return content;

    return null;
  } catch {
    return trimmed;
  }
}

/** Why the composer is locked — mirrors Board session readiness only. */
export function cockpitChatGate(
  session: OpsSession | null,
): { canSend: boolean; reason: string | null } {
  if (session == null) {
    return { canSend: false, reason: "Start an ops session to chat with the seat." };
  }
  if (session.status === "parked") {
    return { canSend: false, reason: "Resume the ops session to continue chatting." };
  }
  const environment = session.environment?.trim();
  if (!environment) {
    return {
      canSend: false,
      reason: "Waiting for the supervisor to provision the ops environment…",
    };
  }
  return { canSend: true, reason: null };
}

/**
 * Presentational ops-session face — exported so tests render without fetch.
 * Action enablement mirrors Board rules; mutations stay on the parent.
 */
export function CockpitSessionView({
  session,
  loading,
  busy,
  error,
  onStart,
  onPark,
  onResume,
  onStop,
}: {
  session: OpsSession | null;
  loading?: boolean;
  busy?: boolean;
  error?: string | null;
  onStart: () => void;
  onPark: () => void;
  onResume: () => void;
  onStop: () => void;
}) {
  const absent = session == null;
  const running = session?.status === "running";
  const parked = session?.status === "parked";
  const environment = session?.environment?.trim() || null;
  const conversationId = session?.conversation_id?.trim() || null;

  return (
    <section
      className="cockpit-session"
      aria-labelledby="cockpit-session-title"
      data-testid="cockpit-session"
    >
      <h2 id="cockpit-session-title">Ops session</h2>
      <p className="dim cockpit-session-lede">
        Board owns lifecycle. These controls call{" "}
        <code>/api/ops-session*</code> only — status below is polled from the
        Board, not a second state machine.
      </p>

      {error && (
        <div className="err" data-testid="cockpit-session-error">
          {error}
        </div>
      )}

      <dl className="cockpit-session-status" data-testid="cockpit-session-status">
        <div>
          <dt>Status</dt>
          <dd data-testid="cockpit-session-status-value">
            {loading && absent
              ? "Loading…"
              : absent
                ? "None"
                : statusLabel(session.status)}
          </dd>
        </div>
        <div>
          <dt>Environment</dt>
          <dd data-testid="cockpit-session-environment">
            {environment ?? "—"}
          </dd>
        </div>
        {conversationId && (
          <div>
            <dt>Conversation</dt>
            <dd data-testid="cockpit-session-conversation">{conversationId}</dd>
          </div>
        )}
      </dl>

      <div className="cockpit-session-actions" data-testid="cockpit-session-actions">
        <button
          type="button"
          className="primary"
          disabled={busy || !absent}
          onClick={onStart}
          data-testid="cockpit-session-start"
        >
          Start
        </button>
        <button
          type="button"
          disabled={busy || !running}
          onClick={onPark}
          data-testid="cockpit-session-park"
        >
          Park
        </button>
        <button
          type="button"
          disabled={busy || !parked}
          onClick={onResume}
          data-testid="cockpit-session-resume"
        >
          Resume
        </button>
        <button
          type="button"
          className="danger"
          disabled={busy || absent}
          onClick={onStop}
          data-testid="cockpit-session-stop"
        >
          Stop
        </button>
      </div>

      {environment && (
        <p className="dim cockpit-attach-fallback" data-testid="cockpit-attach-fallback">
          Optional TTY fallback:{" "}
          <code>openshell sandbox connect {environment}</code>
        </p>
      )}
    </section>
  );
}

/**
 * Presentational ops chat — message list + composer over /api/ops-chat.
 * Enablement comes from Board session status only; no local lifecycle.
 */
export function CockpitChatView({
  messages,
  canSend,
  disabledReason,
  streaming,
  error,
  draft,
  onDraftChange,
  onSend,
}: {
  messages: CockpitChatMessage[];
  canSend: boolean;
  disabledReason?: string | null;
  streaming?: boolean;
  error?: string | null;
  draft: string;
  onDraftChange: (value: string) => void;
  onSend: () => void;
}) {
  const listRef = useRef<HTMLDivElement>(null);
  const titleId = useId();
  const composerDisabled = !canSend || !!streaming;
  const sendDisabled = composerDisabled || !draft.trim();

  useEffect(() => {
    const el = listRef.current;
    if (!el) return;
    el.scrollTop = el.scrollHeight;
  }, [messages]);

  return (
    <section
      className="cockpit-chat"
      aria-labelledby={titleId}
      data-testid="cockpit-chat"
    >
      <h2 id={titleId}>Ops chat</h2>
      <p className="dim cockpit-chat-lede">
        Primary attach: prompts go through the host{" "}
        <code>/api/ops-chat</code> bridge into the Board-named seat. Transcript
        below is browser-local — conversation id stays on the Board session.
      </p>

      {error && (
        <div className="err" data-testid="cockpit-chat-error">
          {error}
        </div>
      )}

      <div
        className="cockpit-chat-messages"
        ref={listRef}
        data-testid="cockpit-chat-messages"
        role="log"
        aria-live="polite"
      >
        {messages.length === 0 && (
          <p className="dim cockpit-chat-empty" data-testid="cockpit-chat-empty">
            {disabledReason ??
              "Send a prompt to steer the board via the ops seat."}
          </p>
        )}
        {messages.map((m) => (
          <div
            key={m.id}
            className={`cockpit-chat-bubble cockpit-chat-bubble-${m.role}${
              m.streaming ? " cockpit-chat-bubble-streaming" : ""
            }`}
            data-testid={`cockpit-chat-msg-${m.role}`}
            data-role={m.role}
          >
            <span className="cockpit-chat-role">
              {m.role === "user"
                ? "You"
                : m.role === "assistant"
                  ? "Ops"
                  : "System"}
            </span>
            <pre className="cockpit-chat-text">
              {m.text || (m.streaming ? "…" : "")}
            </pre>
          </div>
        ))}
      </div>

      {disabledReason && !streaming && (
        <p className="dim cockpit-chat-gate" data-testid="cockpit-chat-gate">
          {disabledReason}
        </p>
      )}

      <form
        className="cockpit-chat-composer"
        data-testid="cockpit-chat-composer"
        onSubmit={(e) => {
          e.preventDefault();
          if (!sendDisabled) onSend();
        }}
      >
        <textarea
          value={draft}
          onChange={(e) => onDraftChange(e.target.value)}
          disabled={composerDisabled}
          rows={3}
          placeholder={
            canSend
              ? "Message the ops seat…"
              : "Chat unavailable until the session is Running"
          }
          data-testid="cockpit-chat-input"
          onKeyDown={(e) => {
            if (e.key === "Enter" && !e.shiftKey) {
              e.preventDefault();
              if (!sendDisabled) onSend();
            }
          }}
        />
        <button
          type="submit"
          className="primary"
          disabled={sendDisabled}
          data-testid="cockpit-chat-send"
        >
          {streaming ? "Streaming…" : "Send"}
        </button>
      </form>
    </section>
  );
}

/**
 * Cockpit — primary-nav surface for steering the board via the ops seat.
 * Chat is the primary attach UX; session controls are a thin face over
 * /api/ops-session*. No local session files or shadow lifecycle.
 */
export function Cockpit() {
  const [session, setSession] = useState<OpsSession | null>(null);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const [messages, setMessages] = useState<CockpitChatMessage[]>([]);
  const [draft, setDraft] = useState("");
  const [streaming, setStreaming] = useState(false);
  const [chatError, setChatError] = useState<string | null>(null);
  const abortRef = useRef<AbortController | null>(null);
  const msgSeq = useRef(0);

  const nextId = () => {
    msgSeq.current += 1;
    return `m-${msgSeq.current}`;
  };

  const refresh = useCallback(async () => {
    try {
      const out = await api.getOpsSession();
      setSession(out.session ?? null);
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    let alive = true;
    const tick = () => {
      if (!alive) return;
      void refresh();
    };
    tick();
    const poll = setInterval(tick, POLL_MS);
    return () => {
      alive = false;
      clearInterval(poll);
      abortRef.current?.abort();
    };
  }, [refresh]);

  // Drop the browser transcript when the Board session is gone — not a
  // lifecycle store, just avoid showing stale turns after Stop.
  useEffect(() => {
    if (session == null && messages.length > 0 && !streaming) {
      setMessages([]);
      setChatError(null);
    }
  }, [session, messages.length, streaming]);

  const runAction = useCallback(
    async (action: () => Promise<unknown>) => {
      setBusy(true);
      setError(null);
      try {
        await action();
        await refresh();
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e));
        // Board may still have moved — refetch so the face stays honest.
        await refresh();
      } finally {
        setBusy(false);
      }
    },
    [refresh],
  );

  const gate = cockpitChatGate(session);

  const sendChat = useCallback(async () => {
    const prompt = draft.trim();
    if (!prompt || streaming || !gate.canSend) return;

    const userId = nextId();
    const assistantId = nextId();
    setDraft("");
    setChatError(null);
    setMessages((prev) => [
      ...prev,
      { id: userId, role: "user", text: prompt },
      { id: assistantId, role: "assistant", text: "", streaming: true },
    ]);
    setStreaming(true);

    const ac = new AbortController();
    abortRef.current = ac;

    const appendAssistant = (chunk: string) => {
      if (!chunk) return;
      setMessages((prev) =>
        prev.map((m) =>
          m.id === assistantId ? { ...m, text: m.text + chunk } : m,
        ),
      );
    };

    try {
      await api.streamOpsChat(prompt, {
        signal: ac.signal,
        onAgentLine: (line) => {
          const text = opsChatLineText(line);
          if (text) appendAssistant(text);
        },
        onError: (message) => {
          setChatError(message);
          setMessages((prev) =>
            prev.map((m) =>
              m.id === assistantId && !m.text
                ? { ...m, text: message, streaming: false }
                : m,
            ),
          );
        },
      });
    } catch (e) {
      if (ac.signal.aborted) return;
      const msg = e instanceof Error ? e.message : String(e);
      setChatError(msg);
      setMessages((prev) =>
        prev.map((m) =>
          m.id === assistantId && !m.text
            ? { ...m, role: "system", text: msg, streaming: false }
            : m,
        ),
      );
    } finally {
      setMessages((prev) =>
        prev.map((m) =>
          m.id === assistantId ? { ...m, streaming: false } : m,
        ),
      );
      setStreaming(false);
      if (abortRef.current === ac) abortRef.current = null;
      // conversation_id may have landed on the Board mid-turn
      await refresh();
    }
  }, [draft, streaming, gate.canSend, refresh]);

  return (
    <div className="cockpit-page" data-testid="cockpit-page">
      <header className="board-hero">
        <h1>Cockpit</h1>
        <p className="board-lede">
          Steer the board by chatting with the ops seat in the browser. Session
          Start / Park / Resume / Stop call Board{" "}
          <code>/api/ops-session*</code>; chat uses the host{" "}
          <code>/api/ops-chat</code> bridge. Optional{" "}
          <code>openshell sandbox connect</code> remains a TTY fallback.
        </p>
      </header>

      <CockpitChatView
        messages={messages}
        canSend={gate.canSend}
        disabledReason={gate.reason}
        streaming={streaming}
        error={chatError}
        draft={draft}
        onDraftChange={setDraft}
        onSend={() => void sendChat()}
      />

      <CockpitSessionView
        session={session}
        loading={loading}
        busy={busy}
        error={error}
        onStart={() => void runAction(() => api.startOpsSession())}
        onPark={() => void runAction(() => api.parkOpsSession())}
        onResume={() => void runAction(() => api.resumeOpsSession())}
        onStop={() => void runAction(() => api.stopOpsSession())}
      />
    </div>
  );
}
