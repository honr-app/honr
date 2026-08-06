//! Explicit agent-engine registry.
//!
//! Launch argv, resume support, conversation-id JSON pointers, and optional
//! pre-start auth live in one table. Unknown engine ids fail loudly — they must
//! never fall through to a silent `claude` default.

use std::fmt;

/// How a parked conversation/session id is passed on the CLI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResumeFlag {
    /// `agy … --conversation "$HONR_CONVERSATION" …`
    Conversation,
    /// Cursor Agent CLI: `agent … --resume "$HONR_CONVERSATION" …`
    Resume,
    /// OpenCode CLI: `opencode run … --session "$HONR_CONVERSATION" …`
    Session,
}

impl ResumeFlag {
    fn argv(self) -> &'static str {
        match self {
            Self::Conversation => "--conversation \"$HONR_CONVERSATION\"",
            Self::Resume => "--resume \"$HONR_CONVERSATION\"",
            Self::Session => "--session \"$HONR_CONVERSATION\"",
        }
    }
}

/// Optional sandbox prep before the engine binary runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreStartAuth {
    None,
    /// Write Gemini/agy settings (+ optional host oauth token) into the sandbox.
    Agy,
}

/// Where the prompt / briefing env var is placed relative to fixed flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PromptStyle {
    /// `-p "$VAR"` after fixed flags (and resume, if any).
    FlagP,
    /// Trailing positional `"$VAR"` (Cursor Agent CLI).
    Positional,
}

/// One registered engine.
#[derive(Debug, Clone, Copy)]
pub struct Engine {
    pub id: &'static str,
    /// Binary + fixed flags, excluding resume and prompt.
    prefix: &'static str,
    /// Flags after the prompt (claude puts format/permission flags here).
    trailing: &'static str,
    prompt: PromptStyle,
    pub resume: Option<ResumeFlag>,
    /// JSON pointer paths walked by [`conversation_id_pointers`] / parsers.
    pub conversation_id_pointers: &'static [&'static str],
    pub pre_start_auth: PreStartAuth,
}

const AGY_CONV_KEYS: &[&str] = &[
    "/conversation_id",
    "/step_update/conversation_id",
    "/result/conversation_id",
    "/message/conversation_id",
];

const CURSOR_SESSION_KEYS: &[&str] = &["/session_id", "/result/session_id"];

/// OpenCode `--format json` lines use camelCase `sessionID` (see their run stream).
const OPENCODE_SESSION_KEYS: &[&str] = &["/sessionID", "/part/sessionID"];

/// Known engines. Order is display-stable; lookup is by `id`.
pub const ENGINES: &[Engine] = &[
    Engine {
        id: "cursor",
        prefix: "agent -p --force --trust --sandbox disabled --output-format stream-json",
        trailing: "",
        prompt: PromptStyle::Positional,
        resume: Some(ResumeFlag::Resume),
        conversation_id_pointers: CURSOR_SESSION_KEYS,
        pre_start_auth: PreStartAuth::None,
    },
    Engine {
        id: "agy",
        prefix: "agy --dangerously-skip-permissions --print-timeout 24h --output-format stream-json",
        trailing: "",
        prompt: PromptStyle::FlagP,
        resume: Some(ResumeFlag::Conversation),
        conversation_id_pointers: AGY_CONV_KEYS,
        pre_start_auth: PreStartAuth::Agy,
    },
    Engine {
        id: "claude",
        // `--bare` skips Claude Code's OAuth and MCP auto-discovery. Auth is
        // OpenShell inference.local (see [`anthropic_inference_env`]); MCP is
        // the injected cockpit/seat file via `--mcp-config`.
        prefix: "claude --bare --strict-mcp-config --mcp-config /sandbox/.honr/mcp/claude_mcp.json",
        trailing: "--output-format stream-json --verbose --permission-mode bypassPermissions",
        prompt: PromptStyle::FlagP,
        resume: None,
        conversation_id_pointers: &[],
        pre_start_auth: PreStartAuth::None,
    },
    Engine {
        id: "opencode",
        // Headless one-shot: JSONL events on stdout, auto-approve tool perms
        // (sandbox is already the containment boundary). Prompt is positional.
        prefix: "opencode run --format json --auto",
        trailing: "",
        prompt: PromptStyle::Positional,
        resume: Some(ResumeFlag::Session),
        conversation_id_pointers: OPENCODE_SESSION_KEYS,
        pre_start_auth: PreStartAuth::None,
    },
];

/// Env var name substituted into the engine command for the prompt/briefing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptEnv {
    /// Supervisor detached start: `HONR_BRIEFING`.
    Briefing,
    /// Cockpit foreground turn: `HONR_PROMPT`.
    Prompt,
}

impl PromptEnv {
    fn shell_ref(self) -> &'static str {
        match self {
            Self::Briefing => "\"$HONR_BRIEFING\"",
            Self::Prompt => "\"$HONR_PROMPT\"",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownEngine {
    pub id: String,
}

impl fmt::Display for UnknownEngine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "unknown agent engine {:?}; expected one of: {}",
            self.id,
            ENGINES
                .iter()
                .map(|e| e.id)
                .collect::<Vec<_>>()
                .join(", ")
        )
    }
}

impl std::error::Error for UnknownEngine {}

/// Look up a registered engine. Unknown ids fail — no Claude fallthrough.
pub fn lookup(id: &str) -> Result<&'static Engine, UnknownEngine> {
    let id = id.trim();
    ENGINES
        .iter()
        .find(|e| e.id == id)
        .ok_or_else(|| UnknownEngine { id: id.to_string() })
}

/// Whether this engine can park/resume via a conversation or session id.
pub fn supports_resume(id: &str) -> bool {
    lookup(id).map(|e| e.resume.is_some()).unwrap_or(false)
}

/// Pre-start auth hook for this engine, if any.
pub fn pre_start_auth(id: &str) -> Result<PreStartAuth, UnknownEngine> {
    Ok(lookup(id)?.pre_start_auth)
}

/// Sandbox env so Anthropic-shaped CLIs hit OpenShell `inference.local`.
///
/// The gateway holds Vertex (or other) credentials and injects them on egress.
/// Do not set `CLAUDE_CODE_USE_VERTEX` — that forces direct ADC/metadata discovery.
/// OpenCode needs the `/v1` suffix; Claude Code appends `/v1/messages` itself.
pub fn anthropic_inference_env(engine_id: &str) -> Vec<(String, String)> {
    match engine_id.trim() {
        "opencode" => vec![
            (
                "ANTHROPIC_BASE_URL".into(),
                "https://inference.local/v1".into(),
            ),
            ("ANTHROPIC_API_KEY".into(), "unused".into()),
        ],
        "claude" => vec![
            (
                "ANTHROPIC_BASE_URL".into(),
                "https://inference.local".into(),
            ),
            ("ANTHROPIC_API_KEY".into(), "unused".into()),
        ],
        _ => Vec::new(),
    }
}

/// Shell exports mirrored into start/turn scripts so a reused sandbox still
/// picks up the route without recreate.
pub fn anthropic_inference_exports(engine_id: &str) -> &'static str {
    match engine_id.trim() {
        "opencode" => {
            "export ANTHROPIC_BASE_URL=https://inference.local/v1\n\
             export ANTHROPIC_API_KEY=unused\n\
             unset CLAUDE_CODE_USE_VERTEX\n"
        }
        "claude" => {
            "export ANTHROPIC_BASE_URL=https://inference.local\n\
             export ANTHROPIC_API_KEY=unused\n\
             unset CLAUDE_CODE_USE_VERTEX\n"
        }
        _ => "",
    }
}

/// Build the inner agent command line (no `timeout` / `setsid` wrapper).
///
/// `conversation_id` is only honored when the engine declares a [`ResumeFlag`];
/// callers that pass `Some` for a non-resumable engine still get a fresh argv
/// (resume gate belongs at the call site).
pub fn command_line(
    engine_id: &str,
    prompt: PromptEnv,
    conversation_id: Option<&str>,
) -> Result<String, UnknownEngine> {
    let engine = lookup(engine_id)?;
    let resume = conversation_id.is_some().then_some(engine.resume).flatten();

    let mut parts: Vec<&str> = Vec::new();
    parts.push(engine.prefix);
    if let Some(flag) = resume {
        parts.push(flag.argv());
    }
    match engine.prompt {
        PromptStyle::FlagP => {
            parts.push("-p");
            parts.push(prompt.shell_ref());
        }
        PromptStyle::Positional => {
            parts.push(prompt.shell_ref());
        }
    }
    if !engine.trailing.is_empty() {
        parts.push(engine.trailing);
    }
    Ok(parts.join(" "))
}

/// Union of conversation/session JSON pointers across registered engines.
///
/// Stream parsers stay engine-agnostic (one log line may arrive before the
/// supervisor knows which shape applies); OpenCode and friends add keys on
/// their registry row and they appear here automatically.
pub fn conversation_id_pointers() -> Vec<&'static str> {
    let mut out = Vec::new();
    for engine in ENGINES {
        for key in engine.conversation_id_pointers {
            if !out.contains(key) {
                out.push(*key);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_engines_are_registered() {
        for id in ["cursor", "agy", "claude", "opencode"] {
            assert_eq!(lookup(id).unwrap().id, id);
        }
    }

    #[test]
    fn unknown_engine_is_rejected() {
        let err = lookup("nope").unwrap_err();
        assert_eq!(err.id, "nope");
        let msg = err.to_string();
        assert!(msg.contains("unknown agent engine"), "{msg}");
        assert!(msg.contains("cursor"), "{msg}");
        assert!(msg.contains("agy"), "{msg}");
        assert!(msg.contains("claude"), "{msg}");
        assert!(msg.contains("opencode"), "{msg}");
        assert!(command_line("nope", PromptEnv::Briefing, None).is_err());
        assert!(!supports_resume("nope"));
    }

    #[test]
    fn claude_argv_fresh_no_resume() {
        let cmd = command_line("claude", PromptEnv::Briefing, None).unwrap();
        assert_eq!(
            cmd,
            "claude --bare --strict-mcp-config --mcp-config /sandbox/.honr/mcp/claude_mcp.json -p \"$HONR_BRIEFING\" --output-format stream-json --verbose --permission-mode bypassPermissions"
        );
        // Claude has no resume flag — conversation id must not change argv.
        let ignored = command_line("claude", PromptEnv::Briefing, Some("cid")).unwrap();
        assert_eq!(ignored, cmd);
        assert!(!supports_resume("claude"));
    }

    #[test]
    fn anthropic_inference_env_splits_opencode_v1_from_claude() {
        let oc = anthropic_inference_env("opencode");
        assert!(oc.iter().any(|(k, v)| {
            k == "ANTHROPIC_BASE_URL" && v == "https://inference.local/v1"
        }));
        let cl = anthropic_inference_env("claude");
        assert!(cl
            .iter()
            .any(|(k, v)| k == "ANTHROPIC_BASE_URL" && v == "https://inference.local"));
        assert!(anthropic_inference_env("cursor").is_empty());
        assert!(anthropic_inference_exports("opencode").contains("/v1"));
        assert!(!anthropic_inference_exports("claude").contains("/v1"));
        assert!(anthropic_inference_exports("cursor").is_empty());
    }

    #[test]
    fn agy_argv_fresh_and_resume() {
        let fresh = command_line("agy", PromptEnv::Briefing, None).unwrap();
        assert_eq!(
            fresh,
            "agy --dangerously-skip-permissions --print-timeout 24h --output-format stream-json -p \"$HONR_BRIEFING\""
        );
        assert!(!fresh.contains("--conversation"));

        let resume = command_line("agy", PromptEnv::Briefing, Some("cid")).unwrap();
        assert!(resume.contains("--conversation \"$HONR_CONVERSATION\""), "{resume}");
        assert!(resume.contains("-p \"$HONR_BRIEFING\""), "{resume}");
        assert!(supports_resume("agy"));
        assert_eq!(pre_start_auth("agy").unwrap(), PreStartAuth::Agy);
    }

    #[test]
    fn cursor_argv_fresh_and_resume() {
        let fresh = command_line("cursor", PromptEnv::Briefing, None).unwrap();
        assert_eq!(
            fresh,
            "agent -p --force --trust --sandbox disabled --output-format stream-json \"$HONR_BRIEFING\""
        );
        assert!(!fresh.contains("--resume"));

        let resume = command_line("cursor", PromptEnv::Prompt, Some("sid")).unwrap();
        assert!(
            resume.contains("--resume \"$HONR_CONVERSATION\""),
            "{resume}"
        );
        assert!(resume.ends_with("\"$HONR_PROMPT\""), "{resume}");
        assert!(supports_resume("cursor"));
        assert_eq!(pre_start_auth("cursor").unwrap(), PreStartAuth::None);
    }

    #[test]
    fn conversation_id_pointers_cover_agy_and_cursor() {
        let keys = conversation_id_pointers();
        assert!(keys.contains(&"/conversation_id"));
        assert!(keys.contains(&"/session_id"));
        assert!(keys.contains(&"/step_update/conversation_id"));
        assert!(keys.contains(&"/sessionID"));
    }

    #[test]
    fn opencode_argv_fresh_and_resume() {
        let fresh = command_line("opencode", PromptEnv::Briefing, None).unwrap();
        assert_eq!(
            fresh,
            "opencode run --format json --auto \"$HONR_BRIEFING\""
        );
        assert!(!fresh.contains("--session"));

        let resume = command_line("opencode", PromptEnv::Prompt, Some("ses_abc")).unwrap();
        assert!(
            resume.contains("--session \"$HONR_CONVERSATION\""),
            "{resume}"
        );
        assert!(resume.ends_with("\"$HONR_PROMPT\""), "{resume}");
        assert!(supports_resume("opencode"));
        assert_eq!(pre_start_auth("opencode").unwrap(), PreStartAuth::None);
    }
}
