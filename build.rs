use sha2::{Digest, Sha256};
use std::{env, fs, path::PathBuf};

fn main() {
    println!("cargo:rustc-check-cfg=cfg(has_embedded_graphqlite)");
    println!("cargo:rerun-if-changed=vendor/graphqlite/0.6.0");
    let os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    let artifact = match (os.as_str(), arch.as_str()) {
        ("macos", "x86_64") => Some((
            "graphqlite-macos-x86_64.dylib",
            "16e9b33af612a1c6e01c6e56087a84d7b072378132cf8f7c7b95635c464e2b3d",
        )),
        ("macos", "aarch64") => Some((
            "graphqlite-macos-aarch64.dylib",
            "1cbf6bf109aa064b2732e327592466f1dfdf9cea101ef979e3bd03e14a8082ee",
        )),
        ("linux", "x86_64") => Some((
            "graphqlite-linux-x86_64.so",
            "28faf6f5615ac6ad19393e69ec23cdec1cf1ca1179beb4b9a9bbad6117f888f7",
        )),
        ("linux", "aarch64") => Some((
            "graphqlite-linux-aarch64.so",
            "a3c40f3d985faeade63b0664e05c691632b7cc2638eba03534482cd3eea451a5",
        )),
        _ => None,
    };
    if let Some((file, expected)) = artifact {
        let path = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap())
            .join("vendor/graphqlite/0.6.0")
            .join(file);
        let actual = Sha256::digest(fs::read(&path).expect("read pinned GraphQLite runtime"))
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        assert_eq!(
            actual, expected,
            "pinned GraphQLite runtime checksum mismatch"
        );
        println!("cargo:rustc-cfg=has_embedded_graphqlite");
        println!("cargo:rustc-env=LWC_GRAPHQLITE_EMBEDDED={}", path.display());
        println!("cargo:rustc-env=LWC_GRAPHQLITE_FILENAME={file}");
    }
}
