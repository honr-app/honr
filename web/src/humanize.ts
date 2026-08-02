/** Plain-language rewrites for agent/escalation dumps shown in the cockpit. */

export function humanizeEscalation(question: string): {
  summary: string;
  detail: string | null;
} {
  const q = question.replace(/\s+/g, " ").trim();
  if (/unable to access|clone failed|CONNECT tunnel|response 403/i.test(q)) {
    return {
      summary:
        "Sandbox couldn't clone the repo — credentials, network policy, or a bad fork URL.",
      detail: q,
    };
  }
  if (/permission denied|authentication failed|could not read Username/i.test(q)) {
    return {
      summary: "Git authentication failed inside the sandbox.",
      detail: q,
    };
  }
  const last = q.match(/Last failure:\s*(.*)$/i);
  if (/failed to run \d+ times/i.test(q) && last) {
    const head = q.slice(0, last.index).trim();
    return {
      summary: head || "This task failed repeatedly without producing work.",
      detail: last[1]?.trim() || q,
    };
  }
  if (q.length > 180) {
    return { summary: `${q.slice(0, 160).trim()}…`, detail: q };
  }
  return { summary: q, detail: null };
}

export function friendlyState(state: string): string {
  switch (state) {
    case "needs_human":
      return "needs you";
    case "claimed":
    case "running":
    case "splitting":
      return "working";
    case "ready":
      return "backlog";
    default:
      return state.replace(/_/g, " ");
  }
}
