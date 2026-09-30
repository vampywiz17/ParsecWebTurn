fn main() {
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(
        tauri_build::AppManifest::new().commands(&[
            "show_configuration",
            "get_configuration",
            "save_configuration",
            "connect_saved",
            "connect_fallback",
            "get_stats",
            "report_stats",
            "open_stats",
            "set_parsec_window_mode",
            "parsec_window_shortcut",
        ]),
    ))
    .expect("Tauri build failed");
}
