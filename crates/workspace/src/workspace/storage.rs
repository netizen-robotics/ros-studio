use super::{Error, Workspace, MAX_FILE_BYTES};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

pub fn load(path: &Path) -> Result<Workspace, Error> {
    Workspace::parse(&read_bytes(path)?)
}
pub(crate) fn read_bytes(path: &Path) -> Result<Vec<u8>, Error> {
    read_bytes_limit(path, MAX_FILE_BYTES)
}
pub fn read_bytes_limit(path: &Path, limit: usize) -> Result<Vec<u8>, Error> {
    if !fs::metadata(path)?.is_file() {
        return Err(super::invalid("source must be a regular file"));
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // A path can change between metadata and open. Nonblocking open keeps
        // a substituted FIFO from waiting forever before descriptor validation.
        options.custom_flags(libc::O_NONBLOCK);
    }
    let file = options.open(path)?;
    if !file.metadata()?.is_file() {
        return Err(super::invalid("source must be a regular file"));
    }
    let mut bytes = Vec::new();
    file.take((limit + 1) as u64).read_to_end(&mut bytes)?;
    Ok(bytes)
}
struct TempGuard(PathBuf);
impl Drop for TempGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}
/// Stage in the destination directory, sync, then rename. No fallback removes the old file.
/// Unix rename replaces atomically. Platforms refusing replacement return an error and
/// preserve the destination. Parent-directory crash durability is not claimed.
pub fn save_atomic(path: &Path, workspace: &Workspace) -> Result<(), Error> {
    save_with(path, workspace, |file, bytes| {
        file.write_all(bytes)?;
        file.sync_all()
    })
}
fn save_with(
    path: &Path,
    workspace: &Workspace,
    write: impl FnOnce(&mut File, &[u8]) -> std::io::Result<()>,
) -> Result<(), Error> {
    let bytes = workspace.encode()?;
    save_bytes_with(path, &bytes, write)
}
pub fn save_bytes(path: &Path, bytes: &[u8]) -> Result<(), Error> {
    save_bytes_with(path, bytes, |file, bytes| {
        file.write_all(bytes)?;
        file.sync_all()
    })
}
fn save_bytes_with(
    path: &Path,
    bytes: &[u8],
    write: impl FnOnce(&mut File, &[u8]) -> std::io::Result<()>,
) -> Result<(), Error> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    if path.file_name().is_none() {
        return Err(super::invalid("destination needs a file name"));
    }
    let mut staged = None;
    for _ in 0..32 {
        let temp = parent.join(format!(
            ".ros-studio-workspace-{}-{}.tmp",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&temp) {
            Ok(file) => {
                staged = Some((TempGuard(temp), file));
                break;
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.into()),
        }
    }
    let (guard, mut file) =
        staged.ok_or_else(|| super::invalid("could not allocate temporary workspace file"))?;
    write(&mut file, bytes)?;
    drop(file);
    fs::rename(&guard.0, path)?;
    Ok(())
}
#[cfg(test)]
pub(super) fn simulate_interrupted_save(path: &Path, workspace: &Workspace) -> Result<(), Error> {
    save_with(path, workspace, |file, _| {
        file.write_all(b"partial")?;
        Err(std::io::Error::new(
            std::io::ErrorKind::Interrupted,
            "injected write interruption",
        ))
    })
}
