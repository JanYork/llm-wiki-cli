use std::{fs, path::Path};

fn line_count(path: impl AsRef<Path>) -> usize {
    fs::read_to_string(path).unwrap().lines().count()
}

#[test]
fn oversized_modules_are_split_by_responsibility() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));

    for module in ["store", "artifacts", "work"] {
        assert!(
            root.join("src").join(module).join("mod.rs").is_file(),
            "src/{module}.rs must be replaced by src/{module}/mod.rs and cohesive siblings"
        );
    }

    assert!(line_count(root.join("src/main.rs")) <= 120);
    assert!(line_count(root.join("tests/cli.rs")) <= 240);
}
