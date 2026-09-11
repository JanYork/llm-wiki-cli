//! Input contracts are shared by discovery and prevalidation; storage still checks invariants.
use crate::error::{AppError, Result};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

fn object(properties: Value, required: &[&str]) -> Value {
    json!({"type":"object", "properties":properties, "required":required, "additionalProperties":false})
}
fn array(items: Value) -> Value {
    json!({"type":"array", "items":items})
}
fn text() -> Value {
    json!({"type":"string", "minLength":1})
}
fn optional_text() -> Value {
    json!({"type":["string","null"]})
}

pub fn describe(name: &str) -> Result<Value> {
    let step = object(json!({"title":text(),"verify":optional_text()}), &["title"]);
    let (schema, example) = match name {
        "discussion" => (
            object(
                json!({"id":text(),"context":text(),"request_id":text(),"if_revision":{"type":"integer","minimum":0},"from_context":optional_text(),"operations":array(object(json!({
                "op":{"type":"string","enum":["start","question","answer","option","gap","reply","summary","revise","withdraw","restore","move","confirm","delivery","metadata","pause","resume","close"]},
                "id":optional_text(),"parent":optional_text(),"text":optional_text(),"description":optional_text(),"reason":optional_text(),"refs":array(text()),"ordinal":{"type":["integer","null"]},"confirmed":{"type":["boolean","null"]},"message_id":optional_text()
            }), &["op"]))}),
                &["id", "context", "request_id", "if_revision", "operations"],
            ),
            json!({"id":"discussion-1","context":format!("lwcctx-v1-{}","0".repeat(64)),"request_id":"turn-1","if_revision":0,"operations":[{"op":"start","text":"Design clarification","description":"Agree on requirements"},{"op":"question","id":"q1","text":"What must this change achieve?"}]}),
        ),
        "remember" => {
            let schema = object(
                json!({
                    "type":text(), "context":text(), "request_id":optional_text(), "occurred_at":optional_text(),
                    "valid_from":optional_text(), "valid_to":optional_text(), "pinned":{"type":"boolean"},
                    "observed":array(text()), "decision":array(text()), "constraints":array(text()),
                    "learned":array(text()), "unresolved":array(text()), "outcome":array(text()),
                    "changes":array(object(json!({"subject":text(), "before":optional_text(), "after":optional_text(), "reason":optional_text()}), &["subject"])),
                    "evidence":array(object(json!({"reference":text(), "excerpt":optional_text()}), &["reference"])),
                    "relations":array(object(json!({"type":{"type":"string","enum":["supersedes","contradicts","resolves","supports","related"]}, "target":text(), "basis":optional_text()}), &["type","target"]))
                }),
                &["type", "context"],
            );
            (
                schema,
                json!({"type":"decision", "context":"CG routing", "decision":["Use the selected checkout index"], "evidence":[{"reference":"src/codegraph/mod.rs"}]}),
            )
        }
        "plan-create" => (
            object(
                json!({"title":text(),"objective":text(),"done_when":text(),"tags":array(text()),"constraints":array(text()),"steps":array(step),"request_id":optional_text()}),
                &["title", "objective", "done_when", "steps"],
            ),
            json!({"title":"CG delivery","objective":"Preserve native output","done_when":"Passthrough checks pass","steps":[{"title":"Implement","verify":"Run focused tests"}]}),
        ),
        "plan-revise" => (
            object(
                json!({"title":optional_text(),"objective":optional_text(),"done_when":optional_text(),"constraints":{"type":["array","null"],"items":text()},"steps":array(step),"focal":{"type":["integer","null"]},"current_step":optional_text(),"updates":array(object(json!({"id":text(),"title":optional_text(),"verify":optional_text(),"disposition":{"type":["string","null"],"enum":[null,"waived","superseded"]},"basis":optional_text()}), &["id"]))}),
                &[],
            ),
            json!({"objective":"Verify correctness without a load test","done_when":"Focused correctness tests pass"}),
        ),
        _ => {
            return Err(AppError::new(
                "unknown_contract",
                "use remember, plan-create, or plan-revise",
            ));
        }
    };
    Ok(
        json!({"name":name,"schema":schema,"example":example,"semantic_validation":"Storage also validates timestamps, nonempty content, revision and lifecycle invariants."}),
    )
}

pub fn parse<T: DeserializeOwned>(name: &str, raw: &str) -> Result<T> {
    let contract = describe(name)?;
    let value: Value = serde_json::from_str(raw).map_err(|error| {
        AppError::new("invalid_input", error.to_string()).with_details(
            json!({"contract":format!("lwc contract {name}"),"example":contract["example"]}),
        )
    })?;
    let mut errors = Vec::new();
    validate(&contract["schema"], &value, "$", &mut errors);
    if !errors.is_empty() {
        return Err(AppError::new("invalid_input", "input does not match the command contract").with_details(json!({"errors":errors,"contract":format!("lwc contract {name}"),"example":contract["example"]})));
    }
    serde_json::from_value(value).map_err(|error| {
        AppError::new("invalid_input", error.to_string()).with_details(
            json!({"contract":format!("lwc contract {name}"),"example":contract["example"]}),
        )
    })
}

// Deliberately validates the vocabulary emitted above, not arbitrary third-party JSON Schema.
fn validate(schema: &Value, value: &Value, path: &str, errors: &mut Vec<Value>) {
    let matches_type = |kind: &str| match kind {
        "object" => value.is_object(),
        "array" => value.is_array(),
        "string" => value.is_string(),
        "boolean" => value.is_boolean(),
        "integer" => value.is_i64() || value.is_u64(),
        "null" => value.is_null(),
        _ => false,
    };
    let valid = schema["type"]
        .as_str()
        .map(matches_type)
        .unwrap_or_else(|| {
            schema["type"]
                .as_array()
                .is_some_and(|types| types.iter().any(|t| t.as_str().is_some_and(matches_type)))
        });
    if !valid {
        errors.push(json!({"path":path,"expected":schema["type"],"fix":"Use the declared JSON type; see the example."}));
        return;
    }
    if schema["enum"]
        .as_array()
        .is_some_and(|values| !values.contains(value))
    {
        errors
            .push(json!({"path":path,"expected":schema["enum"],"fix":"Choose one listed value."}));
    }
    if schema["minLength"] == 1 && value.as_str().is_some_and(|v| v.trim().is_empty()) {
        errors.push(
            json!({"path":path,"expected":"nonempty string","fix":"Provide meaningful text."}),
        );
    }
    if let Some(fields) = value.as_object() {
        for key in schema["required"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            if !fields.contains_key(key) {
                errors.push(json!({"path":format!("{path}.{key}"),"expected":"required field","fix":"Add this field."}));
            }
        }
        for (key, child) in fields {
            if let Some(rule) = schema["properties"].get(key) {
                validate(rule, child, &format!("{path}.{key}"), errors);
            } else {
                errors.push(json!({"path":format!("{path}.{key}"),"expected":"declared field","fix":"Remove the unknown field; inspect the contract."}));
            }
        }
    }
    if let Some(items) = value.as_array() {
        for (i, item) in items.iter().enumerate() {
            validate(&schema["items"], item, &format!("{path}[{i}]"), errors);
        }
    }
}
