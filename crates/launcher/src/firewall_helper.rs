//! Native Windows firewall boundary. Inputs select only a verified installed app.
use crate::catalog::Application;
use std::{mem::size_of, path::PathBuf};
use studio_network::{
    firewall::{Operation, Request, Scope},
    windows::{verify_signature, wide},
};
use windows::{
    core::{w, BSTR, PCWSTR, PWSTR},
    Win32::{
        Foundation::*,
        NetworkManagement::WindowsFirewall::*,
        Security::{Authorization::ConvertSidToStringSidW, *},
        System::{Com::*, Threading::*, Variant::*},
        UI::{Shell::*, WindowsAndMessaging::*},
    },
};
struct Handle(HANDLE);
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}
struct Com;
impl Drop for Com {
    fn drop(&mut self) {
        unsafe {
            CoUninitialize();
        }
    }
}
pub fn message(text: &str, error: bool) {
    let text = wide(text);
    unsafe {
        MessageBoxW(
            HWND::default(),
            PCWSTR(text.as_ptr()),
            w!("ROS Studio network access"),
            MB_OK
                | if error {
                    MB_ICONERROR
                } else {
                    MB_ICONINFORMATION
                },
        );
    }
}
fn text(value: PWSTR) -> Result<String, String> {
    let result = unsafe { value.to_string() }.map_err(|e| e.to_string());
    unsafe {
        CoTaskMemFree(Some(value.0.cast()));
    }
    result
}
fn variant(value: &windows::core::VARIANT) -> Result<String, String> {
    text(unsafe { VariantToStringAlloc(value) }.map_err(|e| e.to_string())?)
}
/// Over-the-shoulder elevation resolves the requesting user's profile via their token,
/// never via the elevated account's environment or a command-line path.
pub fn run(parent: u32, request: &Request) -> Result<String, String> {
    request.validate()?;
    unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED)
            .ok()
            .map_err(|e| e.to_string())?;
    }
    let _com = Com;
    let parent = Handle(
        unsafe {
            OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
                false,
                parent,
            )
        }
        .map_err(|e| e.to_string())?,
    );
    let mut token = HANDLE::default();
    unsafe {
        OpenProcessToken(
            parent.0,
            TOKEN_QUERY | TOKEN_IMPERSONATE | TOKEN_DUPLICATE,
            &mut token,
        )
    }
    .map_err(|e| e.to_string())?;
    let token = Handle(token);
    let mut needed = 0;
    unsafe {
        let _ = GetTokenInformation(token.0, TokenUser, None, 0, &mut needed);
    }
    if needed == 0 || needed > 65536 {
        return Err("Invalid requesting user token".into());
    }
    // Aligned storage is needed for TOKEN_USER's native pointers.
    let mut buffer = vec![0usize; (needed as usize).div_ceil(size_of::<usize>())];
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
    let identity = unsafe { sid.to_string() }.map_err(|e| e.to_string())?;
    unsafe {
        let _ = LocalFree(HLOCAL(sid.0.cast()));
    }
    let profile = text(
        unsafe { SHGetKnownFolderPath(&FOLDERID_LocalAppData, KF_FLAG_DEFAULT, token.0) }
            .map_err(|e| e.to_string())?,
    )?;
    let root = PathBuf::from(profile).join("Programs/ROS Studio");
    let app = match request.application {
        studio_network::firewall::Application::Tools => Application::Tools,
        studio_network::firewall::Application::Visualizer => Application::Visualizer,
    };
    let bundle = root.join(app.id());
    let mut image = vec![0u16; 32768];
    let mut len = image.len() as u32;
    unsafe {
        QueryFullProcessImageNameW(
            parent.0,
            PROCESS_NAME_WIN32,
            PWSTR(image.as_mut_ptr()),
            &mut len,
        )
    }
    .map_err(|e| e.to_string())?;
    let caller =
        PathBuf::from(String::from_utf16(&image[..len as usize]).map_err(|e| e.to_string())?);
    let frontend = bundle.join(format!("studio-{}.exe", app.id()));
    if caller != frontend
        && caller != root.join("studio-launcher.exe")
        && caller != root.join("studio-bootstrap.exe")
    {
        return Err(
            "Requesting application is not installed in the user's ROS Studio directory".into(),
        );
    }
    verify_signature(&caller)?;
    let current = std::env::current_exe().map_err(|e| e.to_string())?;
    if current != bundle.join("studio-network-helper.exe") {
        return Err("Only the installed helper can manage network access".into());
    }
    let release = crate::windows_install::cached_release(&bundle, app)?;
    crate::install::validate_bundle(&bundle, &release)?;
    let backend = bundle.join("studio-backend.exe");
    let adapter = studio_network::adapters::enumerate()?
        .into_iter()
        .find(|a| a.id == request.adapter_id && a.usable());
    if request.operation != Operation::Remove && adapter.is_none() {
        return Err(
            "Selected adapter is unavailable. Select an adapter before repairing access.".into(),
        );
    }
    let scope = if matches!(request.scope, Scope::Private) {
        "Private"
    } else {
        "Public"
    };
    let name = format!("ROSStudio.{identity}.{}.UDP.{scope}", app.id());
    let group = format!("ROSStudio.{identity}.{}", app.id());
    let remote = match &request.scope {
        Scope::Private => "LocalSubnet".into(),
        Scope::Public { peers } => peers.join(","),
    };
    unsafe {
        let policy: INetFwPolicy2 = CoCreateInstance(&NetFwPolicy2, None, CLSCTX_INPROC_SERVER)
            .map_err(|e| e.to_string())?;
        let rules = policy.Rules().map_err(|e| e.to_string())?;
        let existing = rules.Item(&BSTR::from(&name)).ok();
        if let Some(rule) = &existing {
            if rule.Grouping().map_err(|e| e.to_string())? != group {
                return Err("A foreign rule has the reserved name; it will not be changed".into());
            }
        }
        if request.operation == Operation::Inspect {
            let Some(rule) = existing else {
                return Ok(format!("No installer-owned {scope} rule exists for {}. LAN reception may need setup. An empty topic list alone does not identify a firewall failure.",app.name()));
            };
            let desired = InitVariantFromStringArray(&[PCWSTR(
                wide(&adapter.as_ref().unwrap().name).as_ptr(),
            )])
            .map_err(|e| e.to_string())?;
            let valid = rule.ApplicationName().map_err(|e| e.to_string())?
                == backend.to_string_lossy().as_ref()
                && rule.Protocol().map_err(|e| e.to_string())? == 17
                && rule.Enabled().map_err(|e| e.to_string())?.as_bool()
                && rule.Profiles().map_err(|e| e.to_string())?
                    == if scope == "Private" {
                        NET_FW_PROFILE2_PRIVATE.0
                    } else {
                        NET_FW_PROFILE2_PUBLIC.0
                    }
                && rule.RemoteAddresses().map_err(|e| e.to_string())? == remote
                && rule.Direction().map_err(|e| e.to_string())? == NET_FW_RULE_DIR_IN
                && rule.Action().map_err(|e| e.to_string())? == NET_FW_ACTION_ALLOW
                && !rule.EdgeTraversal().map_err(|e| e.to_string())?.as_bool()
                && variant(&rule.Interfaces().map_err(|e| e.to_string())?)? == variant(&desired)?;
            return Ok(if valid {
                format!("Installer-owned {scope} rule matches the selected adapter and backend. Other firewall or network policies may still restrict traffic.")
            } else {
                "Installer-owned rule is stale or differs from the proposed scope. Review and repair explicitly.".into()
            });
        }
        if policy.LocalPolicyModifyState().map_err(|e| e.to_string())? != NET_FW_MODIFY_STATE_OK {
            return Err(
                "Windows policy prevents local firewall changes; contact the administrator".into(),
            );
        }
        let prompt=wide(format!("{} inbound UDP access for {}?\nUser: {identity}\nProgram: {}\nNetwork: {scope}\nAdapter: {}\nApproved remote scope: {remote}\n\nOnly this ROS Studio rule will change. Other firewall settings remain under Windows control.",if request.operation==Operation::Remove{"Remove"}else{"Set up"},app.name(),backend.display(),adapter.as_ref().map_or("unavailable",|a|a.name.as_str())));
        if MessageBoxW(
            HWND::default(),
            PCWSTR(prompt.as_ptr()),
            w!("Approve ROS Studio network rule"),
            MB_YESNO | MB_DEFBUTTON2 | MB_ICONQUESTION,
        ) != IDYES
        {
            return Err("Network setup canceled. Installation remains usable; LAN reception may be blocked.".into());
        }
        if WaitForSingleObject(parent.0, 0) != WAIT_TIMEOUT {
            return Err("Requesting application closed; setup canceled".into());
        }
        // Recheck signed runtime immediately before any rule mutation.
        crate::install::validate_bundle(&bundle, &release)?;
        if request.operation == Operation::Remove {
            if existing.is_some() {
                rules
                    .Remove(&BSTR::from(&name))
                    .map_err(|e| e.to_string())?;
            }
            return Ok(format!("Installer-owned {scope} rule removed"));
        }
        if request.operation == Operation::Add && existing.is_some() {
            return Err("Rule already exists. Inspect or repair it explicitly.".into());
        }
        let rule: INetFwRule =
            CoCreateInstance(&NetFwRule, None, CLSCTX_INPROC_SERVER).map_err(|e| e.to_string())?;
        rule.SetName(&BSTR::from(&name))
            .map_err(|e| e.to_string())?;
        rule.SetGrouping(&BSTR::from(&group))
            .map_err(|e| e.to_string())?;
        rule.SetDescription(&BSTR::from("ROS Studio installer-owned backend UDP access"))
            .map_err(|e| e.to_string())?;
        rule.SetApplicationName(&BSTR::from(backend.to_string_lossy().as_ref()))
            .map_err(|e| e.to_string())?;
        rule.SetProtocol(17).map_err(|e| e.to_string())?;
        rule.SetDirection(NET_FW_RULE_DIR_IN)
            .map_err(|e| e.to_string())?;
        rule.SetAction(NET_FW_ACTION_ALLOW)
            .map_err(|e| e.to_string())?;
        rule.SetProfiles(if scope == "Private" {
            NET_FW_PROFILE2_PRIVATE.0
        } else {
            NET_FW_PROFILE2_PUBLIC.0
        })
        .map_err(|e| e.to_string())?;
        rule.SetRemoteAddresses(&BSTR::from(&remote))
            .map_err(|e| e.to_string())?;
        let alias = wide(&adapter.as_ref().unwrap().name);
        let interfaces =
            InitVariantFromStringArray(&[PCWSTR(alias.as_ptr())]).map_err(|e| e.to_string())?;
        rule.SetInterfaces(&interfaces).map_err(|e| e.to_string())?;
        rule.SetEdgeTraversal(VARIANT_FALSE)
            .map_err(|e| e.to_string())?;
        rule.SetEnabled(VARIANT_TRUE).map_err(|e| e.to_string())?;
        // Add replaces the same owned name atomically in the firewall store; no broad intermediate rule.
        rules.Add(&rule).map_err(|e| e.to_string())?;
        Ok(format!(
            "{scope} backend access configured. Connect explicitly when ready."
        ))
    }
}
