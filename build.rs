fn main() {
    // The full clap command tree exceeds the Windows linker default 1 MiB stack
    // in debug builds, before even `lwc --help` can finish parsing.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let argument = if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
            "/STACK:8388608"
        } else {
            "-Wl,--stack,8388608"
        };
        println!("cargo:rustc-link-arg-bin=lwc={argument}");
    }
}
