use super::{AgentKind, signals::EventKind};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

pub(super) enum AgentExecutionContext {
    Resolved(String),
    Unresolved(&'static str),
}

impl AgentExecutionContext {
    pub(super) fn resolve(agent: AgentKind, event: EventKind, payload: &Value) -> Self {
        let target = target_id(agent);
        let child = match event {
            EventKind::SubagentStart | EventKind::SubagentStop => true,
            EventKind::Prompt | EventKind::TurnStart
                if matches!(
                    agent,
                    AgentKind::Codex
                        | AgentKind::Claude
                        | AgentKind::Hermes
                        | AgentKind::CopilotVscode
                ) =>
            {
                let has_child_identity = match agent {
                    AgentKind::Hermes => {
                        native_id(payload, "child_session_id").is_some()
                            && native_id(payload, "child_subagent_id").is_some()
                    }
                    _ => native_id(payload, "agent_id").is_some(),
                };
                if !has_child_identity {
                    return Self::Unresolved("ambiguous_subject_identity");
                }
                true
            }
            _ => false,
        };
        let session_field = match agent {
            AgentKind::Codex
            | AgentKind::Claude
            | AgentKind::Gemini
            | AgentKind::Hermes
            | AgentKind::Kiro
            | AgentKind::Pi => "session_id",
            AgentKind::Cursor => "conversation_id",
            AgentKind::Antigravity => "conversationId",
            AgentKind::CopilotCli => "sessionId",
            AgentKind::CopilotVscode => "session_id",
            AgentKind::OpenCode => "sessionID",
            AgentKind::Generic => return Self::Unresolved("unsupported_target_identity"),
        };
        let Some(mut session) = native_id(payload, session_field) else {
            return Self::Unresolved("missing_session_id");
        };
        let (subject, actor) = if child {
            match agent {
                AgentKind::Codex | AgentKind::Claude | AgentKind::CopilotVscode => {
                    let Some(actor) = native_id(payload, "agent_id") else {
                        return Self::Unresolved("missing_child_id");
                    };
                    ("subagent", actor)
                }
                AgentKind::Hermes => {
                    let Some(child_session) = native_id(payload, "child_session_id") else {
                        return Self::Unresolved("missing_child_session_id");
                    };
                    let Some(actor) = native_id(payload, "child_subagent_id") else {
                        return Self::Unresolved("missing_child_id");
                    };
                    session = child_session;
                    ("subagent", actor)
                }
                _ => return Self::Unresolved("unsupported_child_identity"),
            }
        } else {
            ("main", "main")
        };
        let canonical = format!(
            "lwc-agent-context/v1\0{target}\0{session}\0{subject}\0{actor}"
        );
        let digest = Sha256::digest(canonical.as_bytes())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        Self::Resolved(format!("lwcctx-v1-{digest}"))
    }

    pub(super) fn id(&self) -> Option<&str> {
        match self {
            Self::Resolved(id) => Some(id),
            Self::Unresolved(_) => None,
        }
    }

    pub(super) fn readiness(&self, bound: Option<bool>) -> Value {
        match self {
            Self::Resolved(id) => json!({
                "status": if bound == Some(true) { "bound" } else if bound == Some(false) { "unbound" } else { "unavailable" },
                "context_id": id,
            }),
            Self::Unresolved(reason) => json!({"status":"unresolved","reason":reason}),
        }
    }
}

fn native_id<'a>(payload: &'a Value, field: &str) -> Option<&'a str> {
    let value = payload.get(field)?.as_str()?;
    (!value.is_empty()
        && value.len() <= 512
        && !value.chars().any(char::is_control))
    .then_some(value)
}

fn target_id(agent: AgentKind) -> &'static str {
    match agent {
        AgentKind::Codex => "codex",
        AgentKind::Claude => "claude",
        AgentKind::Cursor => "cursor",
        AgentKind::Gemini => "gemini",
        AgentKind::Hermes => "hermes",
        AgentKind::Antigravity => "antigravity",
        AgentKind::CopilotCli => "copilot-cli",
        AgentKind::CopilotVscode => "copilot-vscode",
        AgentKind::Kiro => "kiro",
        AgentKind::OpenCode => "opencode",
        AgentKind::Pi => "pi",
        AgentKind::Generic => "generic",
    }
}
