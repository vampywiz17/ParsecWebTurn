use std::{env, fs, path::PathBuf, process::Command};
fn main() {
    println!("cargo:rerun-if-changed=../assets/parsec.ico");
    println!("cargo:rerun-if-changed=vendor/parsecd.wasm");
    if !env::var("TARGET")
        .unwrap_or_default()
        .contains("windows-msvc")
    {
        return;
    }
    let output = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let icon = fs::canonicalize("../assets/parsec.ico").expect("Application icon missing");
    let icon = icon
        .display()
        .to_string()
        .trim_start_matches(r"\\?\")
        .replace('\\', "\\\\");
    let version = env::var("CARGO_PKG_VERSION").unwrap();
    let tuple = version.replace('.', ",");
    let resource = format!(
        r#"1 ICON "{icon}"
1 VERSIONINFO
FILEVERSION {tuple},0
PRODUCTVERSION {tuple},0
FILETYPE 1
BEGIN
 BLOCK "StringFileInfo"
 BEGIN
  BLOCK "040904B0"
  BEGIN
   VALUE "FileDescription", "ParsecWebTurn native client\0"
   VALUE "ProductName", "ParsecWebTurn\0"
   VALUE "FileVersion", "{version}\0"
   VALUE "ProductVersion", "{version}\0"
  END
 END
 BLOCK "VarFileInfo"
 BEGIN
  VALUE "Translation", 0x0409, 1200
 END
END
"#
    );
    let source = output.join("application.rc");
    let compiled = output.join("application.res");
    fs::write(&source, resource).unwrap();
    let compiler = env::var_os("RC").map(PathBuf::from).unwrap_or_else(|| {
        let root =
            PathBuf::from(env::var_os("ProgramFiles(x86)").expect("Windows SDK location missing"))
                .join("Windows Kits/10/bin");
        let mut candidates: Vec<_> = fs::read_dir(root)
            .expect("Install the Windows SDK resource compiler")
            .filter_map(Result::ok)
            .map(|entry| entry.path().join("x64/rc.exe"))
            .filter(|path| path.is_file())
            .collect();
        candidates.sort();
        candidates.pop().expect("Windows SDK rc.exe missing")
    });
    assert!(
        Command::new(compiler)
            .arg("/nologo")
            .arg("/fo")
            .arg(&compiled)
            .arg(source)
            .status()
            .expect("Resource compiler failed")
            .success(),
        "Windows resources failed"
    );
    println!("cargo:rustc-link-arg={}", compiled.display());
}
