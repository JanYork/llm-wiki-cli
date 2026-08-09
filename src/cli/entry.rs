pub fn main() {
    match run(Cli::parse()) {
        Ok(value) => println!("{}", serde_json::to_string_pretty(&value).unwrap()),
        Err(error) => {
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
