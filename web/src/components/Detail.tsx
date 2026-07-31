import { useEffect, useState } from "react";
import { api, money, since } from "../api";
import type { WorkItem } from "../types";

interface Detail extends WorkItem {
  ancestry: { level: string; title: string; intent: string }[];
  constraints: string[];
  children: number[];
}

/** Layer 3: is this right? Transcript, diff, cost — and why it exists at all. */
export function DetailDrawer({
  id,
  now,
  onClose,
  onChanged,
}: {
  id: number;
  now: number;
  onClose: () => void;
  onChanged: () => void;
}) {
  const [d, setD] = useState<Detail | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [text, setText] = useState("");

  const load = () =>
    api
      .detail(id)
      .then((x) => setD(x as Detail))
      .catch((e) => setErr(String(e)));

  useEffect(() => {
    setD(null);
    setErr(null);
    load();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [id]);

  const act = (p: Promise<unknown>) =>
    p.then(() => { load(); onChanged(); }).catch((e) => setErr(String(e)));

  if (err && !d) return <aside className="drawer"><Head onClose={onClose} title={`#${id}`} /><div className="err">{err}</div></aside>;
  if (!d) return <aside className="drawer"><Head onClose={onClose} title={`#${id}`} /><div className="dim">loading…</div></aside>;

  return (
    <aside className="drawer">
      <Head onClose={onClose} title={`#${d.id} ${d.title}`} />
      {err && <div className="err">{err}</div>}

      <div className="pill-row">
        <span className="pill">{d.state}</span>
        {d.level && <span className="pill">{d.level}</span>}
        <span className="pill">{money(d.cost_cents)}</span>
        {d.origin.kind !== "human" && <span className="pill machine">{d.origin.kind}-born</span>}
      </div>

      {/* The highest-leverage payload in the system: sixty words that tell an
          agent why it is writing this code. */}
      <Section title="Intent chain">
        <div className="chain">
          {d.ancestry.map((a, n) => (
            <div key={n} className="chain-line">
              <span className="chain-level">{a.level.toUpperCase()}</span>
              <span>{a.intent}</span>
            </div>
          ))}
          {d.definition_of_done && (
            <div className="chain-line dod">
              <span className="chain-level">DoD</span>
              <span>{d.definition_of_done}</span>
            </div>
          )}
        </div>
      </Section>

      {d.constraints.length > 0 && (
        <Section title="Inherited constraints">
          <ul className="plain">
            {d.constraints.map((c, n) => <li key={n}>📌 {c}</li>)}
          </ul>
        </Section>
      )}

      {(d.pr_url || d.environment) && (
        <Section title="This run">
          {d.pr_url && (
            <p>
              <a className="pr-link" href={d.pr_url} target="_blank" rel="noreferrer">
                ↗ {d.pr_url}
              </a>
            </p>
          )}
          {d.environment && <p className="dim">sandbox {d.environment}</p>}
        </Section>
      )}

      {d.escalation && (
        <Section title="Waiting on you">
          <p className="question">{d.escalation.question}</p>
          <p className="dim">blocked {since(d.escalation.blocked_since, now)}</p>
          {d.escalation.options.map((o, n) => (
            <div key={n} className="option">
              <button
                className={n === d.escalation!.recommended ? "primary" : ""}
                onClick={() => act(api.answer(d.id, o.label))}
              >
                {o.label}
                {n === d.escalation!.recommended && " ★"}
              </button>
              <span className="dim">{o.detail}</span>
            </div>
          ))}
        </Section>
      )}

      {d.state === "review" && (
        <Section title="Review">
          <p className="dim">
            +{d.diff_added} −{d.diff_removed} · gates {d.gates.map((g) => g.name).join(", ")}
          </p>
          {d.pr_url && (
            <p>
              <a className="pr-link" href={d.pr_url} target="_blank" rel="noreferrer">
                ↗ review the pull request
              </a>
            </p>
          )}
          {/* The note lives with the button that sends it. Previously it was a
              shared textarea in a different section, so it was not obvious
              that Request changes needed one — or that it took the note at all. */}
          <textarea
            rows={2}
            value={text}
            placeholder="What needs to change? This reaches the next agent that picks it up."
            onChange={(e) => setText(e.target.value)}
          />
          <div className="btns">
            <button className="primary" onClick={() => act(api.approve(d.id))}>Approve</button>
            <button
              disabled={!text.trim()}
              title={text.trim() ? "" : "Say what needs changing first"}
              onClick={() => { act(api.requestChanges(d.id, text)); setText(""); }}
            >
              Request changes
            </button>
          </div>
        </Section>
      )}

      {/* Steer, Pin, Halt and Cut moved to the agent. They all want prose — a
          reason, a constraint, a judgement — and a textarea with four buttons
          made Steer look like it would reach a running agent, which it cannot.
          What stays here is what is genuinely one tap. */}
      <Section title="Ask the agent">
        <p className="dim">
          To steer this, pin a constraint, halt it or cut it, tell the agent —
          those need a reason, and it can act on one.
        </p>
      </Section>

      {d.notes.length > 0 && (
        <Section title="Notes">
          <ul className="plain">
            {d.notes.map((n, i) => (
              <li key={i}><span className="dim">{n.author}</span> {n.text}</li>
            ))}
          </ul>
        </Section>
      )}

      <Section title="History">
        <ul className="plain hist">
          {d.history.slice(-12).reverse().map((h, i) => (
            <li key={i}>
              <span className="dim">{since(h.at, now)} ago</span> {h.from} → <b>{h.to}</b>
              <span className="dim"> by {h.by}</span>
              {h.reason && <div className="dim reason">{h.reason}</div>}
            </li>
          ))}
        </ul>
      </Section>
    </aside>
  );
}

const Head = ({ title, onClose }: { title: string; onClose: () => void }) => (
  <div className="drawer-head">
    <h3>{title}</h3>
    <button className="close" onClick={onClose}>✕</button>
  </div>
);

const Section = ({ title, children }: { title: string; children: React.ReactNode }) => (
  <div className="section">
    <div className="section-title">{title}</div>
    {children}
  </div>
);
