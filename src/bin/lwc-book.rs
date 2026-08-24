fn main() {
    eprintln!(
        "{}",
        r#"{"error":{"code":"plugin_not_implemented","message":"book is not implemented"}}"#
    );
    std::process::exit(1);
}
