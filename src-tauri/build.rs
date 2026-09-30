fn main() {
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(
        tauri_build::AppManifest::new().commands(&[
            "get_configuration",
            "save_configuration",
            "connect_saved",
            "connect_fallback",
            "get_stats",
            "report_stats",
        ]),
    ))
    .expect("Tauri build failed");
}
