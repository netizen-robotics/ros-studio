//! Per-user Windows registration through native registry and shell APIs.
use std::{fs, path::Path};
use studio_network::windows::{verify_signature, wide};
use windows::{
    core::{w, Interface, PCWSTR},
    Win32::{
        System::{Com::*, Registry::*},
        UI::Shell::*,
    },
};
const KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\ROSStudio";
fn programs() -> Result<std::path::PathBuf, String> {
    let path = unsafe { SHGetKnownFolderPath(&FOLDERID_Programs, KF_FLAG_DEFAULT, None) }
        .map_err(|e| e.to_string())?;
    let result = unsafe { path.to_string() }.map_err(|e| e.to_string());
    unsafe {
        CoTaskMemFree(Some(path.0.cast()));
    }
    Ok(std::path::PathBuf::from(result?).join("ROS Studio"))
}
pub fn shortcut(root: &Path, name: &str, executable: &Path) -> Result<(), String> {
    let target = wide(executable);
    let description = wide(name);
    let directory = wide(root);
    unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED)
            .ok()
            .map_err(|e| e.to_string())?;
    }
    let result = (|| {
        let output = programs()?;
        fs::create_dir_all(&output).map_err(|e| e.to_string())?;
        let output = wide(output.join(format!("{name}.lnk")));
        unsafe {
            let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)
                .map_err(|e| e.to_string())?;
            link.SetPath(PCWSTR(target.as_ptr()))
                .map_err(|e| e.to_string())?;
            link.SetIconLocation(PCWSTR(target.as_ptr()), 0)
                .map_err(|e| e.to_string())?;
            link.SetDescription(PCWSTR(description.as_ptr()))
                .map_err(|e| e.to_string())?;
            link.SetWorkingDirectory(PCWSTR(directory.as_ptr()))
                .map_err(|e| e.to_string())?;
            let persist: IPersistFile = link.cast().map_err(|e| e.to_string())?;
            persist
                .Save(PCWSTR(output.as_ptr()), true)
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    })();
    unsafe {
        CoUninitialize();
    }
    result
}
pub fn remove_shortcut(name: &str) -> Result<(), String> {
    let path = programs()?.join(format!("{name}.lnk"));
    if path.exists() {
        fs::remove_file(path).map_err(|e| e.to_string())?;
    }
    Ok(())
}
fn registry(name: &str, value: &str) -> Result<(), String> {
    let key = wide(KEY);
    let name = wide(name);
    let value = wide(value);
    let bytes = unsafe { std::slice::from_raw_parts(value.as_ptr().cast::<u8>(), value.len() * 2) };
    unsafe {
        RegSetKeyValueW(
            HKEY_CURRENT_USER,
            PCWSTR(key.as_ptr()),
            PCWSTR(name.as_ptr()),
            REG_SZ.0,
            Some(bytes.as_ptr().cast()),
            bytes.len() as u32,
        )
    }
    .ok()
    .map_err(|e| e.to_string())
}
pub fn register(root: &Path) -> Result<(), String> {
    registry("DisplayName", "ROS Studio")?;
    registry("DisplayVersion", env!("CARGO_PKG_VERSION"))?;
    registry("Publisher", "Netizen Robotics")?;
    registry("InstallLocation", &root.to_string_lossy())?;
    registry(
        "DisplayIcon",
        &root.join("studio-launcher.exe").to_string_lossy(),
    )?;
    registry(
        "UninstallString",
        &format!(
            "\"{}\" --uninstall",
            root.join("studio-bootstrap.exe").display()
        ),
    )?;
    shortcut(root, "ROS Studio", &root.join("studio-launcher.exe"))
}
pub fn unregister() -> Result<(), String> {
    let key = wide(KEY);
    let result = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, PCWSTR(key.as_ptr())) };
    if result.0 != 0 && result.0 != 2 {
        return Err(format!("Registry operation failed: {}", result.0));
    }
    remove_shortcut("ROS Studio Launcher")?;
    remove_shortcut("ROS Studio")
}
/// Bootstrap copies only its two fixed signed roles. An app lease excludes live Launcher updates.
pub fn install() -> Result<(), String> {
    let root = crate::install::installed_root()?;
    let state = studio_app_lifecycle::state_directory()?;
    let _lease = studio_app_lifecycle::AppLease::acquire(&state, "launcher")?;
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let source = exe.parent().ok_or("Bootstrap directory missing")?;
    if source == root {
        return Err(
            "Run the new signed bootstrap from its extracted package to update Launcher".into(),
        );
    }
    for name in ["studio-bootstrap.exe", "studio-launcher.exe"] {
        verify_signature(&source.join(name))?;
    }
    crate::catalog::embedded_key()?;
    fs::create_dir_all(&root).map_err(|e| e.to_string())?;
    let stage = root.join(".bootstrap-staging");
    if stage.exists() {
        fs::remove_dir_all(&stage).map_err(|e| e.to_string())?;
    }
    fs::create_dir(&stage).map_err(|e| e.to_string())?;
    for name in ["studio-bootstrap.exe", "studio-launcher.exe"] {
        fs::copy(source.join(name), stage.join(name)).map_err(|e| e.to_string())?;
        verify_signature(&stage.join(name))?;
    }
    let mut replaced = Vec::new();
    let result = (|| {
        for name in ["studio-bootstrap.exe", "studio-launcher.exe"] {
            let target = root.join(name);
            let backup = root.join(format!(".{name}.previous"));
            if backup.exists() {
                fs::remove_file(&backup).map_err(|e| e.to_string())?;
            }
            if target.exists() {
                verify_signature(&target)?;
                fs::rename(&target, &backup).map_err(|e| e.to_string())?;
            }
            replaced.push((target.clone(), backup));
            fs::rename(stage.join(name), target).map_err(|e| e.to_string())?;
        }
        register(&root)
    })();
    if result.is_err() {
        for (target, backup) in replaced.iter().rev() {
            let _ = fs::remove_file(target);
            if backup.exists() {
                let _ = fs::rename(backup, target);
            }
        }
    }
    let _ = fs::remove_dir_all(stage);
    result
}
pub fn confirm(text: &str) -> bool {
    let text = wide(text);
    unsafe {
        windows::Win32::UI::WindowsAndMessaging::MessageBoxW(
            None,
            PCWSTR(text.as_ptr()),
            w!("ROS Studio installation"),
            windows::Win32::UI::WindowsAndMessaging::MB_YESNO
                | windows::Win32::UI::WindowsAndMessaging::MB_DEFBUTTON2,
        ) == windows::Win32::UI::WindowsAndMessaging::IDYES
    }
}

/// Explicit optional setup after an authenticated app installation. Denial does
/// not roll back the usable application and never changes the network category.
pub fn offer_network(
    app: crate::catalog::Application,
    cancel: &std::sync::atomic::AtomicBool,
) -> String {
    if cancel.load(std::sync::atomic::Ordering::Acquire) || !confirm("The application is installed. Set up LAN reception now? Proposed access: backend inbound UDP, selected LAN adapter, Private networks and local subnet. Administrator approval is optional. You can also use Check/repair network access in the application later.") {return "Network access setup skipped; LAN reception may be blocked. Use Network settings to review it later.".into();}
    let result = (|| {
        let preferences =
            studio_network::preferences::load(&studio_network::preferences::path(app.id())?)?;
        let selection = if matches!(
            preferences.selection,
            studio_network::Selection::Local | studio_network::Selection::Custom { .. }
        ) {
            studio_network::Selection::Lan
        } else {
            preferences.selection
        };
        let (adapter_id, _) =
            studio_network::adapters::resolve(&selection, &studio_network::adapters::enumerate()?)?
                .ok_or("Choose a LAN adapter in the application")?;
        studio_network::windows::request(
            &studio_network::firewall::Request {
                application: if app == crate::catalog::Application::Tools {
                    studio_network::firewall::Application::Tools
                } else {
                    studio_network::firewall::Application::Visualizer
                },
                operation: studio_network::firewall::Operation::Repair,
                adapter_id,
                scope: studio_network::firewall::Scope::Private,
            },
            cancel,
        )
    })();
    result.unwrap_or_else(|error|format!("Application installed; network setup remains incomplete: {error}. Open Network settings to select an adapter and retry."))
}
