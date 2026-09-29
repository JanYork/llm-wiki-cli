pub fn main() {
    let cli = Cli::parse();
    let selected_space=if matches!(&cli.command,Command::Agent {command:AgentCommand::Hook {..}} | Command::Cloud {..} | Command::Recovery {..}) {None}else{cli.selected_space.clone().or_else(||if cli.scope==Scope::Project && supports_space(&cli.command){std::env::current_dir().ok().and_then(|cwd|crate::replica::project_binding(&cwd).ok().flatten())}else{None})};
    let full = cli.full || !matches!(&cli.command, Command::Plan { .. } | Command::Remember { .. });
    match run(cli) {
        Ok(Value::Null) => {}
        Ok(value) => {
            let mut value=if full {value}else{compact_receipt(value)};
            if let Some(space)=selected_space.as_deref() {
                if let Ok(Some(signal))=crate::replica::conflict_signal(space)
                    && let Some(object)=value.as_object_mut() {object.insert("signals".into(),json!([signal]));}
                if let Err(error)=crate::replica::start_worker(space)
                    && let Some(object)=value.as_object_mut() {object.insert("sync_warning".into(),json!({"code":error.code}));}
            }
            println!("{}",serde_json::to_string_pretty(&value).unwrap());
        },
        Err(error) => {
            if error.code == "codegraph_exit" {
                std::process::exit(error.details.as_ref().and_then(|d| d["exit_code"].as_i64()).unwrap_or(1) as i32);
            }
            let mut payload = json!({"code": error.code, "message": error.message});
            if let Some(space)=selected_space.as_deref()
                && let Ok(Some(signal))=crate::replica::conflict_signal(space) {payload["signals"]=json!([signal]);}
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
