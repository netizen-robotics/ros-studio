use crate::Selection;
use std::net::Ipv4Addr;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Adapter {
    /// Native adapter identity, never an IP address.
    pub id: String,
    pub name: String,
    pub addresses: Vec<Ipv4Addr>,
    pub physical: bool,
    pub up: bool,
}
impl Adapter {
    pub fn usable(&self) -> bool {
        self.up && !self.addresses.is_empty()
    }
}
pub fn resolve(
    selection: &Selection,
    adapters: &[Adapter],
) -> Result<Option<(String, Ipv4Addr)>, String> {
    let adapter = match selection {
        Selection::Local => return Ok(Some(("loopback".into(), Ipv4Addr::LOCALHOST))),
        Selection::Custom { .. } => return Ok(None),
        Selection::Adapter { id } => adapters.iter().find(|a| &a.id == id && a.usable()).ok_or(
            "Selected adapter is unavailable. Choose an adapter and reconnect explicitly.",
        )?,
        Selection::Lan => {
            let mut eligible = adapters.iter().filter(|a| a.physical && a.usable());
            let first = eligible.next().ok_or(
                "No active physical IPv4 adapter. Open Network settings to choose an adapter.",
            )?;
            if eligible.next().is_some() {
                return Err(
                    "Several LAN adapters are available. Choose one in Network settings.".into(),
                );
            }
            first
        }
    };
    if adapter.addresses.len() != 1 {
        return Err(
            "Adapter has several IPv4 addresses. Use Custom configuration for this topology."
                .into(),
        );
    }
    Ok(Some((adapter.id.clone(), adapter.addresses[0])))
}
pub fn enumerate() -> Result<Vec<Adapter>, String> {
    let mut adapters = native::enumerate()?;
    for a in &mut adapters {
        a.addresses.retain(|ip| {
            !ip.is_loopback()
                && !ip.is_link_local()
                && !ip.is_unspecified()
                && !ip.is_multicast()
                && !ip.is_broadcast()
        });
        a.addresses.sort();
        a.addresses.dedup();
    }
    adapters.sort_by(|a, b| a.name.cmp(&b.name));
    if adapters.len() > 256 {
        return Err("Too many network adapters".into());
    }
    Ok(adapters)
}

#[cfg(unix)]
mod native {
    use super::*;
    use std::{collections::BTreeMap, ffi::CStr};
    pub fn enumerate() -> Result<Vec<Adapter>, String> {
        let mut head = std::ptr::null_mut();
        // SAFETY: getifaddrs owns the returned linked list until freeifaddrs.
        if unsafe { libc::getifaddrs(&mut head) } != 0 {
            return Err("Cannot enumerate network adapters".into());
        }
        struct List(*mut libc::ifaddrs);
        impl Drop for List {
            fn drop(&mut self) {
                unsafe { libc::freeifaddrs(self.0) }
            }
        }
        let _list = List(head);
        let physical = physical_names();
        let mut result = BTreeMap::new();
        let mut current = head;
        while !current.is_null() {
            let entry = unsafe { &*current };
            if !entry.ifa_name.is_null() {
                let name = unsafe { CStr::from_ptr(entry.ifa_name) }
                    .to_string_lossy()
                    .into_owned();
                let adapter = result.entry(name.clone()).or_insert_with(|| Adapter {
                    id: format!("unix:{name}"),
                    physical: physical.contains(&name),
                    name,
                    addresses: vec![],
                    up: entry.ifa_flags & libc::IFF_UP as u32 != 0,
                });
                if !entry.ifa_addr.is_null()
                    && unsafe { (*entry.ifa_addr).sa_family as i32 } == libc::AF_INET
                {
                    let address = unsafe { &*entry.ifa_addr.cast::<libc::sockaddr_in>() };
                    adapter
                        .addresses
                        .push(Ipv4Addr::from(address.sin_addr.s_addr.to_ne_bytes()));
                }
            }
            current = entry.ifa_next;
        }
        Ok(result.into_values().collect())
    }
    #[cfg(target_os = "macos")]
    fn physical_names() -> Vec<String> {
        type Ref = *const std::ffi::c_void;
        #[link(name = "SystemConfiguration", kind = "framework")]
        extern "C" {
            fn SCNetworkInterfaceCopyAll() -> Ref;
            fn SCNetworkInterfaceGetBSDName(interface: Ref) -> Ref;
            fn SCNetworkInterfaceGetInterfaceType(interface: Ref) -> Ref;
        }
        #[link(name = "CoreFoundation", kind = "framework")]
        extern "C" {
            fn CFArrayGetCount(array: Ref) -> isize;
            fn CFArrayGetValueAtIndex(array: Ref, index: isize) -> Ref;
            fn CFStringGetCString(string: Ref, buffer: *mut i8, size: isize, encoding: u32)
                -> bool;
            fn CFRelease(value: Ref);
        }
        unsafe fn text(value: Ref) -> String {
            let mut buffer = [0i8; 512];
            if value.is_null() || !CFStringGetCString(value, buffer.as_mut_ptr(), 512, 0x08000100) {
                return String::new();
            }
            CStr::from_ptr(buffer.as_ptr())
                .to_string_lossy()
                .into_owned()
        }
        let mut result = vec![];
        unsafe {
            let array = SCNetworkInterfaceCopyAll();
            if array.is_null() {
                return result;
            }
            for index in 0..CFArrayGetCount(array) {
                let interface = CFArrayGetValueAtIndex(array, index);
                let kind = text(SCNetworkInterfaceGetInterfaceType(interface));
                if matches!(kind.as_str(), "IEEE80211" | "Ethernet") {
                    result.push(text(SCNetworkInterfaceGetBSDName(interface)));
                }
            }
            CFRelease(array);
        }
        result
    }
    #[cfg(not(target_os = "macos"))]
    fn physical_names() -> Vec<String> {
        vec![]
    }
}

#[cfg(windows)]
mod native {
    use super::*;
    use std::ffi::CStr;
    use windows_sys::Win32::{NetworkManagement::IpHelper::*, Networking::WinSock::*};
    pub fn enumerate() -> Result<Vec<Adapter>, String> {
        let mut bytes = 16384u32;
        // u64 allocation gives the native linked structures their required alignment.
        let mut storage = vec![0u64; bytes as usize / 8];
        let mut status = unsafe {
            GetAdaptersAddresses(
                AF_INET as u32,
                GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER,
                std::ptr::null(),
                storage.as_mut_ptr().cast(),
                &mut bytes,
            )
        };
        if status == 111 && bytes <= 1024 * 1024 {
            storage.resize((bytes as usize).div_ceil(8), 0);
            status = unsafe {
                GetAdaptersAddresses(
                    AF_INET as u32,
                    GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER,
                    std::ptr::null(),
                    storage.as_mut_ptr().cast(),
                    &mut bytes,
                )
            };
        }
        if status != 0 {
            return Err(format!("Cannot enumerate network adapters ({status})"));
        }
        unsafe fn wide(p: *const u16) -> String {
            if p.is_null() {
                return String::new();
            }
            let mut n = 0;
            while n < 1024 && *p.add(n) != 0 {
                n += 1;
            }
            String::from_utf16_lossy(std::slice::from_raw_parts(p, n))
        }
        let mut result = vec![];
        let mut item = storage.as_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>();
        while !item.is_null() && result.len() < 256 {
            let a = unsafe { &*item };
            let name = unsafe { wide(a.FriendlyName) };
            let mut row: MIB_IF_ROW2 = unsafe { std::mem::zeroed() };
            row.InterfaceLuid = a.Luid;
            let hardware = unsafe { GetIfEntry2(&mut row) } == 0
                && row.InterfaceAndOperStatusFlags._bitfield & 1 != 0;
            let mut addresses = vec![];
            let mut unicast = a.FirstUnicastAddress;
            while !unicast.is_null() {
                let u = unsafe { &*unicast };
                if !u.Address.lpSockaddr.is_null()
                    && unsafe { (*u.Address.lpSockaddr).sa_family } == AF_INET
                {
                    let ip = unsafe { &*u.Address.lpSockaddr.cast::<SOCKADDR_IN>() };
                    addresses.push(Ipv4Addr::from(
                        unsafe { ip.sin_addr.S_un.S_addr }.to_ne_bytes(),
                    ));
                }
                unicast = u.Next;
            }
            let id = unsafe { CStr::from_ptr(a.AdapterName.cast()) }
                .to_string_lossy()
                .into_owned();
            result.push(Adapter {
                id: format!("windows:{id}"),
                name,
                addresses,
                physical: hardware && matches!(a.IfType, 6 | 71),
                up: a.OperStatus == 1,
            });
            item = a.Next;
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn adapter(id: &str, physical: bool, ip: [u8; 4]) -> Adapter {
        Adapter {
            id: id.into(),
            name: id.into(),
            physical,
            up: true,
            addresses: vec![ip.into()],
        }
    }
    #[test]
    fn ambiguity_virtual_exclusion_and_missing_explicit_identity() {
        let wifi = adapter("wifi", true, [192, 168, 1, 2]);
        let vpn = adapter("vpn", false, [100, 64, 1, 2]);
        assert_eq!(
            resolve(&Selection::Lan, &[wifi.clone(), vpn.clone()]).unwrap(),
            Some(("wifi".into(), [192, 168, 1, 2].into()))
        );
        assert!(resolve(
            &Selection::Lan,
            &[wifi.clone(), adapter("wired", true, [10, 0, 0, 2])]
        )
        .is_err());
        assert!(resolve(
            &Selection::Adapter {
                id: "missing".into()
            },
            std::slice::from_ref(&wifi)
        )
        .is_err());
        assert_eq!(
            resolve(&Selection::Adapter { id: "vpn".into() }, &[vpn])
                .unwrap()
                .unwrap()
                .0,
            "vpn"
        );
        let renewed = adapter("wifi", true, [192, 168, 1, 20]);
        assert_eq!(
            resolve(&Selection::Adapter { id: wifi.id }, &[renewed])
                .unwrap()
                .unwrap()
                .1,
            Ipv4Addr::new(192, 168, 1, 20)
        );
    }
}
