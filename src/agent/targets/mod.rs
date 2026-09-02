//! Agent installer adapters, following CodeGraph's MIT-licensed target/registry design.

use super::{
    AgentKind,
    install::{self, AgentLocation, TargetPaths},
    tool_protocol::ConsentAdvice,
};
use crate::error::Result;
use serde::Serialize;
use serde_json::Value;
use std::path::{Path, PathBuf};

mod antigravity;
mod claude;
mod codex;
mod copilot_cli;
mod copilot_jetbrains;
mod copilot_vscode;
mod cursor;
mod gemini;
mod hermes;
mod kiro;
mod opencode;
mod pi;

#[derive(Debug)]
pub(super) struct DetectionResult {
    pub installed: bool,
    pub already_configured: bool,
    pub config_path: Option<PathBuf>,
}

pub(super) struct TargetEnvironment<'a> {
    pub location: AgentLocation,
    pub home: &'a Path,
    pub cwd: &'a Path,
}

#[derive(Clone, Copy)]
pub(super) struct InstallOptions {
    pub prompt_hook: bool,
}

pub(super) struct WriteResult {
    pub status: &'static str,
    pub files: Vec<PathBuf>,
    pub notes: Vec<String>,
    pub installed_hook_events: Vec<String>,
}

impl WriteResult {
    pub fn not_installed() -> Self {
        Self {
            status: "not_installed",
            files: Vec::new(),
            notes: Vec::new(),
            installed_hook_events: Vec::new(),
        }
    }

    pub fn unsupported(note: impl Into<String>) -> Self {
        Self {
            status: "unsupported",
            files: Vec::new(),
            notes: vec![note.into()],
            installed_hook_events: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize)]
pub(super) struct HookCapability {
    pub event: &'static str,
    pub semantic_event: &'static str,
    pub effects: &'static [&'static str],
    pub stability: &'static str,
    pub loop_guard: &'static str,
    pub tool_consent_mode: &'static str,
    pub tool_consent_failure: &'static str,
    pub tool_consent_coverage: &'static str,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub(super) struct IdentityCapability {
    pub quality: &'static str,
    pub session_fields: &'static [&'static str],
    pub child_fields: &'static [&'static str],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PromptForm {
    RawSubmitted,
    EffectiveAfterExpansion,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct PromptInput<'a> {
    pub text: &'a str,
    pub form: PromptForm,
}

const CONTEXT: &[&str] = &["context"];
const GUARD: &[&str] = &["guard"];
const OBSERVE: &[&str] = &["observe"];
const CONTEXT_GUARD: &[&str] = &["context", "guard"];
const GUARD_OBSERVE: &[&str] = &["guard", "observe"];
const STOP_CONTINUE: &[&str] = &["guard", "continue"];

const fn hook(
    event: &'static str,
    semantic_event: &'static str,
    effects: &'static [&'static str],
    stability: &'static str,
    loop_guard: &'static str,
) -> HookCapability {
    HookCapability {
        event,
        semantic_event,
        effects,
        stability,
        loop_guard,
        tool_consent_mode: "none",
        tool_consent_failure: "not_applicable",
        tool_consent_coverage: "none",
    }
}

const fn consent_hook(
    event: &'static str,
    semantic_event: &'static str,
    effects: &'static [&'static str],
    stability: &'static str,
    mode: &'static str,
) -> HookCapability {
    HookCapability {
        event,
        semantic_event,
        effects,
        stability,
        loop_guard: "none",
        tool_consent_mode: mode,
        tool_consent_failure: "fail_open",
        tool_consent_coverage: "recognized_lwc_shell",
    }
}

static CLAUDE_HOOKS: &[HookCapability] = &[
    hook("SessionStart", "session_start", CONTEXT, "stable", "none"),
    hook(
        "UserPromptSubmit",
        "prompt",
        CONTEXT_GUARD,
        "stable",
        "none",
    ),
    hook("PreCompact", "compact_before", GUARD, "stable", "none"),
    hook("PostCompact", "compact_after", OBSERVE, "stable", "none"),
    hook("SubagentStart", "subagent_start", CONTEXT, "stable", "none"),
    consent_hook(
        "PreToolUse",
        "tool_before",
        CONTEXT_GUARD,
        "stable",
        "enforced_ask",
    ),
    hook("PostToolUse", "tool_after", CONTEXT, "stable", "none"),
    hook(
        "PostToolUseFailure",
        "tool_failure",
        CONTEXT,
        "stable",
        "none",
    ),
    hook(
        "SubagentStop",
        "subagent_stop",
        STOP_CONTINUE,
        "stable",
        "stop_hook_active",
    ),
    hook("Stop", "stop", STOP_CONTINUE, "stable", "stop_hook_active"),
    hook("SessionEnd", "session_end", OBSERVE, "stable", "none"),
];

static CODEX_HOOKS: &[HookCapability] = &[
    hook("SessionStart", "session_start", CONTEXT, "stable", "none"),
    hook(
        "UserPromptSubmit",
        "prompt",
        CONTEXT_GUARD,
        "stable",
        "none",
    ),
    hook("PreCompact", "compact_before", GUARD, "stable", "none"),
    hook("PostCompact", "compact_after", OBSERVE, "stable", "none"),
    hook("SubagentStart", "subagent_start", CONTEXT, "stable", "none"),
    consent_hook("PreToolUse", "tool_before", CONTEXT, "stable", "advisory"),
    hook("PostToolUse", "tool_after", CONTEXT, "stable", "none"),
    hook(
        "SubagentStop",
        "subagent_stop",
        STOP_CONTINUE,
        "stable",
        "stop_hook_active",
    ),
    hook("Stop", "stop", STOP_CONTINUE, "stable", "stop_hook_active"),
    hook("SessionEnd", "session_end", OBSERVE, "stable", "none"),
];

static CURSOR_HOOKS: &[HookCapability] = &[
    hook("sessionStart", "session_start", CONTEXT, "stable", "none"),
    hook("beforeSubmitPrompt", "prompt", GUARD, "stable", "none"),
    hook("preCompact", "compact_before", OBSERVE, "stable", "none"),
    hook("subagentStart", "subagent_start", GUARD, "stable", "none"),
    consent_hook(
        "beforeShellExecution",
        "tool_before",
        GUARD,
        "stable",
        "enforced_ask",
    ),
    hook("postToolUse", "tool_after", CONTEXT, "stable", "none"),
    hook(
        "postToolUseFailure",
        "tool_failure",
        OBSERVE,
        "stable",
        "none",
    ),
    hook("subagentStop", "subagent_stop", OBSERVE, "stable", "none"),
    hook("stop", "stop", OBSERVE, "stable", "none"),
    hook("sessionEnd", "session_end", OBSERVE, "stable", "none"),
];

static OPENCODE_HOOKS: &[HookCapability] =
    &[hook("context", "session_start", CONTEXT, "beta", "none")];

static HERMES_HOOKS: &[HookCapability] = &[
    hook(
        "on_session_start",
        "session_start",
        OBSERVE,
        "stable",
        "none",
    ),
    hook("pre_llm_call", "prompt", CONTEXT, "stable", "none"),
    consent_hook(
        "pre_tool_call",
        "tool_before",
        GUARD,
        "stable",
        "enforced_ask",
    ),
    hook("post_tool_call", "tool_after", OBSERVE, "stable", "none"),
    hook(
        "subagent_start",
        "subagent_start",
        OBSERVE,
        "stable",
        "none",
    ),
    hook("subagent_stop", "subagent_stop", OBSERVE, "stable", "none"),
    hook("post_llm_call", "stop", OBSERVE, "stable", "none"),
    hook("on_session_end", "session_end", OBSERVE, "stable", "none"),
];

static GEMINI_HOOKS: &[HookCapability] = &[
    hook("SessionStart", "session_start", CONTEXT, "stable", "none"),
    hook("BeforeAgent", "turn_start", CONTEXT_GUARD, "stable", "none"),
    hook("BeforeTool", "tool_before", GUARD, "stable", "none"),
    hook("AfterTool", "tool_after", CONTEXT, "stable", "none"),
    hook("PreCompress", "compact_before", OBSERVE, "stable", "none"),
    hook("AfterAgent", "stop", OBSERVE, "stable", "none"),
    hook("SessionEnd", "session_end", OBSERVE, "stable", "none"),
];

static ANTIGRAVITY_HOOKS: &[HookCapability] = &[
    hook("PreInvocation", "turn_start", OBSERVE, "stable", "none"),
    consent_hook("PreToolUse", "tool_before", GUARD, "stable", "enforced_ask"),
    hook("PostToolUse", "tool_after", OBSERVE, "stable", "none"),
    hook("PostInvocation", "stop", OBSERVE, "stable", "none"),
    hook("Stop", "stop", OBSERVE, "stable", "none"),
];

static KIRO_HOOKS: &[HookCapability] = &[
    hook("SessionStart", "session_start", CONTEXT, "preview", "none"),
    hook(
        "UserPromptSubmit",
        "prompt",
        CONTEXT_GUARD,
        "preview",
        "none",
    ),
    hook(
        "PreToolUse",
        "tool_before",
        CONTEXT_GUARD,
        "preview",
        "none",
    ),
    hook("PostToolUse", "tool_after", CONTEXT, "preview", "none"),
    hook("Stop", "stop", OBSERVE, "preview", "none"),
];

static COPILOT_VSCODE_HOOKS: &[HookCapability] = &[
    hook("SessionStart", "session_start", CONTEXT, "preview", "none"),
    hook("UserPromptSubmit", "prompt", GUARD, "preview", "none"),
    hook("PreCompact", "compact_before", OBSERVE, "preview", "none"),
    hook(
        "SubagentStart",
        "subagent_start",
        CONTEXT,
        "preview",
        "none",
    ),
    hook(
        "PreToolUse",
        "tool_before",
        CONTEXT_GUARD,
        "preview",
        "none",
    ),
    hook(
        "PostToolUse",
        "tool_after",
        CONTEXT_GUARD,
        "preview",
        "none",
    ),
    hook("SubagentStop", "subagent_stop", OBSERVE, "preview", "none"),
    hook("Stop", "stop", OBSERVE, "preview", "none"),
];

static COPILOT_CLI_HOOKS: &[HookCapability] = &[
    hook("sessionStart", "session_start", CONTEXT, "stable", "none"),
    hook("userPromptSubmitted", "prompt", OBSERVE, "stable", "none"),
    hook("userPromptTransformed", "prompt", OBSERVE, "stable", "none"),
    hook("preToolUse", "tool_before", GUARD, "stable", "none"),
    hook("postToolUse", "tool_after", CONTEXT, "stable", "none"),
    hook("preCompact", "compact_before", OBSERVE, "stable", "none"),
    hook("subagentStart", "subagent_start", CONTEXT, "stable", "none"),
    hook("subagentStop", "subagent_stop", OBSERVE, "stable", "none"),
    hook("agentStop", "stop", OBSERVE, "stable", "none"),
    hook("sessionEnd", "session_end", OBSERVE, "stable", "none"),
];

static PI_HOOKS: &[HookCapability] = &[
    hook("session_start", "session_start", CONTEXT, "stable", "none"),
    hook(
        "session_before_compact",
        "compact_before",
        GUARD_OBSERVE,
        "stable",
        "none",
    ),
    hook(
        "before_agent_start",
        "turn_start",
        CONTEXT,
        "stable",
        "none",
    ),
    hook(
        "session_compact",
        "compact_after",
        CONTEXT,
        "stable",
        "none",
    ),
    hook("tool_call", "tool_before", GUARD, "stable", "none"),
    hook("tool_result", "tool_after", CONTEXT, "stable", "none"),
    hook("agent_settled", "stop", OBSERVE, "stable", "none"),
    hook("session_shutdown", "session_end", OBSERVE, "stable", "none"),
];

fn hook_capabilities(target: &str) -> &'static [HookCapability] {
    match target {
        "claude" => CLAUDE_HOOKS,
        "cursor" => CURSOR_HOOKS,
        "codex" => CODEX_HOOKS,
        "opencode" => OPENCODE_HOOKS,
        "hermes" => HERMES_HOOKS,
        "gemini" => GEMINI_HOOKS,
        "antigravity" => ANTIGRAVITY_HOOKS,
        "kiro" => KIRO_HOOKS,
        "copilot-vscode" => COPILOT_VSCODE_HOOKS,
        "copilot-cli" => COPILOT_CLI_HOOKS,
        "copilot-jetbrains" => &[],
        "pi" => PI_HOOKS,
        _ => &[],
    }
}

pub(super) fn hook_capability(agent: AgentKind, native_event: &str) -> Option<HookCapability> {
    let target = match agent {
        AgentKind::Claude => "claude",
        AgentKind::Codex => "codex",
        AgentKind::OpenCode => "opencode",
        AgentKind::Cursor => "cursor",
        AgentKind::Gemini => "gemini",
        AgentKind::Hermes => "hermes",
        AgentKind::Antigravity => "antigravity",
        AgentKind::CopilotCli => "copilot-cli",
        AgentKind::CopilotVscode => "copilot-vscode",
        AgentKind::Kiro => "kiro",
        AgentKind::Pi => "pi",
        AgentKind::Generic => return None,
    };
    let normalized = normalize_event(native_event);
    hook_capabilities(target)
        .iter()
        .copied()
        .find(|capability| normalize_event(capability.event) == normalized)
}

pub(super) fn exact_current_prompt<'a>(
    agent: AgentKind,
    native_event: &str,
    payload: &'a Value,
) -> Option<PromptInput<'a>> {
    let native_event = normalize_event(native_event);
    let (text, form) = match (agent, native_event.as_str()) {
        (AgentKind::Claude | AgentKind::Codex | AgentKind::CopilotVscode, "userpromptsubmit")
        | (AgentKind::Cursor, "beforesubmitprompt")
        | (AgentKind::Gemini, "beforeagent")
        | (AgentKind::CopilotCli, "userpromptsubmitted") => {
            (payload.get("prompt")?.as_str()?, PromptForm::RawSubmitted)
        }
        (AgentKind::Hermes, "prellmcall") => (
            payload.get("extra")?.get("user_message")?.as_str()?,
            PromptForm::RawSubmitted,
        ),
        (AgentKind::Pi, "beforeagentstart") => (
            payload.get("prompt")?.as_str()?,
            PromptForm::EffectiveAfterExpansion,
        ),
        _ => return None,
    };
    (!text.is_empty()).then_some(PromptInput { text, form })
}

pub(super) fn prompt_environment(agent: AgentKind, native_event: &str) -> Option<&'static str> {
    (matches!(agent, AgentKind::Kiro) && normalize_event(native_event) == "userpromptsubmit")
        .then_some("USER_PROMPT")
}

pub(super) fn tool_consent_output(
    agent: AgentKind,
    native_event: &str,
    advice: ConsentAdvice,
) -> Option<Value> {
    if advice.reason.is_empty()
        || !advice.code.starts_with("lwc_")
        || !advice
            .code
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    {
        return None;
    }
    let event = normalize_event(native_event);
    match (agent, event.as_str()) {
        (AgentKind::Claude, "pretooluse") => Some(serde_json::json!({
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "ask",
                "permissionDecisionReason": advice.reason,
            }
        })),
        (AgentKind::Codex, "pretooluse") => Some(serde_json::json!({
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "additionalContext": advice.reason,
            }
        })),
        (AgentKind::Cursor, "beforeshellexecution") => Some(serde_json::json!({
            "permission": "ask",
            "user_message": advice.reason,
            "agent_message": advice.reason,
        })),
        (AgentKind::Hermes, "pretoolcall") => Some(serde_json::json!({
            "action": "approve",
            "message": advice.reason,
            "rule_key": format!("lwc:{}", advice.code),
        })),
        (AgentKind::Antigravity, "pretooluse") => Some(serde_json::json!({
            "decision": "ask",
            "reason": advice.reason,
        })),
        _ => None,
    }
}

fn normalize_event(event: &str) -> String {
    event
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn configured_hook_events(
    target: &str,
    location: AgentLocation,
    options: InstallOptions,
) -> Vec<&'static str> {
    match target {
        "claude" => {
            let mut events = vec![
                "SessionStart",
                "PreToolUse",
                "PostToolUse",
                "PostToolUseFailure",
                "SubagentStart",
                "SubagentStop",
                "Stop",
            ];
            if options.prompt_hook {
                events.insert(1, "UserPromptSubmit");
            }
            events
        }
        "codex" => {
            let mut events = vec![
                "SessionStart",
                "PreToolUse",
                "PostToolUse",
                "SubagentStart",
                "SubagentStop",
                "Stop",
            ];
            if options.prompt_hook {
                events.insert(1, "UserPromptSubmit");
            }
            events
        }
        "cursor" => vec![
            "sessionStart",
            "preCompact",
            "postToolUse",
            "beforeShellExecution",
        ],
        "opencode" => vec!["context"],
        "hermes" if location == AgentLocation::Global => vec!["pre_llm_call", "pre_tool_call"],
        "hermes" => Vec::new(),
        "gemini" => vec!["SessionStart", "BeforeAgent", "AfterTool"],
        "antigravity" => vec!["PreToolUse"],
        "kiro" => vec!["SessionStart", "UserPromptSubmit", "PostToolUse"],
        "copilot-vscode" => vec![
            "SessionStart",
            "PostToolUse",
            "SubagentStart",
            "SubagentStop",
        ],
        "copilot-cli" => vec!["sessionStart", "postToolUse", "subagentStart"],
        "copilot-jetbrains" => Vec::new(),
        "pi" => vec![
            "session_start",
            "session_before_compact",
            "session_compact",
            "before_agent_start",
            "session_shutdown",
        ],
        _ => Vec::new(),
    }
}

fn identity_capability(target: &str) -> IdentityCapability {
    match target {
        "claude" | "codex" | "copilot-vscode" => IdentityCapability {
            quality: "exact_child",
            session_fields: &["session_id"],
            child_fields: &["agent_id"],
        },
        "hermes" => IdentityCapability {
            quality: "exact_child",
            session_fields: &["session_id"],
            child_fields: &["child_session_id", "child_subagent_id"],
        },
        "cursor" => IdentityCapability {
            quality: "root_only",
            session_fields: &["conversation_id"],
            child_fields: &[],
        },
        "gemini" | "kiro" => IdentityCapability {
            quality: "root_only",
            session_fields: &["session_id"],
            child_fields: &[],
        },
        "antigravity" => IdentityCapability {
            quality: "root_only",
            session_fields: &["conversationId"],
            child_fields: &[],
        },
        "opencode" => IdentityCapability {
            quality: "root_only",
            session_fields: &["sessionID"],
            child_fields: &[],
        },
        "pi" => IdentityCapability {
            quality: "root_only",
            session_fields: &["sessionManager.getSessionId()"],
            child_fields: &[],
        },
        "copilot-cli" => IdentityCapability {
            quality: "root_only",
            session_fields: &["sessionId"],
            child_fields: &[],
        },
        _ => IdentityCapability {
            quality: "unavailable",
            session_fields: &[],
            child_fields: &[],
        },
    }
}

pub(super) trait AgentTarget: Sync {
    fn id(&self) -> &'static str;
    fn display_name(&self) -> &'static str;
    fn adaptation(&self) -> &'static str {
        "strong"
    }
    fn mcp_mode(&self, location: AgentLocation) -> &'static str;
    fn permissions_mode(&self, location: AgentLocation) -> &'static str;
    fn instructions_mode(&self, location: AgentLocation) -> &'static str;
    fn skills_mode(&self, location: AgentLocation) -> &'static str;
    fn lifecycle_mode(&self, location: AgentLocation) -> &'static str;
    fn lifecycle_hook(&self, location: AgentLocation) -> bool {
        matches!(
            self.lifecycle_mode(location),
            "installed" | "configured_preview"
        )
    }
    fn hook_capabilities(&self, _location: AgentLocation) -> &'static [HookCapability] {
        hook_capabilities(self.id())
    }
    fn identity_capability(&self) -> IdentityCapability {
        identity_capability(self.id())
    }
    fn configured_hook_events(
        &self,
        location: AgentLocation,
        options: InstallOptions,
    ) -> Vec<&'static str> {
        configured_hook_events(self.id(), location, options)
    }
    fn supports_location(&self, location: AgentLocation) -> bool;
    fn detect(&self, environment: &TargetEnvironment<'_>) -> DetectionResult;
    fn install(
        &self,
        environment: &TargetEnvironment<'_>,
        options: InstallOptions,
    ) -> Result<WriteResult>;
    fn uninstall(&self, environment: &TargetEnvironment<'_>) -> Result<WriteResult>;
    fn configure(&self, paths: &TargetPaths, options: InstallOptions) -> Result<()>;
    fn unconfigure(&self, paths: &TargetPaths, executable: &str) -> Result<()>;
    fn print_config(&self, location: AgentLocation) -> String;
    fn describe_paths(&self, environment: &TargetEnvironment<'_>) -> Vec<PathBuf>;
}

pub(super) static ALL_TARGETS: [&dyn AgentTarget; 12] = [
    &claude::CLAUDE,
    &cursor::CURSOR,
    &codex::CODEX,
    &opencode::OPENCODE,
    &hermes::HERMES,
    &gemini::GEMINI,
    &antigravity::ANTIGRAVITY,
    &kiro::KIRO,
    &copilot_vscode::COPILOT_VSCODE,
    &copilot_cli::COPILOT_CLI,
    &copilot_jetbrains::COPILOT_JETBRAINS,
    &pi::PI,
];

pub(super) fn get_target(id: &str) -> Option<&'static dyn AgentTarget> {
    ALL_TARGETS.iter().copied().find(|target| target.id() == id)
}

pub(super) fn target_ids() -> Vec<&'static str> {
    ALL_TARGETS.iter().map(|target| target.id()).collect()
}

pub(super) fn native_install(
    target: &dyn AgentTarget,
    environment: &TargetEnvironment<'_>,
    options: InstallOptions,
    paths: TargetPaths,
) -> Result<WriteResult> {
    install::install_native(target, environment, options, paths)
}

pub(super) fn native_uninstall(
    target: &dyn AgentTarget,
    environment: &TargetEnvironment<'_>,
    paths: TargetPaths,
) -> Result<WriteResult> {
    install::uninstall_native(target, environment, paths)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn registry_ids_are_unique_and_stable() {
        let ids = target_ids();
        assert_eq!(
            ids,
            [
                "claude",
                "cursor",
                "codex",
                "opencode",
                "hermes",
                "gemini",
                "antigravity",
                "kiro",
                "copilot-vscode",
                "copilot-cli",
                "copilot-jetbrains",
                "pi"
            ]
        );
        let mut unique = ids.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), ids.len());
    }

    #[test]
    fn exact_current_prompt_accepts_only_verified_native_fields() {
        let top_level = json!({
            "prompt": "current 🧠 prompt",
            "messages": ["private history"],
            "transcript_path": "/private/transcript",
            "transformedPrompt": "historical transformed prompt"
        });
        for (agent, event) in [
            (AgentKind::Claude, "UserPromptSubmit"),
            (AgentKind::Codex, "user-prompt-submit"),
            (AgentKind::Cursor, "beforeSubmitPrompt"),
            (AgentKind::Gemini, "BeforeAgent"),
            (AgentKind::CopilotVscode, "UserPromptSubmit"),
            (AgentKind::CopilotCli, "userPromptSubmitted"),
        ] {
            assert_eq!(
                exact_current_prompt(agent, event, &top_level),
                Some(PromptInput {
                    text: "current 🧠 prompt",
                    form: PromptForm::RawSubmitted,
                }),
                "{agent:?} {event}"
            );
        }

        let hermes = json!({
            "extra": {
                "user_message": "hermes current",
                "conversation_history": ["private history"]
            },
            "user_message": "wrong level"
        });
        assert_eq!(
            exact_current_prompt(AgentKind::Hermes, "pre_llm_call", &hermes),
            Some(PromptInput {
                text: "hermes current",
                form: PromptForm::RawSubmitted,
            })
        );

        let pi = json!({"prompt": "expanded current", "messages": ["private history"]});
        assert_eq!(
            exact_current_prompt(AgentKind::Pi, "before_agent_start", &pi),
            Some(PromptInput {
                text: "expanded current",
                form: PromptForm::EffectiveAfterExpansion,
            })
        );

        for (agent, event, payload) in [
            (
                AgentKind::OpenCode,
                "context",
                json!({"messages": [{"parts": [{"text": "history"}]}]}),
            ),
            (
                AgentKind::Antigravity,
                "PreInvocation",
                json!({"transcriptPath": "/private/transcript"}),
            ),
            (
                AgentKind::CopilotCli,
                "userPromptTransformed",
                json!({"prompt": "preceding message", "transformedPrompt": "history"}),
            ),
            (
                AgentKind::Hermes,
                "pre_llm_call",
                json!({"conversation_history": ["history"]}),
            ),
        ] {
            assert_eq!(exact_current_prompt(agent, event, &payload), None);
        }
        assert_eq!(
            exact_current_prompt(
                AgentKind::Claude,
                "UserPromptSubmit",
                &json!({"prompt": ["not", "a", "string"]}),
            ),
            None
        );
        assert_eq!(
            exact_current_prompt(
                AgentKind::Claude,
                "PostToolUse",
                &json!({"prompt": "wrong event"}),
            ),
            None
        );
    }

    #[test]
    fn only_kiro_prompt_submit_uses_a_verified_environment_transport() {
        assert_eq!(
            prompt_environment(AgentKind::Kiro, "UserPromptSubmit"),
            Some("USER_PROMPT")
        );
        assert_eq!(prompt_environment(AgentKind::Kiro, "PreToolUse"), None);
        assert_eq!(
            prompt_environment(AgentKind::Antigravity, "PreInvocation"),
            None
        );
    }

    #[test]
    fn tool_consent_output_uses_only_verified_ask_or_advisory_shapes() {
        let advice = crate::agent::tool_protocol::ConsentAdvice {
            boundary: crate::agent::tool_protocol::ConsentBoundary::ConfigurationChange,
            code: "lwc_config_set",
            reason: "Ask the user before changing LWC configuration.",
        };
        let cases = [
            (
                AgentKind::Claude,
                "PreToolUse",
                json!({
                    "hookSpecificOutput": {
                        "hookEventName": "PreToolUse",
                        "permissionDecision": "ask",
                        "permissionDecisionReason": advice.reason,
                    }
                }),
            ),
            (
                AgentKind::Codex,
                "PreToolUse",
                json!({
                    "hookSpecificOutput": {
                        "hookEventName": "PreToolUse",
                        "additionalContext": advice.reason,
                    }
                }),
            ),
            (
                AgentKind::Cursor,
                "beforeShellExecution",
                json!({
                    "permission": "ask",
                    "user_message": advice.reason,
                    "agent_message": advice.reason,
                }),
            ),
            (
                AgentKind::Hermes,
                "pre_tool_call",
                json!({
                    "action": "approve",
                    "message": advice.reason,
                    "rule_key": "lwc:lwc_config_set",
                }),
            ),
            (
                AgentKind::Antigravity,
                "PreToolUse",
                json!({"decision": "ask", "reason": advice.reason}),
            ),
        ];
        for (agent, event, expected) in cases {
            let output = tool_consent_output(agent, event, advice)
                .unwrap_or_else(|| panic!("{agent:?} {event} must compile consent output"));
            assert_eq!(output, expected, "{agent:?} {event}");
            let text = output.to_string();
            for forbidden in [
                "permissionOverrides",
                "updatedInput",
                "force_ask",
                "failClosed",
                "/private/",
            ] {
                assert!(!text.contains(forbidden), "{agent:?} leaked {forbidden}");
            }
        }

        for (agent, event) in [
            (AgentKind::Claude, "PostToolUse"),
            (AgentKind::Codex, "Stop"),
            (AgentKind::Cursor, "preToolUse"),
            (AgentKind::Hermes, "post_tool_call"),
            (AgentKind::Antigravity, "PostToolUse"),
            (AgentKind::Gemini, "BeforeTool"),
            (AgentKind::Kiro, "PreToolUse"),
            (AgentKind::Pi, "tool_call"),
            (AgentKind::CopilotCli, "preToolUse"),
            (AgentKind::CopilotVscode, "PreToolUse"),
            (AgentKind::OpenCode, "context"),
            (AgentKind::Generic, "PreToolUse"),
        ] {
            assert_eq!(tool_consent_output(agent, event, advice), None);
        }
    }
}
