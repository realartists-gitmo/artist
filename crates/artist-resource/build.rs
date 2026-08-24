fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    let dll = match arch.as_str() {
        "x86_64" => "winfsp-x64.dll",
        "x86" => "winfsp-x86.dll",
        "aarch64" => "winfsp-a64.dll",
        _ => panic!("unsupported WinFsp architecture: {arch}"),
    };
    match std::env::var("CARGO_CFG_TARGET_ENV").as_deref() {
        Ok("msvc") => {
            println!("cargo:rustc-link-lib=dylib=delayimp");
            println!("cargo:rustc-link-arg=/DELAYLOAD:{dll}");
        }
        Ok("gnu") if std::env::var("CARGO_CFG_TARGET_ABI").as_deref() == Ok("llvm") => {
            println!("cargo:rustc-link-arg=-Wl,--delayload={dll}");
        }
        environment => panic!("unsupported WinFsp target environment: {environment:?}"),
    }
}
