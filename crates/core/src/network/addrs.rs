//! IP addresses per interface, from `getifaddrs(3)`.
//!
//! The kernel has no plain-text file with IPv4 addresses per interface, so
//! this is the one place in the crate that calls libc directly.

use std::collections::HashMap;
use std::ffi::CStr;
use std::net::{Ipv4Addr, Ipv6Addr};

/// Addresses of one interface, in the order the kernel lists them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Addresses {
    pub ipv4: Vec<Ipv4Addr>,
    /// Global addresses first; link-local (`fe80::/10`) ones last.
    pub ipv6: Vec<Ipv6Addr>,
}

/// Addresses of every interface of this machine, keyed by interface name.
/// Returns an empty map when `getifaddrs` fails.
pub fn read_addresses() -> HashMap<String, Addresses> {
    let mut map: HashMap<String, Addresses> = HashMap::new();
    let mut head: *mut libc::ifaddrs = std::ptr::null_mut();
    // SAFETY: getifaddrs fills `head` with a list that stays valid until
    // freeifaddrs; it is only read inside this function and freed below.
    if unsafe { libc::getifaddrs(&mut head) } != 0 {
        return map;
    }
    let mut cursor = head;
    while !cursor.is_null() {
        // SAFETY: `cursor` is a node of the list returned above.
        let entry = unsafe { &*cursor };
        cursor = entry.ifa_next;
        if entry.ifa_addr.is_null() || entry.ifa_name.is_null() {
            continue;
        }
        // SAFETY: ifa_name is a NUL-terminated string owned by the list.
        let name = unsafe { CStr::from_ptr(entry.ifa_name) }.to_string_lossy();
        // SAFETY: ifa_addr is non-null; sa_family says which sockaddr it is.
        let family = i32::from(unsafe { (*entry.ifa_addr).sa_family });
        match family {
            libc::AF_INET => {
                // SAFETY: AF_INET addresses are sockaddr_in.
                let sin = unsafe { &*entry.ifa_addr.cast::<libc::sockaddr_in>() };
                let ip = Ipv4Addr::from(u32::from_be(sin.sin_addr.s_addr));
                map.entry(name.into_owned()).or_default().ipv4.push(ip);
            }
            libc::AF_INET6 => {
                // SAFETY: AF_INET6 addresses are sockaddr_in6.
                let sin6 = unsafe { &*entry.ifa_addr.cast::<libc::sockaddr_in6>() };
                let ip = Ipv6Addr::from(sin6.sin6_addr.s6_addr);
                map.entry(name.into_owned()).or_default().ipv6.push(ip);
            }
            _ => {}
        }
    }
    // SAFETY: `head` came from a successful getifaddrs and is freed once.
    unsafe { libc::freeifaddrs(head) };

    for addresses in map.values_mut() {
        // Stable sort: global addresses keep their order, link-local go last.
        addresses.ipv6.sort_by_key(|ip| ip.is_unicast_link_local());
    }
    map
}
