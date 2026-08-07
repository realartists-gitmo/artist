fn main() {
    #[cfg(target_os = "linux")]
    if std::env::var_os("CARGO_FEATURE_EMBEDDED_CANVAS").is_some() {
        println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN");
    }
}
