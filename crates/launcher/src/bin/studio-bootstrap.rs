#![cfg_attr(windows, windows_subsystem = "windows")]
#[cfg(windows)]
fn main() {
    use studio_launcher::{
        catalog::Application, firewall_helper::message, install, windows_setup as setup,
    };
    if std::env::args().skip(1).eq(["--release-configuration"]) {
        println!(
            "{}",
            serde_json::json!({"publisher_sha256":option_env!("STUDIO_WINDOWS_SIGNER_SHA256"),"catalog_key":option_env!("STUDIO_RELEASE_PUBLIC_KEY")})
        );
        return;
    }
    let result: Result<(), String> = (|| {
        let args: Vec<_> = std::env::args().skip(1).collect();
        if args.as_slice() == ["--uninstall"] {
            if !setup::confirm("Uninstall ROS Studio? Close Tools, Visualizer and Launcher first. Installer-owned firewall rules require administrator approval for removal. User workspaces and preferences are retained."){return Ok(());}
            let root = install::installed_root()?;
            let state = studio_app_lifecycle::state_directory()?;
            let _lease = studio_app_lifecycle::AppLease::acquire(&state, "launcher")?;
            for app in [Application::Tools, Application::Visualizer] {
                install::uninstall(&root, &state, app)?;
            }
            setup::unregister()?;
            // The registered uninstaller cannot erase its mapped image; preserve it with user settings.
            let backup = state.join("studio-bootstrap-uninstalled.exe");
            if backup.exists() {
                std::fs::remove_file(&backup).map_err(|e| e.to_string())?;
            }
            let exe = root.join("studio-bootstrap.exe");
            if exe.exists() {
                std::fs::rename(exe, backup).map_err(|e| e.to_string())?;
            }
            for name in [
                "studio-launcher.exe",
                ".studio-launcher.exe.previous",
                ".studio-bootstrap.exe.previous",
            ] {
                let file = root.join(name);
                if file.exists() {
                    studio_network::windows::verify_signature(&file)?;
                    std::fs::remove_file(file).map_err(|e| e.to_string())?;
                }
            }
            let _ = std::fs::remove_dir(&root);
            message("ROS Studio uninstalled. Your workspaces, preferences and retained uninstaller remain in your user settings directory.",false);
            Ok(())
        } else if args.is_empty() {
            if !setup::confirm("Install ROS Studio for your Windows user? Launcher will install into Local App Data and add Start menu shortcuts. Tools and Visualizer packages are downloaded only after you choose Install in Launcher."){return Ok(());}
            setup::install()?;
            message("Launcher installed. Open ROS Studio Launcher from the Start menu to install Tools and Visualizer. Network access setup is offered after application installation and in Network settings.",false);
            Ok(())
        } else {
            Err("Unsupported bootstrap option".into())
        }
    })();
    if let Err(error) = result {
        message(&error, true);
        std::process::exit(1);
    }
}
#[cfg(not(windows))]
fn main() {
    eprintln!("Windows-only bootstrap");
    std::process::exit(1);
}
