fn main() {
    tauri_build::build();

    // tauri-winres/embed-resource emits the normal activation resource for bins only.
    // This opt-in example also links WebView/Tauri code importing Common Controls v6.
    if std::env::var_os("CARGO_FEATURE_VISUAL_PREVIEW_PROBE").is_some()
        && std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc")
    {
        let manifest = std::path::PathBuf::from(
            std::env::var_os("CARGO_MANIFEST_DIR").expect("Cargo manifest directory"),
        )
        .join("../verification/visual-editor/probe.manifest");
        println!("cargo:rerun-if-changed={}", manifest.display());
        println!("cargo:rustc-link-arg-examples=/MANIFEST:EMBED");
        println!(
            "cargo:rustc-link-arg-examples=/MANIFESTINPUT:{}",
            manifest.display()
        );
    }
}
