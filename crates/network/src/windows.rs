//! Windows native trust and short-lived helper activation. No shell interpreter.
use sha2::{Digest, Sha256};
use std::{
    os::windows::ffi::OsStrExt,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};
use windows::{
    core::{w, PCWSTR},
    Win32::{
        Foundation::*,
        Security::WinTrust::*,
        System::Threading::*,
        UI::{Shell::*, WindowsAndMessaging::*},
    },
};
pub fn wide(value: impl AsRef<std::ffi::OsStr>) -> Vec<u16> {
    value.as_ref().encode_wide().chain(Some(0)).collect()
}
pub fn verify_signature(path: &Path) -> Result<(), String> {
    let expected=option_env!("STUDIO_WINDOWS_SIGNER_SHA256").ok_or("This experimental build has no pinned Windows release signer; signed deployment is unavailable")?;
    if expected.len() != 64 || hex::decode(expected).map_or(true, |v| v.len() != 32) {
        return Err("Invalid pinned Windows release signer".into());
    }
    let name = wide(path);
    unsafe {
        let mut info = WINTRUST_FILE_INFO {
            cbStruct: std::mem::size_of::<WINTRUST_FILE_INFO>() as u32,
            pcwszFilePath: PCWSTR(name.as_ptr()),
            ..Default::default()
        };
        let mut data = WINTRUST_DATA {
            cbStruct: std::mem::size_of::<WINTRUST_DATA>() as u32,
            dwUIChoice: WTD_UI_NONE,
            fdwRevocationChecks: WTD_REVOKE_WHOLECHAIN,
            dwUnionChoice: WTD_CHOICE_FILE,
            dwStateAction: WTD_STATEACTION_VERIFY,
            dwProvFlags: WTD_REVOCATION_CHECK_CHAIN_EXCLUDE_ROOT,
            Anonymous: WINTRUST_DATA_0 { pFile: &mut info },
            ..Default::default()
        };
        let mut action = WINTRUST_ACTION_GENERIC_VERIFY_V2;
        let status = WinVerifyTrustEx(HWND::default(), &mut action, &mut data);
        let result = (|| {
            if status != 0 {
                return Err(format!(
                    "Executable signature verification failed ({status:#x})"
                ));
            }
            let provider = WTHelperProvDataFromStateData(data.hWVTStateData);
            if provider.is_null() {
                return Err("Missing signature provider".into());
            }
            let signer = WTHelperGetProvSignerFromChain(provider, 0, false, 0);
            if signer.is_null() {
                return Err("Missing executable signer".into());
            }
            let cert = WTHelperGetProvCertFromChain(signer, 0);
            if cert.is_null() || (*cert).pCert.is_null() {
                return Err("Missing signer certificate".into());
            }
            let cert = &*(*cert).pCert;
            let actual = hex::encode(Sha256::digest(std::slice::from_raw_parts(
                cert.pbCertEncoded,
                cert.cbCertEncoded as usize,
            )));
            if !actual.eq_ignore_ascii_case(expected) {
                return Err("Executable publisher is not the pinned ROS Studio publisher".into());
            }
            Ok(())
        })();
        data.dwStateAction = WTD_STATEACTION_CLOSE;
        let _ = WinVerifyTrustEx(HWND::default(), &mut action, &mut data);
        result
    }
}
pub fn check_build(minimum: u32) -> Result<(), String> {
    use windows::{
        Wdk::System::SystemServices::RtlGetVersion,
        Win32::System::SystemInformation::OSVERSIONINFOW,
    };
    let mut info = OSVERSIONINFOW {
        dwOSVersionInfoSize: std::mem::size_of::<OSVERSIONINFOW>() as u32,
        ..Default::default()
    };
    unsafe {
        RtlGetVersion(&mut info).ok().map_err(|e| e.to_string())?;
    }
    if info.dwBuildNumber < minimum {
        Err(format!(
            "This release requires Windows build {minimum} or later"
        ))
    } else {
        Ok(())
    }
}
/// Called on a worker. Helper has a fixed sibling path and accepts only a typed request.
pub fn request(request: &crate::firewall::Request, cancel: &AtomicBool) -> Result<String, String> {
    if !crate::MANAGED_FIREWALL_AVAILABLE {
        return Err("Store installations use Windows-managed network permissions. Contact your administrator if LAN access is blocked.".into());
    }
    request.validate()?;
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let directory = exe.parent().ok_or("Application directory missing")?;
    let directory = if exe
        .file_stem()
        .is_some_and(|s| s == format!("studio-{}", request.application.id()).as_str())
    {
        directory.to_path_buf()
    } else {
        directory.join(request.application.id())
    };
    let helper = directory.join("studio-network-helper.exe");
    verify_signature(&helper)?;
    let cancellation = Cancellation::new()?;
    let encoded = hex::encode(serde_json::to_vec(request).map_err(|e| e.to_string())?);
    let arguments = wide(format!(
        "--parent {} --request {encoded} --token {}",
        std::process::id(),
        cancellation.nonce
    ));
    let path = wide(&helper);
    unsafe {
        let mut info = SHELLEXECUTEINFOW {
            cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
            fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_FLAG_NO_UI,
            lpVerb: if request.operation == crate::firewall::Operation::Inspect {
                w!("open")
            } else {
                w!("runas")
            },
            lpFile: PCWSTR(path.as_ptr()),
            lpParameters: PCWSTR(arguments.as_ptr()),
            nShow: SW_SHOWNORMAL.0,
            ..Default::default()
        };
        ShellExecuteExW(&mut info).map_err(|e|format!("Network setup was not approved or could not start: {e}. Installation remains usable; LAN reception may be blocked."))?;
        let process = NativeHandle(info.hProcess);
        let start = std::time::Instant::now();
        let result = loop {
            if WaitForSingleObject(process.0, 100) == WAIT_OBJECT_0 {
                let mut code = 1;
                GetExitCodeProcess(process.0, &mut code).map_err(|e| e.to_string())?;
                break if code == 0 {
                    Ok(
                        "Network access check/setup completed. Connect explicitly when ready."
                            .into(),
                    )
                } else {
                    Err(format!("Network access setup did not complete ({code}); see the helper's explanation. Installation remains usable."))
                };
            }
            if cancel.load(Ordering::Acquire) || start.elapsed().as_secs() > 180 {
                cancellation.cancel();
                let _ = WaitForSingleObject(process.0, 5000);
                break Err("Network access setup canceled; no automatic reconnect".into());
            }
        };
        result
    }
}

struct NativeHandle(HANDLE);
// Kernel handles may be waited and closed from any thread; ownership stays unique.
unsafe impl Send for NativeHandle {}
impl NativeHandle {
    fn wait(&self, millis: u32) -> WAIT_EVENT {
        unsafe { WaitForSingleObject(self.0, millis) }
    }
}
impl Drop for NativeHandle {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}
fn current_sid() -> Result<String, String> {
    use windows::{
        core::PWSTR,
        Win32::Security::{Authorization::ConvertSidToStringSidW, *},
    };
    let mut token = HANDLE::default();
    unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) }
        .map_err(|e| e.to_string())?;
    let token = NativeHandle(token);
    let mut needed = 0;
    unsafe {
        let _ = GetTokenInformation(token.0, TokenUser, None, 0, &mut needed);
    }
    if needed == 0 || needed > 65536 {
        return Err("Invalid current user token".into());
    }
    let mut buffer = vec![0usize; (needed as usize).div_ceil(std::mem::size_of::<usize>())];
    unsafe {
        GetTokenInformation(
            token.0,
            TokenUser,
            Some(buffer.as_mut_ptr().cast()),
            needed,
            &mut needed,
        )
    }
    .map_err(|e| e.to_string())?;
    let user = unsafe { &*buffer.as_ptr().cast::<TOKEN_USER>() };
    let mut sid = PWSTR::null();
    unsafe { ConvertSidToStringSidW(user.User.Sid, &mut sid) }.map_err(|e| e.to_string())?;
    let result = unsafe { sid.to_string() }.map_err(|e| e.to_string());
    unsafe {
        let _ = LocalFree(HLOCAL(sid.0.cast()));
    }
    result
}
struct Cancellation {
    event: NativeHandle,
    nonce: String,
}
impl Cancellation {
    fn new() -> Result<Self, String> {
        use windows::Win32::Security::{
            Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW, *,
        };
        let guid =
            unsafe { windows::Win32::System::Com::CoCreateGuid() }.map_err(|e| e.to_string())?;
        let nonce = format!("{guid:?}").replace(['{', '}', '-'], "");
        if nonce.len() != 32 || !nonce.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err("Invalid operation nonce".into());
        }
        let name = wide(format!(
            "Local\\ROSStudio.Network.{}.{nonce}",
            std::process::id()
        ));
        let acl = wide(format!(
            "D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;GA;;;{})",
            current_sid()?
        ));
        let mut descriptor = PSECURITY_DESCRIPTOR::default();
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                PCWSTR(acl.as_ptr()),
                1,
                &mut descriptor,
                None,
            )
        }
        .map_err(|e| e.to_string())?;
        let attributes = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor.0,
            bInheritHandle: false.into(),
        };
        let event = unsafe { CreateEventW(Some(&attributes), true, false, PCWSTR(name.as_ptr())) };
        let already_exists = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
        unsafe {
            let _ = LocalFree(HLOCAL(descriptor.0));
        }
        let event = NativeHandle(event.map_err(|e| e.to_string())?);
        if already_exists {
            return Err("Network operation event already exists".into());
        }
        Ok(Self { event, nonce })
    }
    fn cancel(&self) {
        unsafe {
            let _ = SetEvent(self.event.0);
        }
    }
}
impl Drop for Cancellation {
    fn drop(&mut self) {
        self.cancel();
    }
}
/// Elevated helper watches a caller-owned cancel event and parent lifetime. It has
/// no service lifetime and cannot keep prompting or mutating after cancellation.
pub struct CancelWatch {
    stop: std::sync::Arc<AtomicBool>,
    join: Option<std::thread::JoinHandle<()>>,
}
impl Drop for CancelWatch {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}
pub fn watch_cancel(parent: u32, nonce: &str) -> Result<CancelWatch, String> {
    if nonce.len() != 32 || !nonce.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("Invalid operation nonce".into());
    }
    let name = wide(format!("Local\\ROSStudio.Network.{parent}.{nonce}"));
    let event = NativeHandle(
        unsafe { OpenEventW(SYNCHRONIZATION_SYNCHRONIZE, false, PCWSTR(name.as_ptr())) }
            .map_err(|e| e.to_string())?,
    );
    let parent = NativeHandle(
        unsafe { OpenProcess(PROCESS_SYNCHRONIZE, false, parent) }.map_err(|e| e.to_string())?,
    );
    let stop = std::sync::Arc::new(AtomicBool::new(false));
    let stopped = stop.clone();
    let join = std::thread::spawn(move || {
        let started = std::time::Instant::now();
        while !stopped.load(Ordering::Acquire) {
            if event.wait(100) != WAIT_TIMEOUT
                || parent.wait(0) != WAIT_TIMEOUT
                || started.elapsed().as_secs() > 180
            {
                std::process::exit(1223);
            }
        }
    });
    Ok(CancelWatch {
        stop,
        join: Some(join),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unsigned_build_does_not_enable_privileged_setup() {
        if option_env!("STUDIO_WINDOWS_SIGNER_SHA256").is_none() {
            assert!(verify_signature(&std::env::current_exe().unwrap())
                .unwrap_err()
                .contains("experimental"));
        }
    }
    #[test]
    fn helper_cancellation_closes_the_child_within_a_bound() {
        if let Ok(nonce) = std::env::var("STUDIO_TEST_CANCEL_NONCE") {
            let parent = std::env::var("STUDIO_TEST_CANCEL_PARENT")
                .unwrap()
                .parse()
                .unwrap();
            let _watch = watch_cancel(parent, &nonce).unwrap();
            std::thread::sleep(std::time::Duration::from_secs(20));
            panic!("Cancellation failed");
        }
        let cancellation = Cancellation::new().unwrap();
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "windows::tests::helper_cancellation_closes_the_child_within_a_bound",
                "--nocapture",
            ])
            .env("STUDIO_TEST_CANCEL_NONCE", &cancellation.nonce)
            .env("STUDIO_TEST_CANCEL_PARENT", std::process::id().to_string())
            .spawn()
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(300));
        cancellation.cancel();
        let started = std::time::Instant::now();
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                assert_eq!(status.code(), Some(1223));
                break;
            }
            if started.elapsed().as_secs() > 5 {
                let _ = child.kill();
                let _ = child.wait();
                panic!("Helper cancellation did not terminate child");
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
    }
}
