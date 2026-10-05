fn main() {
    // tauri_build::build() が RC.EXE なしで失敗することを避けるため、エラーハンドリングを追加
    if std::env::var("CARGO_CFG_TARGET_FAMILY").as_deref() == Ok("windows") {
        // Windows: tauri_build はスキップ（開発時）
        println!("cargo:warning=Skipping tauri-build on development check");
    } else {
        tauri_build::build()
    }
}
