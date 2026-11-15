fn main() {
    // candle_engine: the pure-Rust embedding backend replaces ONNX Runtime
    // (Intel macOS has no ONNX Runtime build; the `candle-only` feature tries it anywhere)
    println!("cargo:rustc-check-cfg=cfg(candle_engine)");
    let os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    if (os == "macos" && arch == "x86_64") || std::env::var("CARGO_FEATURE_CANDLE_ONLY").is_ok() {
        println!("cargo:rustc-cfg=candle_engine");
    }
    tauri_build::build()
}
