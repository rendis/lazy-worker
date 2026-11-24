//! Finds the devices on the local network that announce an SSH server over Bonjour.
//!
//! We ask the system resolver (`mDNSResponder`) through its `dns_sd` API, which is part of
//! libSystem. We never open our own multicast socket, because that would compete with the system
//! one, and we announce nothing here: the only service this device announces is the one of an open
//! pairing window, which `advert.rs` registers. We also read that service on other devices, for the
//! account name. What a device announces is text from the network, so
//! every name and host is checked before it leaves this module.

use super::{
    advert::{decode_txt, Advert, SERVICE_TYPE},
    destination::Destination,
    peers::clean_name,
};
use serde::Serialize;
use std::time::Duration;

/// How long one search runs. Devices answer within a fraction of a second on a quiet network.
pub const TIMEOUT: Duration = Duration::from_secs(2);
/// The most devices that one search follows, so a hostile network cannot make it grow.
const MAX_FOUND: usize = 64;

/// A device that announced an SSH server. `host` has passed the rules of `Destination`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Found {
    pub name: String,
    pub host: String,
    pub port: u16,
    /// What the pairing window of that device announces, when it has one open. It is a hint
    /// from the network: the code is still needed to pair.
    pub pairing: Option<Pairing>,
}

/// The account and the id that an open pairing window announced.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Pairing {
    pub account: String,
    pub id: String,
}

/// What a resolve of one service returned.
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
struct Raw {
    name: String,
    host: String,
    port: u16,
    txt: Vec<u8>,
}

/// Builds one entry from text that the network sent, or nothing when any part is not valid.
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
fn found_from(name: &str, host: &str, port: u16) -> Option<Found> {
    let host = host.strip_suffix('.').unwrap_or(host);
    let destination = Destination::new("x", host, Some(port)).ok()?;
    let host = destination.host().to_owned();
    let name = clean_name(name);
    Some(Found {
        name: if name.is_empty() { host.clone() } else { name },
        host,
        port,
        pairing: None,
    })
}

/// Puts each valid pairing announcement on the device of the same host. An announcement without a
/// device of that host is dropped, and so is a second one for a host.
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
fn attach(mut found: Vec<Found>, adverts: Vec<Raw>) -> Vec<Found> {
    for raw in adverts {
        let Some(host) = found_from(&raw.name, &raw.host, raw.port.max(1)).map(|item| item.host) else { continue };
        let Some(Advert { account, id }) = decode_txt(&raw.txt) else { continue };
        for item in found.iter_mut().filter(|item| item.host == host && item.pairing.is_none()) {
            item.pairing = Some(Pairing { account: account.clone(), id: id.clone() });
        }
    }
    found
}

/// Drops this device and repeats, then sorts by name. One device answers once for each network
/// interface, so the host and port are the identity.
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
fn finish(found: Vec<Found>, own_host: Option<&str>) -> Vec<Found> {
    let own = own_host.map(|host| host.trim_end_matches('.').to_ascii_lowercase());
    let mut kept: Vec<Found> = Vec::new();
    for item in found {
        if own.as_deref() == Some(item.host.as_str()) {
            continue;
        }
        if kept.iter().any(|other| other.host == item.host && other.port == item.port) {
            continue;
        }
        kept.push(item);
    }
    kept.sort_by_cached_key(|item| (item.name.to_lowercase(), item.host.clone(), item.port));
    kept
}

/// Searches for `TIMEOUT`. It blocks, so call it off the main thread.
#[cfg(target_os = "macos")]
pub fn discover() -> Result<Vec<Found>, &'static str> {
    let own = crate::proxy::network().hostname;
    // Both searches run at the same time, so the wait stays one `TIMEOUT`. A failed search for
    // pairing windows only means that no account is filled in.
    let pairing = std::thread::spawn(|| dns_sd::browse(SERVICE_TYPE, TIMEOUT));
    let servers: Vec<Found> = dns_sd::browse("_ssh._tcp", TIMEOUT)?
        .into_iter()
        .filter_map(|raw| found_from(&raw.name, &raw.host, raw.port))
        .collect();
    let adverts = pairing.join().ok().and_then(Result::ok).unwrap_or_default();
    Ok(attach(finish(servers, own.as_deref()), adverts))
}

#[cfg(not(target_os = "macos"))]
pub fn discover() -> Result<Vec<Found>, &'static str> {
    Err("unsupported")
}

/// The system `dns_sd` API. The functions are declared by hand, because the surface is small.
///
/// Soundness rules of this module:
/// - Every callback runs on this thread, inside `DNSServiceProcessResult`. A context pointer is a
///   shared reference to a `RefCell` that outlives the reference it was given to, and no borrow of
///   that cell is held while `DNSServiceProcessResult` runs.
/// - Every `DNSServiceRef` lives in a `Service`, whose `Drop` deallocates it exactly once. A
///   `Service` is dropped before the cell its callback points to, so no callback runs on freed memory.
/// - A callback only copies text out. A string that is not UTF-8 is skipped.
#[cfg(target_os = "macos")]
mod dns_sd {
    use super::{Raw, MAX_FOUND};
    use std::{
        cell::RefCell,
        ffi::{c_char, c_int, c_uint, c_void, CStr, CString},
        ptr,
        rc::Rc,
        time::{Duration, Instant},
    };

    type Ref = *mut c_void;
    type Flags = u32;
    type ErrorCode = i32;

    const FLAGS_ADD: Flags = 0x2;
    const INTERFACE_ANY: u32 = 0;
    const NO_ERROR: ErrorCode = 0;
    const POLL_IN: i16 = 0x1;

    #[repr(C)]
    struct PollFd {
        fd: c_int,
        events: i16,
        revents: i16,
    }

    type BrowseReply = extern "C" fn(Ref, Flags, u32, ErrorCode, *const c_char, *const c_char, *const c_char, *mut c_void);
    type ResolveReply =
        extern "C" fn(Ref, Flags, u32, ErrorCode, *const c_char, *const c_char, u16, u16, *const u8, *mut c_void);

    extern "C" {
        fn DNSServiceBrowse(
            sd_ref: *mut Ref,
            flags: Flags,
            interface: u32,
            regtype: *const c_char,
            domain: *const c_char,
            callback: BrowseReply,
            context: *mut c_void,
        ) -> ErrorCode;
        fn DNSServiceResolve(
            sd_ref: *mut Ref,
            flags: Flags,
            interface: u32,
            name: *const c_char,
            regtype: *const c_char,
            domain: *const c_char,
            callback: ResolveReply,
            context: *mut c_void,
        ) -> ErrorCode;
        fn DNSServiceRefSockFD(sd_ref: Ref) -> c_int;
        fn DNSServiceProcessResult(sd_ref: Ref) -> ErrorCode;
        fn DNSServiceRefDeallocate(sd_ref: Ref);
        fn poll(fds: *mut PollFd, count: c_uint, timeout_ms: c_int) -> c_int;
    }

    /// One live `DNSServiceRef`, deallocated when it drops.
    struct Service(Ref);

    impl Service {
        fn fd(&self) -> c_int {
            // SAFETY: `self.0` is a live ref, because it is deallocated only in `drop`.
            unsafe { DNSServiceRefSockFD(self.0) }
        }

        /// Reads the answers that wait on the socket. They run the callback of this ref.
        fn process(&self) -> bool {
            // SAFETY: as above. The callback only borrows the cell behind its context.
            unsafe { DNSServiceProcessResult(self.0) == NO_ERROR }
        }
    }

    impl Drop for Service {
        fn drop(&mut self) {
            // SAFETY: the ref came from a call that succeeded and is deallocated once.
            unsafe { DNSServiceRefDeallocate(self.0) };
        }
    }

    /// A service instance that the browse reported.
    struct Instance {
        name: CString,
        domain: CString,
        interface: u32,
    }

    /// An answer to a resolve: the host, the port in host byte order, and the TXT record.
    type Resolved = (String, u16, Vec<u8>);

    struct Resolving {
        service: Option<Service>,
        /// The callback context. It sits on the heap, so its address does not change when the entry moves.
        answer: Rc<RefCell<Option<Resolved>>>,
        name: String,
    }

    fn text<'a>(pointer: *const c_char) -> Option<&'a str> {
        if pointer.is_null() {
            return None;
        }
        // SAFETY: the system passes a NUL-terminated string that lives during the callback.
        unsafe { CStr::from_ptr(pointer) }.to_str().ok()
    }

    extern "C" fn on_browse(
        _: Ref, flags: Flags, interface: u32, error: ErrorCode,
        name: *const c_char, _regtype: *const c_char, domain: *const c_char, context: *mut c_void,
    ) {
        if error != NO_ERROR || flags & FLAGS_ADD == 0 {
            return;
        }
        let (Some(name), Some(domain)) = (text(name), text(domain)) else { return };
        let (Ok(name), Ok(domain)) = (CString::new(name), CString::new(domain)) else { return };
        // SAFETY: `context` is the `RefCell` that `browse` passed, which outlives the browse ref.
        let instances = unsafe { &*(context as *const RefCell<Vec<Instance>>) };
        instances.borrow_mut().push(Instance { name, domain, interface });
    }

    extern "C" fn on_resolve(
        _: Ref, _flags: Flags, _interface: u32, error: ErrorCode, _full: *const c_char,
        host: *const c_char, port: u16, txt_len: u16, txt: *const u8, context: *mut c_void,
    ) {
        if error != NO_ERROR {
            return;
        }
        let Some(host) = text(host) else { return };
        // SAFETY: `context` is the `RefCell` inside the `Rc` of a `Resolving`, which outlives its ref.
        let answer = unsafe { &*(context as *const RefCell<Option<Resolved>>) };
        let txt = if txt.is_null() {
            Vec::new()
        } else {
            // SAFETY: the system passes `txt_len` bytes that live during the callback.
            unsafe { std::slice::from_raw_parts(txt, usize::from(txt_len)) }.to_vec()
        };
        // The port arrives in network byte order.
        *answer.borrow_mut() = Some((host.to_owned(), u16::from_be(port), txt));
    }

    /// Browses `regtype` in `local.` and resolves each result until the deadline.
    pub fn browse(regtype: &str, timeout: Duration) -> Result<Vec<Raw>, &'static str> {
        let deadline = Instant::now() + timeout;
        let instances: RefCell<Vec<Instance>> = RefCell::new(Vec::new());
        let regtype = CString::new(regtype).map_err(|_| "discoveryFailed")?;
        let domain = CString::new("local.").map_err(|_| "discoveryFailed")?;
        let mut reference: Ref = ptr::null_mut();
        // SAFETY: both strings live through the call, and the context is `instances`, which
        // outlives `browser` because it is declared first and so dropped last.
        let code = unsafe {
            DNSServiceBrowse(
                &mut reference, 0, INTERFACE_ANY, regtype.as_ptr(), domain.as_ptr(), on_browse,
                &instances as *const _ as *mut c_void,
            )
        };
        if code != NO_ERROR || reference.is_null() {
            return Err("discoveryFailed");
        }
        let browser = Service(reference);
        let mut resolving: Vec<Resolving> = Vec::new();
        let mut done: Vec<Raw> = Vec::new();
        let mut started = 0;

        loop {
            // Start a resolve for each instance that the browse reported since the last turn.
            let fresh: Vec<Instance> = instances.borrow_mut().drain(..).collect();
            for instance in fresh {
                if started >= MAX_FOUND {
                    break;
                }
                started += 1;
                let mut entry = Resolving {
                    service: None,
                    answer: Rc::new(RefCell::new(None)),
                    name: instance.name.to_string_lossy().into_owned(),
                };
                let mut reference: Ref = ptr::null_mut();
                // SAFETY: the strings live through the call. The context is the cell inside the
                // `Rc`, which `entry` keeps until after its service dropped.
                let code = unsafe {
                    DNSServiceResolve(
                        &mut reference, 0, instance.interface, instance.name.as_ptr(), regtype.as_ptr(),
                        instance.domain.as_ptr(), on_resolve, Rc::as_ptr(&entry.answer) as *mut c_void,
                    )
                };
                if code == NO_ERROR && !reference.is_null() {
                    entry.service = Some(Service(reference));
                    resolving.push(entry);
                }
            }

            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            // The first entry is the browse. The others follow `resolving` that still has a service.
            let mut owners: Vec<Option<usize>> = vec![None];
            let mut fds = vec![PollFd { fd: browser.fd(), events: POLL_IN, revents: 0 }];
            for (index, entry) in resolving.iter().enumerate() {
                if let Some(service) = &entry.service {
                    owners.push(Some(index));
                    fds.push(PollFd { fd: service.fd(), events: POLL_IN, revents: 0 });
                }
            }
            let millis = c_int::try_from(left.as_millis().min(1000)).unwrap_or(1000);
            // SAFETY: `fds` is a live array of `fds.len()` entries.
            let ready = unsafe { poll(fds.as_mut_ptr(), fds.len() as c_uint, millis) };
            if ready < 0 {
                // A signal interrupted the wait. Any other failure ends the search with what it has.
                if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                break;
            }
            for (owner, fd) in owners.iter().zip(&fds) {
                if fd.revents == 0 {
                    continue;
                }
                let Some(index) = owner else {
                    if !browser.process() {
                        return Ok(done);
                    }
                    continue;
                };
                let entry = &resolving[*index];
                let alive = entry.service.as_ref().is_some_and(Service::process);
                let answer = entry.answer.borrow_mut().take();
                if let Some((host, port, txt)) = &answer {
                    done.push(Raw { name: entry.name.clone(), host: host.clone(), port: *port, txt: txt.clone() });
                }
                // One answer is enough. Dropping the service deallocates its ref.
                if answer.is_some() || !alive {
                    resolving[*index].service = None;
                }
            }
        }
        Ok(done)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(name: &str, host: &str, port: u16) -> Found {
        Found { name: name.into(), host: host.into(), port, pairing: None }
    }

    const ID: &str = "0123456789abcdef0123456789abcdef";

    fn raw(host: &str, txt: Vec<u8>) -> Raw {
        Raw { name: "Studio".into(), host: host.into(), port: 22, txt }
    }

    fn txt(account: &str) -> Vec<u8> {
        crate::remote::advert::encode_txt(&Advert { account: account.into(), id: ID.into() }).unwrap()
    }

    #[test]
    fn an_announcement_is_matched_to_the_device_of_the_same_host() {
        let list = vec![found("A", "a.local", 22), found("B", "b.local", 22)];
        let out = attach(list, vec![raw("B.local.", txt("rey")), raw("c.local", txt("zed"))]);
        assert_eq!(out[0].pairing, None);
        assert_eq!(out[1].pairing, Some(Pairing { account: "rey".into(), id: ID.into() }));
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn a_bad_record_or_host_attaches_nothing_and_the_first_good_record_wins() {
        let list = vec![found("A", "a.local", 22)];
        let bad = vec![
            raw("a.local", b"junk".to_vec()),
            raw("a.local", txt("rey")[..5].to_vec()),
            raw("-oProxyCommand=x", txt("rey")),
            raw("", txt("rey")),
        ];
        assert_eq!(attach(list.clone(), bad)[0].pairing, None);
        let two = attach(list, vec![raw("a.local", txt("first")), raw("a.local", txt("second"))]);
        assert_eq!(two[0].pairing.as_ref().unwrap().account, "first");
    }

    #[test]
    fn a_host_from_the_network_is_checked_and_lowercased() {
        assert_eq!(found_from("Studio", "Studio-Mac.local.", 22), Some(found("Studio", "studio-mac.local", 22)));
        assert_eq!(found_from("x", "-oProxyCommand=x", 22), None);
        assert_eq!(found_from("x", "a b.local", 22), None);
        assert_eq!(found_from("x", "host.local", 0), None);
        assert_eq!(found_from("x", "", 22), None);
    }

    #[test]
    fn a_name_is_cleaned_and_falls_back_to_the_host() {
        assert_eq!(found_from("  Of\u{7}fice \n", "office.local", 22).unwrap().name, "Office");
        assert_eq!(found_from("\u{7}", "office.local", 22).unwrap().name, "office.local");
        assert_eq!(found_from(&"n".repeat(500), "office.local", 22).unwrap().name.chars().count() <= super::super::peers::MAX_NAME, true);
    }

    #[test]
    fn this_device_and_repeats_are_dropped_and_the_rest_is_sorted() {
        let list = vec![
            found("zeta", "zeta.local", 22),
            found("Me", "me.local", 22),
            found("alpha", "alpha.local", 22),
            found("alpha", "alpha.local", 22),
            found("Alpha again", "alpha.local", 2222),
        ];
        let names: Vec<_> = finish(list, Some("Me.local.")).into_iter().map(|item| (item.name, item.port)).collect();
        assert_eq!(
            names,
            vec![("alpha".to_owned(), 22), ("Alpha again".to_owned(), 2222), ("zeta".to_owned(), 22)]
        );
    }

    /// A manual check on the real network: `cargo test discovery_manual -- --ignored --nocapture`.
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore]
    fn discovery_manual() {
        let started = std::time::Instant::now();
        let found = discover();
        println!("found {:?} devices in {:?}", found.as_ref().map(Vec::len), started.elapsed());
        assert!(found.is_ok());
    }

    #[test]
    fn nothing_is_dropped_when_this_device_has_no_host() {
        assert_eq!(finish(vec![found("a", "a.local", 22)], None).len(), 1);
    }
}
