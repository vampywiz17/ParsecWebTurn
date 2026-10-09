fn main() {
    println!("cargo:rerun-if-changed=src/windows_media.cpp");
    if std::env::var_os("CARGO_CFG_WINDOWS").is_some() {
        cc::Build::new()
            .cpp(true)
            .file("src/windows_media.cpp")
            .flag_if_supported("/std:c++17")
            .warnings(true)
            .compile("parsec_windows_media");
    }
}
