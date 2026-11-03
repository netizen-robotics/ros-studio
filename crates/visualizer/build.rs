include!("../ui-foundation/build-support/windows-icon.rs");
fn main() {
    embed_app_resources("visualizer", "ROS Studio Visualizer");
    println!("cargo:rerun-if-changed=../network/windows.manifest");
    println!("cargo:rerun-if-env-changed=STUDIO_RELEASE_PUBLIC_KEY");
    println!("cargo:rerun-if-env-changed=STUDIO_WINDOWS_SIGNER_SHA256");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let manifest = std::path::Path::new(&std::env::var("CARGO_MANIFEST_DIR").unwrap())
            .join("../network/windows.manifest")
            .canonicalize()
            .unwrap();
        println!("cargo:rustc-link-arg-bins=/MANIFEST:EMBED");
        println!(
            "cargo:rustc-link-arg-bins=/MANIFESTINPUT:{}",
            manifest.display()
        );
    }
}
