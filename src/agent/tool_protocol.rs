use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::path::Path;

const MAX_COMMAND_BYTES: usize = 16 * 1024;
const MAX_ARGV_ITEMS: usize = 128;
const MAX_ARG_BYTES: usize = 4 * 1024;
const MAX_STDOUT_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ToolHost {
    Claude,
    Codex,
    Cursor,
    OpenCode,
    Hermes,
    Gemini,
    Antigravity,
    Kiro,
    Pi,
    CopilotCli,
    CopilotVscode,
    Generic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InvocationTransport {
    Mcp,
    Argv,
    Shell,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConsentClass {
    Noop,
    Ask,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConsentBoundary {
    StateInitialization,
    ConfigurationChange,
    CodeGraphInitialization,
    SyncStart,
    AgentIntegrationChange,
    ChangesetFinalization,
    CheckpointRestore,
    DurableContentRemoval,
    SchemaChange,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ConsentAdvice {
    pub(crate) boundary: ConsentBoundary,
    pub(crate) code: &'static str,
    pub(crate) reason: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RecognizedInvocation {
    pub(crate) action: String,
    pub(crate) transport: InvocationTransport,
    pub(crate) consent: ConsentClass,
    pub(crate) consent_advice: Option<ConsentAdvice>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReceiptProgress {
    pub(crate) completed: u64,
    pub(crate) total: Option<u64>,
    pub(crate) sequence: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Receipt {
    pub(crate) action: Option<String>,
    pub(crate) identifiers: BTreeMap<String, String>,
    pub(crate) revisions: BTreeMap<String, String>,
    pub(crate) state: Option<String>,
    pub(crate) status: Option<String>,
    pub(crate) phase: Option<String>,
    pub(crate) counts: BTreeMap<String, u64>,
    pub(crate) progress: Option<ReceiptProgress>,
    pub(crate) next_action: Option<String>,
    pub(crate) error_code: Option<String>,
    pub(crate) recovery_action: Option<String>,
}

struct ToolEnvelope<'a> {
    name: &'a str,
    server: Option<&'a str>,
    input: ToolInput<'a>,
    result: Option<&'a Value>,
}

enum ToolInput<'a> {
    Borrowed(&'a Value),
    Owned(Value),
}

struct ReceiptObjects<'a> {
    general: Vec<(&'static str, &'a Map<String, Value>)>,
    pressure: Option<&'a Map<String, Value>>,
    retained: Option<&'a Map<String, Value>>,
}

impl ToolInput<'_> {
    fn as_value(&self) -> &Value {
        match self {
            Self::Borrowed(value) => value,
            Self::Owned(value) => value,
        }
    }
}

pub(crate) fn recognize_invocation(
    host: ToolHost,
    payload: &Value,
    installer_owned_lwc: Option<&str>,
) -> Option<RecognizedInvocation> {
    let envelope = tool_envelope(host, payload)?;
    let mcp_tool = if envelope.name == "mcp__lwc__lwc_explore"
        || envelope.name == "lwc.lwc_explore"
        || (envelope.name == "lwc_explore" && envelope.server == Some("lwc"))
    {
        Some("lwc_explore")
    } else if envelope.name == "mcp__lwc__lwc_codegraph"
        || envelope.name == "lwc.lwc_codegraph"
        || (envelope.name == "lwc_codegraph" && envelope.server == Some("lwc"))
    {
        Some("lwc_codegraph")
    } else {
        None
    };
    if let Some(tool) = mcp_tool {
        return Some(RecognizedInvocation {
            action: format!("mcp.{tool}"),
            transport: InvocationTransport::Mcp,
            consent: ConsentClass::Noop,
            consent_advice: None,
        });
    }
    let (argv, transport) = command_argv(host, envelope.name, envelope.input.as_value())?;
    if !is_lwc_executable(argv.first()?.as_str(), installer_owned_lwc) {
        return None;
    }
    let action = invocation_action(&argv)?;
    let consent_advice = consent_advice_for(&action);
    Some(RecognizedInvocation {
        consent: if consent_advice.is_some() {
            ConsentClass::Ask
        } else {
            ConsentClass::Noop
        },
        consent_advice,
        action,
        transport,
    })
}

pub(crate) fn parse_receipt(
    host: ToolHost,
    payload: &Value,
    invocation: &RecognizedInvocation,
) -> Option<Receipt> {
    if invocation.transport == InvocationTransport::Mcp {
        return None;
    }
    let result = tool_envelope(host, payload)?.result?;
    if result_reports_failure(result) {
        return None;
    }
    let stdout = match result {
        Value::String(stdout) => stdout.as_str(),
        Value::Object(object) => object.get("stdout")?.as_str()?,
        _ => return None,
    };
    if stdout.len() > MAX_STDOUT_BYTES {
        return None;
    }
    let value = serde_json::from_str::<Value>(stdout).ok()?;
    let root = value.as_object()?;
    let objects = receipt_objects(root);
    let preferred_work = objects.general.iter().find_map(|(container, object)| {
        (*container == "work" && has_work_identifier(object)).then_some(*object)
    });

    let action = root
        .get("action")
        .and_then(Value::as_str)
        .filter(|value| safe_action_code(value))
        .map(str::to_owned)
        .or_else(|| Some(invocation.action.clone()));
    let mut identifiers = BTreeMap::new();
    extract_identifiers(root, None, &mut identifiers);
    for (container, object) in &objects.general {
        extract_identifiers(object, Some(container), &mut identifiers);
    }
    let mut revisions = BTreeMap::new();
    extract_revisions(root, &mut revisions);
    for (_, object) in &objects.general {
        extract_revisions(object, &mut revisions);
    }
    let state = preferred_work
        .and_then(|object| code_field(object, "state"))
        .or_else(|| first_code(root, &objects.general, "state"));
    let status = first_code(root, &objects.general, "status");
    let phase = preferred_work
        .and_then(|object| code_field(object, "phase"))
        .or_else(|| first_code(root, &objects.general, "phase"));
    let mut counts = BTreeMap::new();
    extract_counts(root, &mut counts);
    for (_, object) in &objects.general {
        extract_counts(object, &mut counts);
    }
    extract_memory_root_counts(root, &mut counts);
    extract_retained_counts(objects.retained, &mut counts);
    extract_pressure_counts(objects.pressure, &mut counts);
    let progress = preferred_work.and_then(receipt_progress).or_else(|| {
        std::iter::once(root)
            .chain(objects.general.iter().map(|(_, object)| *object))
            .find_map(receipt_progress)
    });
    let next_action = first_next_action(root, &objects.general);
    let recovery_action = first_command_action(root, &objects.general, "recovery_command")
        .or_else(|| {
            root.get("recovery")
                .and_then(Value::as_object)
                .and_then(|recovery| recovery.get("command"))
                .and_then(Value::as_str)
                .and_then(normalized_shell_action)
        })
        .or_else(|| {
            root.get("error")
                .and_then(Value::as_object)
                .and_then(|error| error.get("details"))
                .and_then(Value::as_object)
                .and_then(|details| details.get("recovery_command"))
                .and_then(Value::as_str)
                .and_then(normalized_shell_action)
        });
    let error_code = std::iter::once(root)
        .chain(objects.general.iter().map(|(_, object)| *object))
        .find_map(extract_error_code);

    Some(Receipt {
        action,
        identifiers,
        revisions,
        state,
        status,
        phase,
        counts,
        progress,
        next_action,
        error_code,
        recovery_action,
    })
}

fn result_reports_failure(result: &Value) -> bool {
    let Some(object) = result.as_object() else {
        return false;
    };
    ["exit_code", "exitCode"]
        .into_iter()
        .filter_map(|key| object.get(key).and_then(Value::as_i64))
        .any(|exit_code| exit_code != 0)
        || object.get("success").and_then(Value::as_bool) == Some(false)
        || ["is_error", "isError"]
            .into_iter()
            .any(|key| object.get(key).and_then(Value::as_bool) == Some(true))
}

fn tool_envelope(host: ToolHost, payload: &Value) -> Option<ToolEnvelope<'_>> {
    let object = payload.as_object()?;
    if host == ToolHost::Cursor
        && object.get("hook_event_name").and_then(Value::as_str) == Some("beforeShellExecution")
    {
        let command = object.get("command")?.as_str()?;
        if command.len() > MAX_COMMAND_BYTES {
            return None;
        }
        return Some(ToolEnvelope {
            name: shell_tool_name(host),
            server: None,
            input: ToolInput::Borrowed(payload),
            result: None,
        });
    }
    if host == ToolHost::Antigravity {
        let tool_call = object.get("toolCall")?.as_object()?;
        return Some(ToolEnvelope {
            name: tool_call.get("name")?.as_str()?,
            server: None,
            input: ToolInput::Borrowed(tool_call.get("args")?),
            result: None,
        });
    }
    if host == ToolHost::CopilotCli {
        let raw_input = object.get("toolArgs")?;
        let input = match raw_input {
            Value::Object(_) => ToolInput::Borrowed(raw_input),
            Value::String(raw) if raw.len() <= MAX_COMMAND_BYTES => {
                let parsed = serde_json::from_str::<Value>(raw).ok()?;
                parsed.as_object()?;
                ToolInput::Owned(parsed)
            }
            _ => return None,
        };
        return Some(ToolEnvelope {
            name: object.get("toolName")?.as_str()?,
            server: object.get("mcpServerName").and_then(Value::as_str),
            input,
            result: None,
        });
    }
    let (name, server, input, result) = match host {
        ToolHost::Claude | ToolHost::Codex | ToolHost::Gemini => (
            "tool_name",
            "mcp_server_name",
            "tool_input",
            Some("tool_response"),
        ),
        ToolHost::Cursor => (
            "tool_name",
            "mcp_server_name",
            "tool_input",
            Some("tool_output"),
        ),
        ToolHost::Hermes => (
            "tool_name",
            "mcp_server_name",
            "tool_input",
            Some("tool_result"),
        ),
        ToolHost::Kiro | ToolHost::CopilotVscode => {
            ("tool_name", "mcp_server_name", "tool_input", None)
        }
        ToolHost::Pi => ("tool_name", "mcp_server_name", "args", Some("result")),
        ToolHost::OpenCode => ("tool", "server", "args", Some("output")),
        ToolHost::Antigravity | ToolHost::CopilotCli | ToolHost::Generic => return None,
    };
    Some(ToolEnvelope {
        name: object.get(name)?.as_str()?,
        server: object.get(server).and_then(Value::as_str),
        input: ToolInput::Borrowed(object.get(input)?),
        result: result.and_then(|key| object.get(key)),
    })
}

fn command_argv(
    host: ToolHost,
    tool_name: &str,
    input: &Value,
) -> Option<(Vec<String>, InvocationTransport)> {
    if tool_name != shell_tool_name(host) {
        return None;
    }
    if host == ToolHost::Kiro {
        return None;
    }
    let input = input.as_object()?;
    let command_key = match host {
        ToolHost::Antigravity => "CommandLine",
        _ => "command",
    };
    if let Some(argv) = input.get(command_key).and_then(Value::as_array) {
        return argv_from_values(argv).map(|argv| (argv, InvocationTransport::Argv));
    }
    let command = input.get(command_key)?.as_str()?;
    shell_argv(command).map(|argv| (argv, InvocationTransport::Shell))
}

fn shell_tool_name(host: ToolHost) -> &'static str {
    match host {
        ToolHost::Claude => "Bash",
        ToolHost::Codex => "Bash",
        ToolHost::Cursor => "Shell",
        ToolHost::OpenCode => "bash",
        ToolHost::Hermes => "terminal",
        ToolHost::Gemini => "run_shell_command",
        ToolHost::Antigravity => "run_command",
        ToolHost::Kiro => "execute_bash",
        ToolHost::Pi | ToolHost::CopilotCli => "bash",
        ToolHost::CopilotVscode => "Bash",
        ToolHost::Generic => "",
    }
}

fn argv_from_values(values: &[Value]) -> Option<Vec<String>> {
    if values.is_empty() || values.len() > MAX_ARGV_ITEMS {
        return None;
    }
    let argv = values
        .iter()
        .map(Value::as_str)
        .map(|value| value.map(str::to_owned))
        .collect::<Option<Vec<_>>>()?;
    valid_argv(&argv).then_some(argv)
}

fn valid_argv(argv: &[String]) -> bool {
    argv.len() <= MAX_ARGV_ITEMS
        && argv.iter().all(|argument| {
            argument.len() <= MAX_ARG_BYTES && !argument.chars().any(char::is_control)
        })
        && argv.iter().map(|argument| argument.len()).sum::<usize>() <= MAX_COMMAND_BYTES
}

fn shell_argv(command: &str) -> Option<Vec<String>> {
    if command.is_empty()
        || command.len() > MAX_COMMAND_BYTES
        || command.trim_start() != command
        || command.chars().any(|character| {
            matches!(
                character,
                '|' | '&'
                    | ';'
                    | '<'
                    | '>'
                    | '('
                    | ')'
                    | '$'
                    | '`'
                    | '\\'
                    | '#'
                    | '\n'
                    | '\r'
                    | '\0'
            )
        })
    {
        return None;
    }
    let mut argv = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    for character in command.chars() {
        match (quote, character) {
            (None, '\'' | '"') => quote = Some(character),
            (Some(expected), value) if value == expected => quote = None,
            (None, value) if value.is_whitespace() => {
                if !current.is_empty() {
                    argv.push(std::mem::take(&mut current));
                }
            }
            (_, value) => current.push(value),
        }
    }
    if quote.is_some() {
        return None;
    }
    if !current.is_empty() {
        argv.push(current);
    }
    (!argv.is_empty() && valid_argv(&argv)).then_some(argv)
}

fn is_lwc_executable(value: &str, installer_owned_lwc: Option<&str>) -> bool {
    value == "lwc" || (Path::new(value).is_absolute() && installer_owned_lwc == Some(value))
}

fn invocation_action(argv: &[String]) -> Option<String> {
    let mut index = 1;
    while let Some(argument) = argv.get(index).map(String::as_str) {
        match argument {
            "--scope" => {
                let scope = argv.get(index + 1)?.as_str();
                if !matches!(scope, "project" | "global" | "all") {
                    return None;
                }
                index += 2;
            }
            "--changeset" => {
                let name = argv.get(index + 1)?;
                if name.is_empty()
                    || name.len() > 80
                    || name.contains(['/', '\\'])
                    || name.chars().any(char::is_control)
                {
                    return None;
                }
                index += 2;
            }
            value if value.starts_with("--scope=") => {
                if !matches!(
                    value.strip_prefix("--scope="),
                    Some("project" | "global" | "all")
                ) {
                    return None;
                }
                index += 1;
            }
            value if value.starts_with("--changeset=") => {
                let name = value.strip_prefix("--changeset=")?;
                if name.is_empty()
                    || name.len() > 80
                    || name.contains(['/', '\\'])
                    || name.chars().any(char::is_control)
                {
                    return None;
                }
                index += 1;
            }
            _ => break,
        }
    }
    let command = argv.get(index)?.as_str();
    match command {
        "init" | "serve" | "view" | "search" | "context" | "lint" | "log" | "remember"
        | "trans" => Some(command.to_owned()),
        "sync" => {
            let tail = &argv[index + 1..];
            let has_option = |name: &str| {
                tail.iter()
                    .any(|value| value == name || value.starts_with(&format!("{name}=")))
            };
            let action = if has_option("--abort") {
                "sync.abort"
            } else if has_option("--resolve") {
                "sync.resolve"
            } else if has_option("--resume") {
                "sync.resume"
            } else {
                "sync.start"
            };
            Some(action.to_owned())
        }
        "work" | "cg" | "office" | "tutor" | "book" | "practice" | "changeset" | "schema"
        | "purpose" | "source" | "page" | "tag" | "todo" | "plan" | "load" | "agent" | "ingest"
        | "memory" | "config" | "maintenance" | "checkpoint" | "graph" | "weight" | "span" => {
            let subcommand = argv.get(index + 1)?.as_str();
            if !safe_code(subcommand, 64, false) || subcommand.starts_with('-') {
                return None;
            }
            let normalized = match command {
                "cg" if !matches!(subcommand, "init" | "status") => "run",
                "office" | "tutor" | "book" | "practice" => "run",
                _ => subcommand,
            };
            Some(format!("{command}.{normalized}"))
        }
        _ => None,
    }
}

fn consent_advice_for(action: &str) -> Option<ConsentAdvice> {
    let (boundary, code, reason) = match action {
        "init" => (
            ConsentBoundary::StateInitialization,
            "lwc_init",
            "Initialize durable LWC state.",
        ),
        "config.set" => (
            ConsentBoundary::ConfigurationChange,
            "lwc_config_set",
            "Set LWC configuration.",
        ),
        "config.unset" => (
            ConsentBoundary::ConfigurationChange,
            "lwc_config_unset",
            "Unset LWC configuration.",
        ),
        "cg.init" => (
            ConsentBoundary::CodeGraphInitialization,
            "lwc_cg_init",
            "Initialize the LWC CodeGraph index.",
        ),
        "sync.start" => (
            ConsentBoundary::SyncStart,
            "lwc_sync_start",
            "Start a new LWC Sync session.",
        ),
        "agent.install" => (
            ConsentBoundary::AgentIntegrationChange,
            "lwc_agent_install",
            "Install LWC Agent integration.",
        ),
        "agent.uninstall" => (
            ConsentBoundary::AgentIntegrationChange,
            "lwc_agent_uninstall",
            "Uninstall LWC Agent integration.",
        ),
        "changeset.commit" => (
            ConsentBoundary::ChangesetFinalization,
            "lwc_changeset_commit",
            "Commit an LWC changeset.",
        ),
        "changeset.discard" => (
            ConsentBoundary::ChangesetFinalization,
            "lwc_changeset_discard",
            "Discard an LWC changeset.",
        ),
        "changeset.rollback" => (
            ConsentBoundary::ChangesetFinalization,
            "lwc_changeset_rollback",
            "Roll back an LWC changeset.",
        ),
        "checkpoint.restore" => (
            ConsentBoundary::CheckpointRestore,
            "lwc_checkpoint_restore",
            "Restore an LWC checkpoint.",
        ),
        "source.remove" => (
            ConsentBoundary::DurableContentRemoval,
            "lwc_source_remove",
            "Remove an LWC source.",
        ),
        "page.remove" => (
            ConsentBoundary::DurableContentRemoval,
            "lwc_page_remove",
            "Remove an LWC page.",
        ),
        "schema.set" => (
            ConsentBoundary::SchemaChange,
            "lwc_schema_set",
            "Set the durable LWC schema.",
        ),
        _ => return None,
    };
    Some(ConsentAdvice {
        boundary,
        code,
        reason,
    })
}

fn receipt_objects<'a>(root: &'a Map<String, Value>) -> ReceiptObjects<'a> {
    const CONTAINERS: &[&str] = &[
        "work",
        "plan",
        "todo",
        "sync",
        "changeset",
        "source",
        "ingest",
        "memory",
        "tutor",
        "practice",
        "book",
    ];
    let mut general = CONTAINERS
        .iter()
        .filter_map(|container| {
            root.get(*container)
                .and_then(Value::as_object)
                .map(|object| (*container, object))
        })
        .collect::<Vec<_>>();
    if let Some(work) = root.get("graph_work").and_then(Value::as_object) {
        general.push(("work", work));
    }
    if let Some(graph) = root.get("graph").and_then(Value::as_object) {
        if let Some(work) = graph.get("work").and_then(Value::as_object) {
            general.push(("work", work));
        }
        general.push(("graph", graph));
    }
    ReceiptObjects {
        general,
        pressure: root.get("pressure").and_then(Value::as_object),
        retained: root.get("retained").and_then(Value::as_object),
    }
}

fn has_work_identifier(object: &Map<String, Value>) -> bool {
    object
        .get("work_id")
        .or_else(|| object.get("id"))
        .and_then(identifier_value)
        .is_some()
}

fn code_field(object: &Map<String, Value>, key: &str) -> Option<String> {
    object
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| safe_code(value, 64, true))
        .map(str::to_owned)
}

fn receipt_progress(object: &Map<String, Value>) -> Option<ReceiptProgress> {
    Some(ReceiptProgress {
        completed: object.get("completed")?.as_u64()?,
        total: object.get("total").and_then(Value::as_u64),
        sequence: object.get("sequence").and_then(Value::as_u64),
    })
}

fn extract_identifiers(
    object: &Map<String, Value>,
    container: Option<&str>,
    output: &mut BTreeMap<String, String>,
) {
    const KEYS: &[&str] = &[
        "id",
        "work_id",
        "plan_id",
        "todo_id",
        "changeset_id",
        "source_id",
        "session_id",
        "event_id",
        "turn_id",
        "attempt_id",
        "paper_id",
        "goal_id",
        "review_id",
        "book_id",
        "subject_id",
    ];
    for key in KEYS {
        let Some(value) = object.get(*key).and_then(identifier_value) else {
            continue;
        };
        let output_key = if *key == "id" {
            container.map_or_else(|| "id".to_owned(), |container| format!("{container}_id"))
        } else {
            (*key).to_owned()
        };
        output.entry(output_key).or_insert(value);
    }
}

fn identifier_value(value: &Value) -> Option<String> {
    let value = match value {
        Value::String(value) => value.clone(),
        Value::Number(value) => value.to_string(),
        _ => return None,
    };
    (safe_code(&value, 128, true) && !value.contains('/')).then_some(value)
}

fn extract_revisions(object: &Map<String, Value>, output: &mut BTreeMap<String, String>) {
    for key in [
        "revision",
        "if_revision",
        "next_revision",
        "post_revision",
        "draft_revision",
        "rollback_revision",
    ] {
        if let Some(value) = object.get(key).and_then(identifier_value) {
            output.entry(key.to_owned()).or_insert(value);
        }
    }
}

fn first_code(
    root: &Map<String, Value>,
    objects: &[(&str, &Map<String, Value>)],
    key: &str,
) -> Option<String> {
    std::iter::once(root)
        .chain(objects.iter().map(|(_, object)| *object))
        .find_map(|object| code_field(object, key))
}

fn extract_counts(object: &Map<String, Value>, output: &mut BTreeMap<String, u64>) {
    for key in [
        "count",
        "pending",
        "active",
        "omitted",
        "staged_operation_count",
        "conflict_count",
        "documents",
        "items",
        "sources",
        "pages",
        "signals",
    ] {
        if let Some(value) = object.get(key).and_then(Value::as_u64) {
            output.entry(key.to_owned()).or_insert(value);
        }
    }
}

fn extract_memory_root_counts(root: &Map<String, Value>, output: &mut BTreeMap<String, u64>) {
    for key in [
        "pending_hints",
        "retained_count",
        "logical_bytes",
        "max_bytes",
    ] {
        if let Some(value) = root.get(key).and_then(Value::as_u64) {
            output.insert(key.to_owned(), value);
        }
    }
}

fn extract_retained_counts(
    retained: Option<&Map<String, Value>>,
    output: &mut BTreeMap<String, u64>,
) {
    let Some(retained) = retained else { return };
    if let Some(events) = retained.get("events").and_then(Value::as_u64) {
        output.insert("retained_count".to_owned(), events);
    }
    if let Some(logical_bytes) = retained.get("logical_bytes").and_then(Value::as_u64) {
        output.insert("logical_bytes".to_owned(), logical_bytes);
    }
}

fn extract_pressure_counts(
    pressure: Option<&Map<String, Value>>,
    output: &mut BTreeMap<String, u64>,
) {
    let Some(pressure) = pressure else { return };
    for key in ["logical_bytes", "max_bytes"] {
        if let Some(value) = pressure.get(key).and_then(Value::as_u64) {
            output.insert(key.to_owned(), value);
        }
    }
}

fn first_command_action(
    root: &Map<String, Value>,
    objects: &[(&str, &Map<String, Value>)],
    key: &str,
) -> Option<String> {
    std::iter::once(root)
        .chain(objects.iter().map(|(_, object)| *object))
        .find_map(|object| {
            object
                .get(key)
                .and_then(Value::as_str)
                .and_then(normalized_shell_action)
        })
}

fn first_next_action(
    root: &Map<String, Value>,
    objects: &[(&str, &Map<String, Value>)],
) -> Option<String> {
    std::iter::once(root)
        .chain(objects.iter().map(|(_, object)| *object))
        .find_map(|object| {
            let value = object.get("next_action")?.as_str()?;
            normalized_shell_action(value)
                .or_else(|| safe_action_code(value).then(|| value.to_owned()))
        })
}

fn normalized_shell_action(command: &str) -> Option<String> {
    let argv = shell_argv(command)?;
    if argv.first().map(String::as_str) != Some("lwc") {
        return None;
    }
    invocation_action(&argv)
}

fn extract_error_code(object: &Map<String, Value>) -> Option<String> {
    object
        .get("error_code")
        .and_then(Value::as_str)
        .or_else(|| {
            object
                .get("error")
                .and_then(Value::as_object)
                .and_then(|error| error.get("code"))
                .and_then(Value::as_str)
        })
        .filter(|value| safe_code(value, 64, true))
        .map(str::to_owned)
}

fn safe_code(value: &str, max_bytes: usize, allow_dot: bool) -> bool {
    !value.is_empty()
        && value.len() <= max_bytes
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(byte, b'_' | b'-' | b':')
                || (allow_dot && byte == b'.')
        })
}

fn safe_action_code(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-' | b'.')
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn shell(host: ToolHost, payload: Value) -> RecognizedInvocation {
        recognize_invocation(host, &payload, None).expect("expected recognized LWC invocation")
    }

    #[test]
    fn each_host_uses_only_its_allowlisted_envelope_and_shell_tool() {
        let fixtures = [
            (
                ToolHost::Claude,
                json!({"tool_name":"Bash","tool_input":{"command":"lwc plan brief abc"}}),
            ),
            (
                ToolHost::Codex,
                json!({"tool_name":"Bash","tool_input":{"command":"lwc plan brief abc"}}),
            ),
            (
                ToolHost::Cursor,
                json!({"tool_name":"Shell","tool_input":{"command":"lwc plan brief abc"}}),
            ),
            (
                ToolHost::Gemini,
                json!({"tool_name":"run_shell_command","tool_input":{"command":"lwc plan brief abc"}}),
            ),
            (
                ToolHost::Hermes,
                json!({"tool_name":"terminal","tool_input":{"command":"lwc plan brief abc"}}),
            ),
            (
                ToolHost::Antigravity,
                json!({"toolCall":{"name":"run_command","args":{"CommandLine":"lwc plan brief abc"}}}),
            ),
            (
                ToolHost::Pi,
                json!({"tool_name":"bash","args":{"command":"lwc plan brief abc"}}),
            ),
            (
                ToolHost::CopilotCli,
                json!({"toolName":"bash","toolArgs":"{\"command\":\"lwc plan brief abc\"}"}),
            ),
            (
                ToolHost::CopilotVscode,
                json!({"tool_name":"Bash","tool_input":{"command":"lwc plan brief abc"}}),
            ),
            (
                ToolHost::OpenCode,
                json!({"tool":"bash","args":{"command":"lwc plan brief abc"}}),
            ),
        ];

        for (host, payload) in fixtures {
            let invocation = shell(host, payload);
            assert_eq!(invocation.action, "plan.brief");
            assert_eq!(invocation.transport, InvocationTransport::Shell);
            assert_eq!(invocation.consent, ConsentClass::Noop);
        }
        assert!(
            recognize_invocation(
                ToolHost::Generic,
                &json!({"tool_name":"Bash","tool_input":{"command":"lwc status"}}),
                None,
            )
            .is_none()
        );
        assert!(
            recognize_invocation(
                ToolHost::Claude,
                &json!({"toolName":"Bash","toolInput":{"command":"lwc status"}}),
                None,
            )
            .is_none()
        );
        assert!(
            recognize_invocation(
                ToolHost::Codex,
                &json!({"tool_name":"exec_command","tool_input":{"cmd":"lwc plan brief abc"}}),
                None,
            )
            .is_none()
        );
        assert!(
            recognize_invocation(
                ToolHost::Antigravity,
                &json!({"toolName":"run_command","toolInput":{"command":"lwc plan brief abc"}}),
                None,
            )
            .is_none()
        );
        assert!(
            recognize_invocation(
                ToolHost::Kiro,
                &json!({"tool_name":"execute_bash","tool_input":{"command":"lwc plan brief abc"}}),
                None,
            )
            .is_none()
        );
        assert!(
            recognize_invocation(
                ToolHost::Kiro,
                &json!({
                    "tool_name":"execute_bash",
                    "tool_input":{"argv":["lwc","config","show"]}
                }),
                None,
            )
            .is_none()
        );
        assert!(
            recognize_invocation(
                ToolHost::Antigravity,
                &json!({
                    "toolCall":{"name":"run_command","args":{"argv":["lwc","config","show"]}}
                }),
                None,
            )
            .is_none()
        );
        for tool_args in ["not json".to_owned(), "x".repeat(MAX_COMMAND_BYTES + 1)] {
            assert!(
                recognize_invocation(
                    ToolHost::CopilotCli,
                    &json!({"toolName":"bash","toolArgs":tool_args}),
                    None,
                )
                .is_none()
            );
        }
    }

    #[test]
    fn mcp_requires_an_exact_lwc_read_only_tool_identity() {
        let combined = recognize_invocation(
            ToolHost::Claude,
            &json!({"tool_name":"mcp__lwc__lwc_explore","tool_input":{}}),
            None,
        )
        .unwrap();
        assert_eq!(combined.action, "mcp.lwc_explore");
        assert_eq!(combined.transport, InvocationTransport::Mcp);
        assert_eq!(combined.consent, ConsentClass::Noop);
        assert_eq!(combined.consent_advice, None);

        let separate = recognize_invocation(
            ToolHost::Gemini,
            &json!({
                "tool_name":"lwc_explore",
                "mcp_server_name":"lwc",
                "tool_input":{}
            }),
            None,
        )
        .unwrap();
        assert_eq!(separate.action, "mcp.lwc_explore");
        assert_eq!(separate.consent_advice, None);

        let codegraph = recognize_invocation(
            ToolHost::Claude,
            &json!({"tool_name":"mcp__lwc__lwc_codegraph","tool_input":{}}),
            None,
        )
        .unwrap();
        assert_eq!(codegraph.action, "mcp.lwc_codegraph");
        assert_eq!(codegraph.transport, InvocationTransport::Mcp);
        assert_eq!(codegraph.consent, ConsentClass::Noop);
        assert_eq!(codegraph.consent_advice, None);

        for payload in [
            json!({"tool_name":"lwc_explore","tool_input":{}}),
            json!({"tool_name":"lwc_explore","mcp_server_name":"other","tool_input":{}}),
            json!({"tool_name":"mcp__other__lwc_explore","tool_input":{}}),
            json!({"tool_name":"mcp__lwc__other","tool_input":{}}),
            json!({"tool_name":"lwc_codegraph","tool_input":{}}),
            json!({"tool_name":"lwc_codegraph","mcp_server_name":"other","tool_input":{}}),
            json!({"tool_name":"mcp__other__lwc_codegraph","tool_input":{}}),
        ] {
            assert!(recognize_invocation(ToolHost::Claude, &payload, None).is_none());
        }
    }

    #[test]
    fn cursor_before_shell_execution_reads_only_the_exact_top_level_command() {
        let invocation = shell(
            ToolHost::Cursor,
            json!({
                "conversation_id": "PRIVATE_CONVERSATION",
                "hook_event_name": "beforeShellExecution",
                "command": "lwc --scope global schema set '/PRIVATE/秘密-schema.md'",
                "cwd": "/PRIVATE/CWD",
                "sandbox": false,
                "prompt": "PRIVATE_PROMPT"
            }),
        );
        assert_eq!(invocation.action, "schema.set");
        assert_eq!(invocation.transport, InvocationTransport::Shell);
        assert_eq!(invocation.consent, ConsentClass::Ask);
        assert_eq!(
            invocation.consent_advice,
            Some(ConsentAdvice {
                boundary: ConsentBoundary::SchemaChange,
                code: "lwc_schema_set",
                reason: "Set the durable LWC schema.",
            })
        );
        let rendered = format!("{invocation:?}");
        for secret in [
            "PRIVATE_CONVERSATION",
            "PRIVATE/秘密-schema.md",
            "PRIVATE/CWD",
            "PRIVATE_PROMPT",
        ] {
            assert!(!rendered.contains(secret), "leaked {secret}: {rendered}");
        }

        for payload in [
            json!({"command":"lwc schema set schema.md"}),
            json!({
                "hook_event_name":"afterShellExecution",
                "command":"lwc schema set schema.md"
            }),
            json!({
                "hook_event_name":"beforeShellExecution",
                "tool_input":{"command":"lwc schema set schema.md"}
            }),
            json!({
                "hook_event_name":"beforeShellExecution",
                "Command":"lwc schema set schema.md"
            }),
            json!({
                "hook_event_name":"beforeShellExecution",
                "command":["lwc","schema","set","schema.md"]
            }),
            json!({
                "hook_event_name":"beforeShellExecution",
                "command":"x".repeat(MAX_COMMAND_BYTES + 1)
            }),
        ] {
            assert!(
                recognize_invocation(ToolHost::Cursor, &payload, None).is_none(),
                "accepted noncanonical Cursor shell payload: {payload}"
            );
        }

        let cursor_payload = json!({
            "hook_event_name":"beforeShellExecution",
            "command":"lwc config set --plan enabled"
        });
        for host in [
            ToolHost::Claude,
            ToolHost::Codex,
            ToolHost::OpenCode,
            ToolHost::Hermes,
            ToolHost::Gemini,
            ToolHost::Antigravity,
            ToolHost::Kiro,
            ToolHost::Pi,
            ToolHost::CopilotCli,
            ToolHost::CopilotVscode,
            ToolHost::Generic,
        ] {
            assert!(
                recognize_invocation(host, &cursor_payload, None).is_none(),
                "{host:?} accepted Cursor's native shell payload"
            );
        }
    }

    #[test]
    fn direct_argv_and_exact_installer_owned_executable_are_supported() {
        let argv = recognize_invocation(
            ToolHost::Codex,
            &json!({
                "tool_name":"Bash",
                "tool_input":{"command":["lwc","config","set","--graph","grafeo"]}
            }),
            None,
        )
        .unwrap();
        assert_eq!(argv.transport, InvocationTransport::Argv);
        assert_eq!(argv.action, "config.set");
        assert_eq!(argv.consent, ConsentClass::Ask);

        let owned_executable = if cfg!(windows) {
            "C:/opt/lwc/bin/lwc.exe"
        } else {
            "/opt/lwc/bin/lwc"
        };
        let owned_command = format!("{owned_executable} cg status");
        let owned = recognize_invocation(
            ToolHost::Claude,
            &json!({
                "tool_name":"Bash",
                "tool_input":{"command":owned_command}
            }),
            Some(owned_executable),
        )
        .unwrap();
        assert_eq!(owned.action, "cg.status");
        let unowned_executable = if cfg!(windows) {
            "C:/tmp/unowned/lwc.exe"
        } else {
            "/tmp/unowned/lwc"
        };
        let unowned_command = format!("{unowned_executable} cg status");
        assert!(
            recognize_invocation(
                ToolHost::Claude,
                &json!({
                    "tool_name":"Bash",
                    "tool_input":{"command":unowned_command}
                }),
                Some(owned_executable),
            )
            .is_none()
        );
        assert!(
            recognize_invocation(
                ToolHost::Claude,
                &json!({
                    "tool_name":"Bash",
                    "tool_input":{"command":"relative/lwc cg status"}
                }),
                Some("relative/lwc"),
            )
            .is_none()
        );
        let literal_pipe_argument = recognize_invocation(
            ToolHost::Codex,
            &json!({
                "tool_name":"Bash",
                "tool_input":{"command":["lwc","search","alpha|beta"]}
            }),
            None,
        )
        .unwrap();
        assert_eq!(literal_pipe_argument.transport, InvocationTransport::Argv);
        assert_eq!(literal_pipe_argument.action, "search");
    }

    #[test]
    fn shell_grammar_rejects_operators_wrappers_environment_and_unrelated_text() {
        for command in [
            "lwc status | cat",
            "lwc status > /tmp/out",
            "lwc status < input",
            "lwc status; echo pwned",
            "lwc status && echo pwned",
            "lwc status &",
            "lwc status $(whoami)",
            "lwc status `whoami`",
            "lwc config show (echo pwned)",
            "lwc status\\; echo pwned",
            "env lwc status",
            "FOO=bar lwc status",
            "sudo lwc status",
            "command lwc status",
            "sh -c 'lwc status'",
            "powershell -Command 'lwc status'",
            "lwc status\necho pwned",
            "请运行 lwc status",
            "echo lwc status",
        ] {
            assert!(
                recognize_invocation(
                    ToolHost::Claude,
                    &json!({"tool_name":"Bash","tool_input":{"command":command}}),
                    None,
                )
                .is_none(),
                "recognized unsafe or unrelated shell text: {command}"
            );
        }
        let chinese = shell(
            ToolHost::Claude,
            json!({"tool_name":"Bash","tool_input":{"command":"lwc search '中文 问题'"}}),
        );
        assert_eq!(chinese.action, "search");
    }

    #[test]
    fn consent_ask_closed_set_has_only_static_typed_advice() {
        let cases = [
            (
                "lwc init",
                "init",
                ConsentBoundary::StateInitialization,
                "lwc_init",
                "Initialize durable LWC state.",
            ),
            (
                "lwc --scope=global config set --memory enabled",
                "config.set",
                ConsentBoundary::ConfigurationChange,
                "lwc_config_set",
                "Set LWC configuration.",
            ),
            (
                "lwc --scope project config unset --graph",
                "config.unset",
                ConsentBoundary::ConfigurationChange,
                "lwc_config_unset",
                "Unset LWC configuration.",
            ),
            (
                "lwc cg init",
                "cg.init",
                ConsentBoundary::CodeGraphInitialization,
                "lwc_cg_init",
                "Initialize the LWC CodeGraph index.",
            ),
            (
                "lwc sync private-host /PRIVATE/project",
                "sync.start",
                ConsentBoundary::SyncStart,
                "lwc_sync_start",
                "Start a new LWC Sync session.",
            ),
            (
                "lwc agent install --target PRIVATE_HOST",
                "agent.install",
                ConsentBoundary::AgentIntegrationChange,
                "lwc_agent_install",
                "Install LWC Agent integration.",
            ),
            (
                "lwc agent uninstall --target PRIVATE_HOST",
                "agent.uninstall",
                ConsentBoundary::AgentIntegrationChange,
                "lwc_agent_uninstall",
                "Uninstall LWC Agent integration.",
            ),
            (
                "lwc changeset commit PRIVATE_DRAFT",
                "changeset.commit",
                ConsentBoundary::ChangesetFinalization,
                "lwc_changeset_commit",
                "Commit an LWC changeset.",
            ),
            (
                "lwc changeset discard PRIVATE_DRAFT",
                "changeset.discard",
                ConsentBoundary::ChangesetFinalization,
                "lwc_changeset_discard",
                "Discard an LWC changeset.",
            ),
            (
                "lwc changeset rollback PRIVATE_CHANGESET_ID",
                "changeset.rollback",
                ConsentBoundary::ChangesetFinalization,
                "lwc_changeset_rollback",
                "Roll back an LWC changeset.",
            ),
            (
                "lwc checkpoint restore PRIVATE_CHECKPOINT",
                "checkpoint.restore",
                ConsentBoundary::CheckpointRestore,
                "lwc_checkpoint_restore",
                "Restore an LWC checkpoint.",
            ),
            (
                "lwc source remove PRIVATE_SOURCE_ID",
                "source.remove",
                ConsentBoundary::DurableContentRemoval,
                "lwc_source_remove",
                "Remove an LWC source.",
            ),
            (
                "lwc page remove '秘密-page'",
                "page.remove",
                ConsentBoundary::DurableContentRemoval,
                "lwc_page_remove",
                "Remove an LWC page.",
            ),
            (
                "lwc schema set '/PRIVATE/秘密-schema.md'",
                "schema.set",
                ConsentBoundary::SchemaChange,
                "lwc_schema_set",
                "Set the durable LWC schema.",
            ),
        ];

        for (command, action, boundary, code, reason) in cases {
            let invocation = shell(
                ToolHost::Claude,
                json!({"tool_name":"Bash","tool_input":{"command":command}}),
            );
            assert_eq!(invocation.action, action);
            assert_eq!(invocation.consent, ConsentClass::Ask);
            assert_eq!(
                invocation.consent_advice,
                Some(ConsentAdvice {
                    boundary,
                    code,
                    reason,
                })
            );
            let advice = invocation.consent_advice.unwrap();
            assert!(advice.code.is_ascii());
            assert!(advice.code.len() <= 256);
            assert!(advice.reason.len() <= 256);
            let rendered = format!("{invocation:?}");
            for secret in [
                "PRIVATE_HOST",
                "PRIVATE_DRAFT",
                "PRIVATE_CHANGESET_ID",
                "PRIVATE_CHECKPOINT",
                "PRIVATE_SOURCE_ID",
                "PRIVATE/秘密-schema.md",
                "秘密-page",
            ] {
                assert!(!rendered.contains(secret), "leaked {secret}: {rendered}");
            }
        }
    }

    #[test]
    fn every_action_outside_the_ask_closed_set_is_noop_without_advice() {
        for command in [
            "lwc --scope project plan brief abc",
            "lwc plan current",
            "lwc plan advance abc --if-revision 2 --done step",
            "lwc plan complete abc --if-revision 3",
            "lwc todo create 'later'",
            "lwc search '当前问题'",
            "lwc load tag strong",
            "lwc cg status",
            "lwc graph status",
            "lwc graph verify",
            "lwc work status abc",
            "lwc work resume abc",
            "lwc work cancel abc",
            "lwc config show",
            "lwc sync host /private/project --resume abc",
            "lwc sync host /private/project --resume abc --resolve packet.json",
            "lwc sync host /private/project --abort abc",
            "lwc sync host /private/project --resume=abc",
            "lwc sync host /private/project --resume=abc --resolve=packet.json",
            "lwc sync host /private/project --abort=abc",
            "lwc agent status",
            "lwc agent refresh",
            "lwc changeset begin draft",
            "lwc changeset list",
            "lwc changeset show draft",
            "lwc checkpoint create snapshot",
            "lwc checkpoint list",
            "lwc source list",
            "lwc source show source-id",
            "lwc page put slug --title title --file page.md",
            "lwc page list",
            "lwc page show slug",
            "lwc schema show",
            "lwc memory status",
            "lwc memory recall query",
            "lwc maintenance compact",
            "lwc ingest retry source-id",
            "lwc office status",
            "lwc tutor status",
            "lwc practice status",
            "lwc book status",
            "lwc trans input.docx output.md",
        ] {
            let invocation = shell(
                ToolHost::Claude,
                json!({"tool_name":"Bash","tool_input":{"command":command}}),
            );
            assert_eq!(invocation.consent, ConsentClass::Noop, "{command}");
            assert_eq!(invocation.consent_advice, None, "{command}");
        }

        fn label(class: ConsentClass) -> &'static str {
            match class {
                ConsentClass::Noop => "noop",
                ConsentClass::Ask => "ask",
            }
        }
        assert_eq!(label(ConsentClass::Noop), "noop");
        assert_eq!(label(ConsentClass::Ask), "ask");
        assert!(
            recognize_invocation(
                ToolHost::Claude,
                &json!({"tool_name":"Bash","tool_input":{"command":"rm -rf .lwc"}}),
                None,
            )
            .is_none()
        );
    }

    #[test]
    fn receipt_extracts_only_bounded_allowlisted_metadata() {
        let id = "a".repeat(64);
        let invocation = shell(
            ToolHost::Claude,
            json!({"tool_name":"Bash","tool_input":{"command":"lwc plan advance abc"}}),
        );
        let stdout = json!({
            "action": "plan.advance",
            "plan": {
                "id": id,
                "revision": 7,
                "state": "active",
                "status": "ready",
                "phase": "next",
                "completed": 1,
                "total": 3,
                "sequence": 9,
                "path": "/private/wiki.db",
                "message": "PRIVATE MESSAGE",
                "body": "PRIVATE BODY",
                "result": {"secret": "PRIVATE RESULT"}
            },
            "next_action": format!("lwc plan brief {id}"),
            "error": {
                "code": "revision_conflict",
                "message": "PRIVATE ERROR",
                "details": {"recovery_command": format!("lwc plan brief {id}")}
            },
            "prompt": "PRIVATE PROMPT",
            "args": ["PRIVATE ARG"],
            "path": "/private/other"
        })
        .to_string();
        let payload = json!({
            "tool_name": "Bash",
            "tool_input": {"command": "lwc plan advance abc"},
            "tool_response": {
                "stdout": stdout,
                "stderr": "PRIVATE STDERR"
            }
        });

        let receipt = parse_receipt(ToolHost::Claude, &payload, &invocation).unwrap();

        assert_eq!(receipt.action.as_deref(), Some("plan.advance"));
        assert_eq!(receipt.identifiers.get("plan_id"), Some(&id));
        assert_eq!(
            receipt.revisions.get("revision").map(String::as_str),
            Some("7")
        );
        assert_eq!(receipt.state.as_deref(), Some("active"));
        assert_eq!(receipt.status.as_deref(), Some("ready"));
        assert_eq!(receipt.phase.as_deref(), Some("next"));
        assert_eq!(
            receipt.progress,
            Some(ReceiptProgress {
                completed: 1,
                total: Some(3),
                sequence: Some(9),
            })
        );
        assert_eq!(receipt.next_action.as_deref(), Some("plan.brief"));
        assert_eq!(receipt.recovery_action.as_deref(), Some("plan.brief"));
        assert_eq!(receipt.error_code.as_deref(), Some("revision_conflict"));
        let retained = format!("{receipt:?}");
        for secret in [
            "/private",
            "PRIVATE MESSAGE",
            "PRIVATE BODY",
            "PRIVATE RESULT",
            "PRIVATE ERROR",
            "PRIVATE PROMPT",
            "PRIVATE ARG",
            "PRIVATE STDERR",
            "stdout",
            "stderr",
        ] {
            assert!(
                !retained.contains(secret),
                "receipt leaked {secret}: {retained}"
            );
        }
    }

    #[test]
    fn receipt_extracts_typed_memory_counts_from_only_fixed_containers() {
        let invocation = shell(
            ToolHost::Claude,
            json!({"tool_name":"Bash","tool_input":{"command":"lwc memory status"}}),
        );
        let stdout = json!({
            "pending_hints": 3,
            "retained": {
                "events": 9,
                "logical_bytes": 850,
                "message": "PRIVATE_RETAINED_MESSAGE"
            },
            "pressure": {
                "logical_bytes": 850,
                "max_bytes": 1000,
                "ratio": 0.85,
                "path": "/PRIVATE_PRESSURE_PATH"
            },
            "policy": {"max_bytes": 999_999},
            "arbitrary": {
                "pending_hints": 999,
                "retained_count": 999,
                "logical_bytes": 999,
                "max_bytes": 999,
                "prompt": "PRIVATE_NESTED_PROMPT"
            },
            "database": "/PRIVATE_MEMORY_DATABASE/wiki.db",
            "body": "PRIVATE_MEMORY_BODY"
        })
        .to_string();
        let payload = json!({
            "tool_name":"Bash",
            "tool_input":{"command":"lwc memory status"},
            "tool_response":{"stdout":stdout,"stderr":"PRIVATE_MEMORY_STDERR"}
        });

        let receipt = parse_receipt(ToolHost::Claude, &payload, &invocation).unwrap();
        assert_eq!(
            receipt.counts,
            BTreeMap::from([
                ("logical_bytes".to_owned(), 850),
                ("max_bytes".to_owned(), 1000),
                ("pending_hints".to_owned(), 3),
                ("retained_count".to_owned(), 9),
            ])
        );
        let retained = format!("{receipt:?}");
        for secret in [
            "0.85",
            "999999",
            "PRIVATE_RETAINED_MESSAGE",
            "/PRIVATE_PRESSURE_PATH",
            "PRIVATE_NESTED_PROMPT",
            "/PRIVATE_MEMORY_DATABASE",
            "PRIVATE_MEMORY_BODY",
            "PRIVATE_MEMORY_STDERR",
        ] {
            assert!(
                !retained.contains(secret),
                "receipt leaked {secret}: {retained}"
            );
        }
    }

    #[test]
    fn receipt_extracts_only_direct_graph_work_and_ignores_recursive_lookalikes() {
        let invocation = shell(
            ToolHost::Claude,
            json!({
                "tool_name":"Bash",
                "tool_input":{"command":"lwc page put safe --title safe --file safe.md"}
            }),
        );
        let work_id = "a".repeat(64);
        let stdout = json!({
            "graph": {
                "work": {
                    "id": work_id,
                    "state": "queued",
                    "phase": "projecting",
                    "completed": 2,
                    "total": 10,
                    "sequence": 4,
                    "pending_hints": 777,
                    "max_bytes": 999_999,
                    "message": "PRIVATE_GRAPH_WORK_MESSAGE",
                    "result": {"body": "PRIVATE_GRAPH_WORK_RESULT"}
                },
                "deeper": {
                    "work": {
                        "id": "PRIVATE_RECURSIVE_WORK_ID",
                        "state": "failed"
                    }
                }
            },
            "untrusted": {
                "work": {
                    "id": "PRIVATE_UNTRUSTED_WORK_ID",
                    "state": "failed"
                }
            }
        })
        .to_string();
        let payload = json!({
            "tool_name":"Bash",
            "tool_input":{"command":"lwc page put safe --title safe --file safe.md"},
            "tool_response":{"stdout":stdout}
        });

        let receipt = parse_receipt(ToolHost::Claude, &payload, &invocation).unwrap();
        assert_eq!(receipt.identifiers.get("work_id"), Some(&work_id));
        assert_eq!(receipt.state.as_deref(), Some("queued"));
        assert_eq!(receipt.phase.as_deref(), Some("projecting"));
        assert!(!receipt.counts.contains_key("pending_hints"));
        assert!(!receipt.counts.contains_key("max_bytes"));
        assert_eq!(
            receipt.progress,
            Some(ReceiptProgress {
                completed: 2,
                total: Some(10),
                sequence: Some(4),
            })
        );
        let retained = format!("{receipt:?}");
        for secret in [
            "PRIVATE_GRAPH_WORK_MESSAGE",
            "PRIVATE_GRAPH_WORK_RESULT",
            "PRIVATE_RECURSIVE_WORK_ID",
            "PRIVATE_UNTRUSTED_WORK_ID",
        ] {
            assert!(
                !retained.contains(secret),
                "receipt leaked {secret}: {retained}"
            );
        }
    }

    #[test]
    fn receipt_extracts_checkpoint_restore_graph_work_from_dispatch_shape() {
        let invocation = shell(
            ToolHost::Claude,
            json!({
                "tool_name":"Bash",
                "tool_input":{"command":"lwc checkpoint restore before-upgrade"}
            }),
        );
        let work_id = "b".repeat(64);
        let stdout = json!({
            "scope": "project",
            "database": "/PRIVATE_CHECKPOINT_DATABASE/wiki.db",
            "checkpoint": "before-upgrade",
            "path": "/PRIVATE_CHECKPOINT_PATH/before-upgrade.db",
            "safety_checkpoint": "PRIVATE_SAFETY_CHECKPOINT",
            "graph_work": {
                "id": work_id,
                "kind": "graph_projection",
                "scope": "project",
                "database": "/PRIVATE_GRAPH_DATABASE/wiki.db",
                "state": "queued",
                "phase": "queued",
                "completed": 0,
                "total": null,
                "sequence": 1,
                "pid": 4242,
                "message": "PRIVATE_GRAPH_WORK_MESSAGE",
                "result": {"path": "/PRIVATE_GRAPH_RESULT"}
            }
        })
        .to_string();
        let payload = json!({
            "tool_name":"Bash",
            "tool_input":{"command":"lwc checkpoint restore before-upgrade"},
            "tool_response":{"stdout":stdout}
        });

        let receipt = parse_receipt(ToolHost::Claude, &payload, &invocation).unwrap();
        assert_eq!(receipt.identifiers.get("work_id"), Some(&work_id));
        assert_eq!(receipt.state.as_deref(), Some("queued"));
        assert_eq!(receipt.phase.as_deref(), Some("queued"));
        assert_eq!(
            receipt.progress,
            Some(ReceiptProgress {
                completed: 0,
                total: None,
                sequence: Some(1),
            })
        );
        let retained = format!("{receipt:?}");
        for secret in [
            "/PRIVATE_CHECKPOINT_DATABASE/wiki.db",
            "/PRIVATE_CHECKPOINT_PATH/before-upgrade.db",
            "PRIVATE_SAFETY_CHECKPOINT",
            "/PRIVATE_GRAPH_DATABASE/wiki.db",
            "4242",
            "PRIVATE_GRAPH_WORK_MESSAGE",
            "/PRIVATE_GRAPH_RESULT",
        ] {
            assert!(
                !retained.contains(secret),
                "receipt leaked {secret}: {retained}"
            );
        }
    }

    #[test]
    fn graph_work_state_wins_over_outer_graph_state_without_recursive_scanning() {
        let invocation = shell(
            ToolHost::Claude,
            json!({
                "tool_name":"Bash",
                "tool_input":{"command":"lwc checkpoint restore before-upgrade"}
            }),
        );
        let work_id = "c".repeat(64);
        let stdout = json!({
            "status": "restored",
            "graph": {
                "state": "ready",
                "status": "healthy",
                "phase": "idle"
            },
            "graph_work": {
                "id": work_id,
                "state": "running",
                "phase": "projecting",
                "completed": 3,
                "total": 8,
                "sequence": 5
            },
            "untrusted": {
                "graph_work": {
                    "id": "PRIVATE_RECURSIVE_GRAPH_WORK_ID",
                    "state": "failed"
                }
            }
        })
        .to_string();
        let payload = json!({
            "tool_name":"Bash",
            "tool_input":{"command":"lwc checkpoint restore before-upgrade"},
            "tool_response":{"stdout":stdout}
        });

        let receipt = parse_receipt(ToolHost::Claude, &payload, &invocation).unwrap();
        assert_eq!(receipt.identifiers.get("work_id"), Some(&work_id));
        assert_eq!(receipt.state.as_deref(), Some("running"));
        assert_eq!(receipt.phase.as_deref(), Some("projecting"));
        assert_eq!(receipt.status.as_deref(), Some("restored"));
        assert_eq!(
            receipt.progress,
            Some(ReceiptProgress {
                completed: 3,
                total: Some(8),
                sequence: Some(5),
            })
        );
        assert!(!format!("{receipt:?}").contains("PRIVATE_RECURSIVE_GRAPH_WORK_ID"));
    }

    #[test]
    fn typed_count_receipts_fail_open_on_bad_types_failure_and_size() {
        let invocation = shell(
            ToolHost::Claude,
            json!({"tool_name":"Bash","tool_input":{"command":"lwc memory status"}}),
        );
        let malformed = json!({
            "tool_name":"Bash",
            "tool_input":{"command":"lwc memory status"},
            "tool_response":{"stdout":json!({
                "pending_hints":"3",
                "retained":{"events":-1},
                "pressure":{"logical_bytes":1.5,"max_bytes":null}
            }).to_string()}
        });
        assert!(
            parse_receipt(ToolHost::Claude, &malformed, &invocation)
                .unwrap()
                .counts
                .is_empty()
        );

        let forged_error = json!({
            "pending_hints": 999,
            "pressure": {"max_bytes": 1000},
            "error": {
                "code": "memory_pressure",
                "message": "PRIVATE_FORGED_FAILURE_MESSAGE"
            }
        })
        .to_string();
        for tool_response in [
            json!({"exit_code":1,"stdout":forged_error}),
            json!({"success":false,"stdout":forged_error}),
        ] {
            let failed = json!({
                "tool_name":"Bash",
                "tool_input":{"command":"lwc memory status"},
                "tool_response":tool_response
            });
            assert!(parse_receipt(ToolHost::Claude, &failed, &invocation).is_none());
        }

        let oversized = json!({
            "pending_hints": 3,
            "pressure": {"max_bytes": 1000},
            "ignored": "x".repeat(MAX_STDOUT_BYTES)
        })
        .to_string();
        let payload = json!({
            "tool_name":"Bash",
            "tool_input":{"command":"lwc memory status"},
            "tool_response":{"stdout":oversized}
        });
        assert!(parse_receipt(ToolHost::Claude, &payload, &invocation).is_none());
    }

    #[test]
    fn receipt_rejects_non_stdout_multiple_invalid_and_oversized_results() {
        let invocation = shell(
            ToolHost::Claude,
            json!({"tool_name":"Bash","tool_input":{"command":"lwc config show"}}),
        );
        for payload in [
            json!({
                "tool_name":"Bash",
                "tool_input":{"command":"lwc config show"},
                "tool_response":{"stdout":"{} {}"}
            }),
            json!({
                "tool_name":"Bash",
                "tool_input":{"command":"lwc config show"},
                "tool_response":{"stdout":"not json"}
            }),
            json!({
                "tool_name":"Bash",
                "tool_input":{"command":"lwc config show"},
                "tool_response":{"stderr":"{\"status\":\"secret\"}"}
            }),
            json!({
                "tool_name":"Bash",
                "tool_input":{"command":"lwc config show"},
                "tool_response":{"status":"ready"}
            }),
            json!({
                "tool_name":"Bash",
                "tool_input":{"command":"lwc config show"},
                "tool_response":{"exit_code":1,"stdout":"{\"status\":\"ready\"}"}
            }),
            json!({
                "tool_name":"Bash",
                "tool_input":{"command":"lwc config show"},
                "tool_response":{"success":false,"stdout":"{\"status\":\"ready\"}"}
            }),
            json!({
                "tool_name":"Bash",
                "tool_input":{"command":"lwc config show"},
                "tool_response":{"is_error":true,"stdout":"{\"status\":\"ready\"}"}
            }),
        ] {
            assert!(parse_receipt(ToolHost::Claude, &payload, &invocation).is_none());
        }

        let oversized = format!("{{\"message\":\"{}\"}}", "x".repeat(MAX_STDOUT_BYTES));
        let payload = json!({
            "tool_name":"Bash",
            "tool_input":{"command":"lwc config show"},
            "tool_response":{"stdout":oversized}
        });
        assert!(parse_receipt(ToolHost::Claude, &payload, &invocation).is_none());
    }

    #[test]
    fn receipt_does_not_retain_an_untrusted_action_label() {
        let invocation = shell(
            ToolHost::Claude,
            json!({"tool_name":"Bash","tool_input":{"command":"lwc config show"}}),
        );
        let payload = json!({
            "tool_name":"Bash",
            "tool_input":{"command":"lwc config show"},
            "tool_response":{"stdout":"{\"action\":\"PRIVATE_SECRET\",\"status\":\"ready\"}"}
        });

        let receipt = parse_receipt(ToolHost::Claude, &payload, &invocation).unwrap();
        assert_eq!(receipt.action.as_deref(), Some("config.show"));
        assert!(!format!("{receipt:?}").contains("PRIVATE_SECRET"));
    }

    #[test]
    fn receipt_accepts_a_bounded_stable_next_action_code() {
        let invocation = shell(
            ToolHost::Claude,
            json!({"tool_name":"Bash","tool_input":{"command":"lwc sync host /project"}}),
        );
        let payload = json!({
            "tool_name":"Bash",
            "tool_input":{"command":"lwc sync host /project"},
            "tool_response":{"stdout":"{\"next_action\":\"resume_continuity\"}"}
        });

        let receipt = parse_receipt(ToolHost::Claude, &payload, &invocation).unwrap();
        assert_eq!(receipt.next_action.as_deref(), Some("resume_continuity"));
    }

    #[test]
    fn undocumented_or_model_facing_host_results_are_not_receipts() {
        let cli_invocation = shell(
            ToolHost::CopilotCli,
            json!({
                "toolName":"bash",
                "toolArgs":"{\"command\":\"lwc config show\"}"
            }),
        );
        let cli_payload = json!({
            "toolName":"bash",
            "toolArgs":"{\"command\":\"lwc config show\"}",
            "toolResult":{
                "resultType":"success",
                "textResultForLlm":"{\"status\":\"ready\"}"
            }
        });
        assert!(parse_receipt(ToolHost::CopilotCli, &cli_payload, &cli_invocation).is_none());

        let vscode_invocation = shell(
            ToolHost::CopilotVscode,
            json!({"tool_name":"Bash","tool_input":{"command":"lwc config show"}}),
        );
        let vscode_payload = json!({
            "tool_name":"Bash",
            "tool_input":{"command":"lwc config show"},
            "tool_result":{
                "result_type":"success",
                "text_result_for_llm":"{\"status\":\"ready\"}"
            }
        });
        assert!(
            parse_receipt(ToolHost::CopilotVscode, &vscode_payload, &vscode_invocation).is_none()
        );

        let synthetic = RecognizedInvocation {
            action: "config.show".to_owned(),
            transport: InvocationTransport::Shell,
            consent: ConsentClass::Noop,
            consent_advice: None,
        };
        assert!(
            parse_receipt(
                ToolHost::Antigravity,
                &json!({
                    "toolCall":{"name":"run_command","args":{"CommandLine":"lwc config show"}},
                    "toolOutput":{"stdout":"{\"status\":\"ready\"}"}
                }),
                &synthetic,
            )
            .is_none()
        );
        assert!(
            parse_receipt(
                ToolHost::Kiro,
                &json!({
                    "tool_name":"execute_bash",
                    "tool_input":{"command":"lwc config show"},
                    "tool_result":{"stdout":"{\"status\":\"ready\"}"}
                }),
                &synthetic,
            )
            .is_none()
        );
    }
}
