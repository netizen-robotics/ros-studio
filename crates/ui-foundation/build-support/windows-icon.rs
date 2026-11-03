// Shared MSVC resource compilation. Development requires the Windows SDK rc.exe.
// Runtime users receive embedded icons and version metadata, never compiler tools.
fn embed_app_resources(application: &str, description: &str) {
    println!("cargo:rerun-if-env-changed=RC");
    let root = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let icon = root.join("../ui-foundation/assets/app-icons").join(format!("{application}.ico"));
    println!("cargo:rerun-if-changed={}", icon.display());
    println!("cargo:rerun-if-changed=../ui-foundation/build-support/windows-icon.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") { return; }
    let version = std::env::var("CARGO_PKG_VERSION").unwrap();
    let distribution = if std::env::var_os("CARGO_FEATURE_STORE_DISTRIBUTION").is_some() { "microsoft-store" } else { "bundled" };
    let numeric = version.split('.').take(3).collect::<Vec<_>>().join(",");
    let icon = icon.canonicalize().expect("bundled application icon").display().to_string().replace('\\', "/");
    let output = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let resource = output.join("app-icon.rc");
    let compiled = output.join("app-icon.res");
    std::fs::write(&resource, format!(r#"1 ICON "{icon}"
1 VERSIONINFO
FILEVERSION {numeric},0
PRODUCTVERSION {numeric},0
FILEOS 0x40004L
FILETYPE 0x1L
BEGIN
 BLOCK "StringFileInfo"
 BEGIN
  BLOCK "040904b0"
  BEGIN
   VALUE "Comments", "ROS Studio distribution: {distribution}\0"
   VALUE "CompanyName", "Netizen Robotics\0"
   VALUE "FileDescription", "{description}\0"
   VALUE "FileVersion", "{version}\0"
   VALUE "ProductName", "ROS Studio\0"
   VALUE "ProductVersion", "{version}\0"
   VALUE "LegalCopyright", "Copyright (c) 2026 Netizen Robotics\0"
  END
 END
 BLOCK "VarFileInfo"
 BEGIN
  VALUE "Translation", 0x0409, 1200
 END
END
"#)).expect("write Windows app resources");
    let compiler = std::env::var_os("RC").unwrap_or_else(|| "rc.exe".into());
    let result = std::process::Command::new(compiler).arg("/nologo").arg("/fo").arg(&compiled).arg(&resource).output().expect("Windows SDK rc.exe is required to embed application icons");
    assert!(result.status.success(), "Windows resource compilation failed: {}{}", String::from_utf8_lossy(&result.stdout), String::from_utf8_lossy(&result.stderr));
    println!("cargo:rustc-link-arg-bins={}", compiled.display());
}
