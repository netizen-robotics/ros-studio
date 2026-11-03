//! Staged installation. A running-app lease also serializes installers.
use crate::catalog::{Application, Release};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};
use studio_app_lifecycle::AppLease;
pub const MAX_EXPANDED: u64 = 4 * 1024 * 1024 * 1024;
pub fn canceled(cancel: &AtomicBool) -> Result<(), String> {
    if cancel.load(Ordering::Acquire) {
        Err("Operation canceled; installed application retained".into())
    } else {
        Ok(())
    }
}
pub fn installed_root() -> Result<PathBuf, String> {
    #[cfg(windows)]
    return Ok(PathBuf::from(
        std::env::var_os("LOCALAPPDATA").ok_or("Local application directory unavailable")?,
    )
    .join("Programs/ROS Studio"));
    #[cfg(not(windows))]
    Ok(
        PathBuf::from(std::env::var_os("HOME").ok_or("Home directory unavailable")?)
            .join("Applications/ROS Studio"),
    )
}
pub fn verify_archive(path: &Path, release: &Release, cancel: &AtomicBool) -> Result<(), String> {
    let file = File::open(path).map_err(|e| e.to_string())?;
    if file.metadata().map_err(|e| e.to_string())?.len() != release.archive_bytes {
        return Err("Archive size does not match catalog".into());
    }
    let mut hash = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    let mut reader = file;
    loop {
        canceled(cancel)?;
        let n = reader.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        hash.update(&buf[..n]);
    }
    if hex::encode(hash.finalize()) != release.sha256.to_lowercase() {
        return Err("Archive SHA-256 does not match catalog".into());
    }
    Ok(())
}
pub fn extract(
    archive: &Path,
    destination: &Path,
    app: Application,
    cancel: &AtomicBool,
) -> Result<PathBuf, String> {
    fs::create_dir(destination).map_err(|e| e.to_string())?;
    let gzip = flate2::read::GzDecoder::new(File::open(archive).map_err(|e| e.to_string())?);
    let mut tar = tar::Archive::new(gzip);
    let mut total = 0u64;
    let mut count = 0;
    let root = directory_name(app);
    let mut library_alias = false;
    for entry in tar.entries().map_err(|e| e.to_string())? {
        canceled(cancel)?;
        count += 1;
        if count > 50_000 {
            return Err("Archive contains too many entries".into());
        }
        let mut entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path().map_err(|e| e.to_string())?.into_owned();
        if path
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
            || path.components().next().map(|c| c.as_os_str()) != Some(std::ffi::OsStr::new(&root))
        {
            return Err("Archive path escapes application bundle".into());
        }
        #[cfg(windows)]
        if !crate::windows_install::safe_relative(path.to_str().ok_or("Non-Unicode archive path")?)
        {
            return Err("Invalid Windows archive path".into());
        }
        let kind = entry.header().entry_type();
        if kind.is_symlink() {
            if cfg!(windows) {
                return Err("Windows runtime links are unsupported".into());
            }
            if path != Path::new(&root).join("Contents/Resources/lib")
                || entry.link_name().map_err(|e| e.to_string())?.as_deref()
                    != Some(Path::new("../Frameworks"))
                || library_alias
            {
                return Err(
                    "Archive links must be the single contained Resources/lib alias".into(),
                );
            }
            library_alias = true;
            continue;
        }
        if !kind.is_dir() && !kind.is_file() {
            return Err(
                "Archive links and special files are unsupported; publish a dereferenced bundle"
                    .into(),
            );
        }
        total = total
            .checked_add(entry.size())
            .ok_or("Expanded archive size overflow")?;
        if total > MAX_EXPANDED {
            return Err("Expanded archive exceeds 4 GiB".into());
        }
        let output = destination.join(path);
        if kind.is_dir() {
            fs::create_dir_all(&output).map_err(|e| e.to_string())?;
            continue;
        }
        fs::create_dir_all(output.parent().unwrap()).map_err(|e| e.to_string())?;
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&output)
            .map_err(|e| e.to_string())?;
        let mut buf = [0u8; 64 * 1024];
        loop {
            canceled(cancel)?;
            let n = entry.read(&mut buf).map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            file.write_all(&buf[..n]).map_err(|e| e.to_string())?;
        }
        file.sync_all().map_err(|e| e.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = entry.header().mode().map_err(|e| e.to_string())?;
            fs::set_permissions(
                &output,
                fs::Permissions::from_mode(if mode & 0o111 != 0 { 0o755 } else { 0o644 }),
            )
            .map_err(|e| e.to_string())?;
        }
    }
    let bundle = destination.join(root);
    if library_alias {
        if !bundle.join("Contents/Frameworks").is_dir()
            || bundle.join("Contents/Resources/lib").exists()
        {
            return Err("Invalid bundled library alias".into());
        }
        #[cfg(unix)]
        std::os::unix::fs::symlink("../Frameworks", bundle.join("Contents/Resources/lib"))
            .map_err(|e| e.to_string())?;
        #[cfg(not(unix))]
        return Err("Bundle aliases require macOS".into());
    }
    if !cfg!(windows) && !bundle.join("Contents/Info.plist").is_file() {
        return Err("Archive is missing application Info.plist".into());
    }
    Ok(bundle)
}
#[cfg(not(windows))]
fn plist(bundle: &Path, key: &str) -> Result<String, String> {
    if fs::metadata(bundle.join("Contents/Info.plist"))
        .map_err(|e| e.to_string())?
        .len()
        > 65536
    {
        return Err("Application Info.plist exceeds 64 KiB".into());
    }
    let out = std::process::Command::new("/usr/libexec/PlistBuddy")
        .arg("-c")
        .arg(format!("Print :{key}"))
        .arg(bundle.join("Contents/Info.plist"))
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(format!("Missing application {key}"));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}
pub fn directory_name(app: Application) -> String {
    if cfg!(windows) {
        app.id().to_owned()
    } else {
        app.bundle_name()
    }
}
pub fn installed_version(root: &Path, app: Application) -> Result<Option<String>, String> {
    let bundle = root.join(directory_name(app));
    if !bundle.exists() {
        return Ok(None);
    }
    #[cfg(windows)]
    {
        let release = crate::windows_install::cached_release(&bundle, app)?;
        crate::windows_install::validate_files(&bundle, &release)?;
        Ok(Some(release.version))
    }
    #[cfg(not(windows))]
    if plist(&bundle, "CFBundleIdentifier")? != app.bundle_id() {
        return Err("Installed bundle identity is unexpected; refusing to manage it".into());
    }
    #[cfg(not(windows))]
    Ok(Some(plist(&bundle, "CFBundleShortVersionString")?))
}
#[cfg(not(windows))]
fn bounded_manifest(path: &Path) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    File::open(path)
        .map_err(|e| e.to_string())?
        .take(65537)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > 65536 {
        return Err("Runtime manifest exceeds 64 KiB".into());
    }
    Ok(bytes)
}
pub fn validate_bundle(bundle: &Path, release: &Release) -> Result<(), String> {
    #[cfg(windows)]
    {
        crate::windows_install::validate_files(bundle, release)?;
        for name in [
            format!("studio-{}.exe", release.application.id()),
            "studio-backend.exe".into(),
            "studio-network-helper.exe".into(),
        ] {
            studio_network::windows::verify_signature(&bundle.join(name))?;
        }
        Ok(())
    }
    #[cfg(not(windows))]
    validate_macos_bundle(bundle, release)
}
#[cfg(not(windows))]
fn validate_macos_bundle(bundle: &Path, release: &Release) -> Result<(), String> {
    if !cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        return Err("Installation currently supports macOS ARM64 only".into());
    }
    if plist(bundle, "CFBundleIdentifier")? != release.bundle_id
        || plist(bundle, "CFBundleShortVersionString")? != release.version
        || plist(bundle, "CFBundleExecutable")? != format!("studio-{}", release.application.id())
    {
        return Err("Application identity/version does not match catalog".into());
    }
    let runtime: serde_json::Value = serde_json::from_slice(&bounded_manifest(
        &bundle.join("Contents/Resources/manifest.json"),
    )?)
    .map_err(|e| e.to_string())?;
    if runtime["distro"] != "jazzy" {
        return Err("Bundle does not contain the declared Jazzy runtime".into());
    }
    for (program, args) in [
        ("/usr/bin/codesign", vec!["--verify", "--deep", "--strict"]),
        ("/usr/sbin/spctl", vec!["--assess", "--type", "execute"]),
    ] {
        let status = std::process::Command::new(program)
            .args(args)
            .arg(bundle)
            .output()
            .map_err(|e| e.to_string())?;
        if !status.status.success() {
            return Err(format!(
                "Application verification failed: {program}: {}",
                String::from_utf8_lossy(&status.stderr)
                    .chars()
                    .take(1024)
                    .collect::<String>()
            ));
        }
    }
    Ok(())
}
fn backup(root: &Path, app: Application) -> PathBuf {
    root.join(if cfg!(windows) {
        format!(".{}-previous", app.id())
    } else {
        format!(".{}-previous.app", app.id())
    })
}
/// Recovery is only called while holding the application's exclusive lease.
pub fn recover(root: &Path, app: Application) -> Result<(), String> {
    let target = root.join(directory_name(app));
    let previous = backup(root, app);
    if !target.exists() && previous.exists() {
        fs::rename(previous, target).map_err(|e| e.to_string())?;
    }
    Ok(())
}
pub fn replace(
    root: &Path,
    staged: &Path,
    app: Application,
    cancel: &AtomicBool,
) -> Result<(), String> {
    canceled(cancel)?;
    let target = root.join(directory_name(app));
    let previous = backup(root, app);
    if target.exists() {
        if previous.exists() {
            fs::remove_dir_all(&previous).map_err(|e| e.to_string())?;
        }
        fs::rename(&target, &previous).map_err(|e| e.to_string())?;
    }
    if let Err(error) = fs::rename(staged, &target) {
        if previous.exists() {
            fs::rename(&previous, &target).map_err(|rollback| {
                format!("Install failed: {error}; recovery required: {rollback}")
            })?;
        }
        return Err(error.to_string());
    }
    Ok(())
}
pub fn install(
    root: &Path,
    state: &Path,
    archive: &Path,
    release: &Release,
    cancel: &AtomicBool,
) -> Result<(), String> {
    if cfg!(windows) {
        return Err("Windows installation requires the verified catalog envelope".into());
    }
    install_using(root, state, archive, release, cancel, validate_bundle)
}
pub fn install_authenticated(
    root: &Path,
    state: &Path,
    archive: &Path,
    release: &Release,
    envelope: &[u8],
    cancel: &AtomicBool,
) -> Result<(), String> {
    let catalog = crate::catalog::verify(envelope, &crate::catalog::embedded_key()?)?;
    let authenticated = catalog
        .releases
        .iter()
        .any(|r| serde_json::to_value(r).ok() == serde_json::to_value(release).ok());
    if !authenticated || !release.for_host() {
        return Err("Release does not match the authenticated host catalog".into());
    }
    install_using(root, state, archive, release, cancel, |bundle, release| {
        validate_bundle(bundle, release)?;
        #[cfg(windows)]
        fs::write(bundle.join(".release-catalog.json"), envelope).map_err(|e| e.to_string())?;
        Ok(())
    })
}
fn install_using(
    root: &Path,
    state: &Path,
    archive: &Path,
    release: &Release,
    cancel: &AtomicBool,
    validate: impl FnOnce(&Path, &Release) -> Result<(), String>,
) -> Result<(), String> {
    release.validate()?;
    let _lease = AppLease::acquire(state, release.application.id())?;
    fs::create_dir_all(root).map_err(|e| e.to_string())?;
    recover(root, release.application)?;
    if let Some(current) = installed_version(root, release.application)? {
        if semver::Version::parse(&release.version).map_err(|e| e.to_string())?
            <= semver::Version::parse(&current).map_err(|e| e.to_string())?
        {
            return Err("Install requires a newer application version".into());
        }
    }
    let stage = root.join(format!(".{}-staging", release.application.id()));
    if stage.exists() {
        fs::remove_dir_all(&stage).map_err(|e| e.to_string())?;
    }
    verify_archive(archive, release, cancel)?;
    let result = (|| {
        let bundle = extract(archive, &stage, release.application, cancel)?;
        validate(&bundle, release)?;
        canceled(cancel)?;
        replace(root, &bundle, release.application, cancel)?;
        #[cfg(windows)]
        crate::windows_setup::shortcut(
            root,
            release.application.name(),
            &root
                .join(directory_name(release.application))
                .join(format!("studio-{}.exe", release.application.id())),
        )?;
        Ok(())
    })();
    let _ = fs::remove_dir_all(stage);
    result
}
pub fn open(root: &Path, state: &Path, app: Application) -> Result<(), String> {
    {
        let _lease = match AppLease::acquire(state, app.id()) {
            Ok(lease) => {
                recover(root, app)?;
                lease
            }
            Err(_) => AppLease::running(state, app.id())?,
        };
        if installed_version(root, app)?.is_none() {
            return Err("Install this application first".into());
        }
    }
    #[cfg(windows)]
    {
        let bundle = root.join(directory_name(app));
        let release = crate::windows_install::cached_release(&bundle, app)?;
        validate_bundle(&bundle, &release)?;
        std::process::Command::new(bundle.join(format!("studio-{}.exe", app.id())))
            .spawn()
            .map_err(|e| e.to_string())?;
        Ok(())
    }
    #[cfg(not(windows))]
    let status = std::process::Command::new("/usr/bin/open")
        .arg(root.join(directory_name(app)))
        .status()
        .map_err(|e| e.to_string())?;
    #[cfg(not(windows))]
    if status.success() {
        Ok(())
    } else {
        Err("macOS could not open the application".into())
    }
}
/// Uninstall preserves user settings and removes only the authenticated application tree.
pub fn uninstall(root: &Path, state: &Path, app: Application) -> Result<(), String> {
    let _lease = AppLease::acquire(state, app.id())?;
    recover(root, app)?;
    let bundle = root.join(directory_name(app));
    if !bundle.exists() {
        return Ok(());
    }
    #[cfg(windows)]
    {
        let release = crate::windows_install::cached_release(&bundle, app)?;
        validate_bundle(&bundle, &release)?;
        let application = if app == Application::Tools {
            studio_network::firewall::Application::Tools
        } else {
            studio_network::firewall::Application::Visualizer
        };
        for scope in [
            studio_network::firewall::Scope::Private,
            studio_network::firewall::Scope::Public {
                peers: vec!["192.0.2.1".into()],
            },
        ] {
            studio_network::windows::request(
                &studio_network::firewall::Request {
                    application,
                    operation: studio_network::firewall::Operation::Remove,
                    adapter_id: "uninstall".into(),
                    scope,
                },
                &AtomicBool::new(false),
            )?;
        }
        crate::windows_setup::remove_shortcut(app.name())?;
    }
    #[cfg(not(windows))]
    if installed_version(root, app)?.is_none() {
        return Ok(());
    }
    let tombstone = root.join(format!(".{}-uninstall", app.id()));
    if tombstone.exists() {
        return Err("Previous uninstall needs recovery; application retained".into());
    }
    fs::rename(&bundle, &tombstone).map_err(|e| e.to_string())?;
    if let Err(error) = fs::remove_dir_all(&tombstone) {
        let _ = fs::rename(&tombstone, &bundle);
        return Err(format!("Uninstall incomplete: {error}"));
    }
    let previous = backup(root, app);
    if previous.exists() {
        fs::remove_dir_all(previous).map_err(|e| e.to_string())?;
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    fn temporary() -> PathBuf {
        static ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let p = std::env::temp_dir().join(format!(
            "studio-install-{}-{}",
            std::process::id(),
            ID.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&p).unwrap();
        p
    }
    #[test]
    fn replacement_failure_rolls_back_and_interruption_recovers() {
        let root = temporary();
        let app = Application::Tools;
        fs::create_dir(root.join(directory_name(app))).unwrap();
        fs::write(root.join(directory_name(app)).join("old"), "old").unwrap();
        assert!(replace(&root, &root.join("missing"), app, &AtomicBool::new(false)).is_err());
        assert!(root.join(directory_name(app)).join("old").exists());
        fs::rename(root.join(directory_name(app)), backup(&root, app)).unwrap();
        recover(&root, app).unwrap();
        assert!(root.join(directory_name(app)).join("old").exists());
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn cancellation_preserves_existing() {
        let root = temporary();
        fs::create_dir(root.join(directory_name(Application::Tools))).unwrap();
        assert!(replace(
            &root,
            &root.join("missing"),
            Application::Tools,
            &AtomicBool::new(true)
        )
        .is_err());
        assert!(root.join(directory_name(Application::Tools)).exists());
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn archive_rejects_links_and_excessive_size() {
        let root = temporary();
        let archive = root.join("unsafe.tar.gz");
        let encoder = flate2::write::GzEncoder::new(
            File::create(&archive).unwrap(),
            flate2::Compression::fast(),
        );
        let mut builder = tar::Builder::new(encoder);
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Symlink);
        header.set_size(0);
        header.set_mode(0o777);
        header.set_link_name("/tmp/outside").unwrap();
        header.set_cksum();
        builder
            .append_data(
                &mut header,
                Path::new(&directory_name(Application::Tools)).join("escape"),
                std::io::empty(),
            )
            .unwrap();
        builder.into_inner().unwrap().finish().unwrap();
        assert!(extract(
            &archive,
            &root.join("stage"),
            Application::Tools,
            &AtomicBool::new(false)
        )
        .unwrap_err()
        .contains("links"));
        fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(all(test, target_os = "macos"))]
mod transaction_tests {
    use super::*;
    fn fixture(root: &Path, version: &str) -> (PathBuf, Release) {
        let app = Application::Tools;
        let source = root.join(format!("source-{version}"));
        let bundle = source.join(app.bundle_name());
        fs::create_dir_all(bundle.join("Contents")).unwrap();
        fs::write(bundle.join("Contents/Info.plist"),format!(r#"<?xml version="1.0"?><plist version="1.0"><dict><key>CFBundleIdentifier</key><string>{}</string><key>CFBundleShortVersionString</key><string>{version}</string></dict></plist>"#,app.bundle_id())).unwrap();
        fs::write(bundle.join("marker"), version).unwrap();
        let archive = root.join(format!("{version}.tar.gz"));
        let gzip = flate2::write::GzEncoder::new(
            File::create(&archive).unwrap(),
            flate2::Compression::fast(),
        );
        let mut builder = tar::Builder::new(gzip);
        builder.append_dir_all(app.bundle_name(), bundle).unwrap();
        builder.into_inner().unwrap().finish().unwrap();
        let bytes = fs::read(&archive).unwrap();
        let release = Release {
            application: app,
            version: version.into(),
            platform: "macos".into(),
            architecture: "aarch64".into(),
            distro: "jazzy".into(),
            minimum_macos: "14.0".into(),
            windows: None,
            minimum_launcher: "0.1.0".into(),
            archive_url: format!(
                "https://github.com/example/studio/releases/download/{version}/tools.tar.gz"
            ),
            archive_bytes: bytes.len() as u64,
            sha256: hex::encode(Sha256::digest(&bytes)),
            bundle_id: app.bundle_id().into(),
        };
        (archive, release)
    }
    #[test]
    fn install_update_running_deferral_verification_failure_and_downgrade() {
        let temp = std::env::temp_dir().join(format!("studio-transaction-{}", std::process::id()));
        fs::create_dir_all(&temp).unwrap();
        let root = temp.join("Applications");
        let state = temp.join("state");
        let cancel = AtomicBool::new(false);
        let app = Application::Tools;
        let (first, v1) = fixture(&temp, "0.1.1");
        install_using(&root, &state, &first, &v1, &cancel, |_, _| Ok(())).unwrap();
        assert_eq!(
            installed_version(&root, app).unwrap().as_deref(),
            Some("0.1.1")
        );
        let (second, v2) = fixture(&temp, "0.1.2");
        let lease = AppLease::acquire(&state, app.id()).unwrap();
        assert!(install_using(&root, &state, &second, &v2, &cancel, |_, _| Ok(())).is_err());
        drop(lease);
        assert!(
            install_using(&root, &state, &second, &v2, &cancel, |_, _| Err(
                "synthetic signature rejection".into()
            ))
            .is_err()
        );
        assert_eq!(
            fs::read_to_string(root.join(directory_name(app)).join("marker")).unwrap(),
            "0.1.1"
        );
        install_using(&root, &state, &second, &v2, &cancel, |_, _| Ok(())).unwrap();
        assert_eq!(
            fs::read_to_string(root.join(directory_name(app)).join("marker")).unwrap(),
            "0.1.2"
        );
        assert_eq!(
            fs::read_to_string(backup(&root, app).join("marker")).unwrap(),
            "0.1.1"
        );
        assert!(install_using(&root, &state, &first, &v1, &cancel, |_, _| Ok(())).is_err());
        let mut bad = v2;
        bad.sha256 = "00".repeat(32);
        assert!(verify_archive(&second, &bad, &cancel).is_err());
        bad.archive_bytes += 1;
        assert!(verify_archive(&second, &bad, &cancel).is_err());
        fs::remove_dir_all(temp).unwrap();
    }
}
