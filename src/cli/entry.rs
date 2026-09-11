pub fn main() {
    let cli = Cli::parse();
    let full = cli.full || !matches!(&cli.command, Command::Plan { .. } | Command::Remember { .. });
    match run(cli) {
        Ok(Value::Null) => {}
        Ok(value) => println!("{}", serde_json::to_string_pretty(&if full { value } else { compact_receipt(value) }).unwrap()),
        Err(error) => {
            if error.code == "codegraph_exit" {
                std::process::exit(error.details.as_ref().and_then(|d| d["exit_code"].as_i64()).unwrap_or(1) as i32);
            }
            let mut payload = json!({"code": error.code, "message": error.message});
            if let Some(details) = error.details {
                payload["details"] = details;
            }
            eprintln!(
                "{}",
                serde_json::to_string(&json!({"error": payload})).unwrap()
            );
            std::process::exit(1);
        }
    }
}

fn compact_receipt(mut value: Value) -> Value {
    if value.get("action").is_some() && value["plan"].is_object() {
        let revision = value["plan"]["revision"].clone();
        let id = value["plan"]["id"].as_str().unwrap_or("").to_owned();
        if let Some(steps) = value["plan"]["steps"].as_array_mut() { steps.retain(|s|s["updated_revision"] == revision); }
        value["read"] = json!(format!("lwc plan show {id}"));
    }
    if value.get("created").is_some() && value["event"].is_object() {
        let id = value["event"]["id"].as_str().unwrap_or("").to_owned();
        value["event"].as_object_mut().unwrap().retain(|key,_|matches!(key.as_str(),"id"|"type"|"context"|"occurred_at"|"recorded_at"));
        value.as_object_mut().unwrap().remove("pressure");
        value.as_object_mut().unwrap().remove("database");
        value["read"] = json!(format!("lwc memory show {id}"));
    }
    if let Some(plans) = value.get_mut("plans").and_then(Value::as_array_mut) {
        for plan in plans { if let Some(fields)=plan.as_object_mut(){ fields.retain(|key,_|matches!(key.as_str(),"id"|"title"|"state"|"revision"|"updated_at")); } }
        value["binding"] = json!("unbound; use --context from the active Hook to resolve ownership");
    }
    value
}
