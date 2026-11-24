//! The one service that this device announces: a pairing window that is open.
//!
//! While a window is open, this device registers `_lazyworker-pair._tcp` through the system
//! `dns_sd` API, so a device on the same network can fill in the account by itself. The record
//! carries the account name and the device id, which are no secret on the local network. It never
//! carries the code, and the 6-digit code that SPAKE2 checks is still needed to pair: a forged
//! record can at most fill a wrong account into a form, and it cannot pair. The registration ends
//! when the window ends, for any reason, because the `Registration` is dropped.

use super::{destination::valid_user, identity::DeviceId};

/// The Bonjour type of a pairing window.
pub const SERVICE_TYPE: &str = "_lazyworker-pair._tcp";
/// The version of the record. A record of another version is not read.
const VERSION: &str = "1";
/// The longest name of a Bonjour service, in bytes.
const MAX_NAME: usize = 63;

/// What a pairing window announces.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Advert {
    pub account: String,
    pub id: String,
}

/// The TXT record: `v=1`, `acct=<account>`, `id=<device id>`, each as one length-prefixed string.
pub fn encode_txt(advert: &Advert) -> Result<Vec<u8>, &'static str> {
    if !valid_user(&advert.account) || DeviceId::parse(&advert.id).is_err() {
        return Err("advertInvalid");
    }
    let mut out = Vec::new();
    for entry in [
        format!("v={VERSION}"),
        format!("acct={}", advert.account),
        format!("id={}", advert.id),
    ] {
        out.push(u8::try_from(entry.len()).map_err(|_| "advertInvalid")?);
        out.extend_from_slice(entry.as_bytes());
    }
    Ok(out)
}

/// Reads a TXT record that came from the network. It is nothing unless the version is 1 and the
/// account and the id are valid. A key that is not known is ignored, and a key that repeats is not valid.
pub fn decode_txt(bytes: &[u8]) -> Option<Advert> {
    let (mut version, mut account, mut id) = (None, None, None);
    let mut rest = bytes;
    while let Some((&length, tail)) = rest.split_first() {
        let entry = tail.get(..usize::from(length))?;
        rest = &tail[usize::from(length)..];
        let Ok(entry) = std::str::from_utf8(entry) else { return None };
        let Some((key, value)) = entry.split_once('=') else { continue };
        let slot = match key.to_ascii_lowercase().as_str() {
            "v" => &mut version,
            "acct" => &mut account,
            "id" => &mut id,
            _ => continue,
        };
        if slot.replace(value.to_owned()).is_some() {
            return None;
        }
    }
    let (account, id) = (account?, id?);
    if version.as_deref() != Some(VERSION) || !valid_user(&account) {
        return None;
    }
    DeviceId::parse(&id).ok()?;
    Some(Advert { account, id })
}

/// A live registration. Dropping it deregisters the service.
pub type Registration = Box<dyn Send>;

/// Registers the service. The system one talks to `mDNSResponder`, and a test replaces it.
pub trait Advertiser: Send + Sync {
    /// Registers the service under `name`, or returns nothing when that failed. The announcement
    /// is only a help, so a failure never stops a pairing window.
    fn register(&self, name: &str, advert: &Advert) -> Option<Registration>;
}

/// The advertiser of the system.
pub struct System;

impl Advertiser for System {
    fn register(&self, name: &str, advert: &Advert) -> Option<Registration> {
        let txt = encode_txt(advert).ok()?;
        system::register(name, &txt)
    }
}

/// An advertiser that announces nothing.
#[cfg(test)]
pub struct Silent;

#[cfg(test)]
impl Advertiser for Silent {
    fn register(&self, _: &str, _: &Advert) -> Option<Registration> {
        None
    }
}

/// The name as a Bonjour instance name: at most 63 bytes, cut at a character.
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
fn instance_name(name: &str) -> String {
    let mut end = name.len().min(MAX_NAME);
    while !name.is_char_boundary(end) {
        end -= 1;
    }
    name[..end].to_owned()
}

#[cfg(target_os = "macos")]
mod system {
    use super::{instance_name, Registration, SERVICE_TYPE};
    use std::{
        ffi::{c_char, c_void, CString},
        ptr,
    };

    type Ref = *mut c_void;
    type Reply = extern "C" fn(Ref, u32, i32, *const c_char, *const c_char, *const c_char, *mut c_void);

    extern "C" {
        fn DNSServiceRegister(
            sd_ref: *mut Ref,
            flags: u32,
            interface: u32,
            name: *const c_char,
            regtype: *const c_char,
            domain: *const c_char,
            host: *const c_char,
            port: u16,
            txt_len: u16,
            txt: *const c_void,
            callback: Option<Reply>,
            context: *mut c_void,
        ) -> i32;
        fn DNSServiceRefDeallocate(sd_ref: Ref);
    }

    const SSH_PORT: u16 = 22;

    /// One registered `DNSServiceRef`, deallocated once when it drops.
    struct Registered(Ref);

    // SAFETY: the ref has no callback and nobody reads its socket. The only call that uses it again
    // is the deallocation in `drop`, which runs on one thread at a time.
    unsafe impl Send for Registered {}

    impl Drop for Registered {
        fn drop(&mut self) {
            // SAFETY: the ref came from a call that succeeded and is deallocated here only.
            unsafe { DNSServiceRefDeallocate(self.0) };
        }
    }

    pub fn register(name: &str, txt: &[u8]) -> Option<Registration> {
        let regtype = CString::new(SERVICE_TYPE).ok()?;
        // A name that cannot be a C string falls back to the computer name, which the system picks.
        let name = CString::new(instance_name(name)).ok().filter(|name| !name.as_bytes().is_empty());
        let mut reference: Ref = ptr::null_mut();
        // SAFETY: every pointer is valid for the call, and the system copies the strings and the
        // record. There is no callback, so there is no context. The port is in network byte order.
        let code = unsafe {
            DNSServiceRegister(
                &mut reference,
                0,
                0,
                name.as_ref().map_or(ptr::null(), |name| name.as_ptr()),
                regtype.as_ptr(),
                ptr::null(),
                ptr::null(),
                SSH_PORT.to_be(),
                u16::try_from(txt.len()).ok()?,
                txt.as_ptr().cast(),
                None,
                ptr::null_mut(),
            )
        };
        if code != 0 || reference.is_null() {
            return None;
        }
        Some(Box::new(Registered(reference)))
    }
}

#[cfg(not(target_os = "macos"))]
mod system {
    use super::Registration;

    pub fn register(_: &str, _: &[u8]) -> Option<Registration> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "0123456789abcdef0123456789abcdef";

    fn advert(account: &str) -> Advert {
        Advert { account: account.into(), id: ID.into() }
    }

    #[test]
    fn the_record_has_three_strings_and_nothing_secret() {
        let bytes = encode_txt(&advert("rey")).unwrap();
        let mut expected = Vec::new();
        for entry in ["v=1", "acct=rey", &format!("id={ID}")] {
            expected.push(entry.len() as u8);
            expected.extend_from_slice(entry.as_bytes());
        }
        assert_eq!(bytes, expected);
        assert_eq!(decode_txt(&bytes), Some(advert("rey")));
    }

    #[test]
    fn an_account_or_id_that_is_not_valid_is_not_encoded() {
        assert!(encode_txt(&advert("")).is_err());
        assert!(encode_txt(&advert("a b")).is_err());
        assert!(encode_txt(&advert("-oProxyCommand")).is_err());
        assert!(encode_txt(&Advert { account: "rey".into(), id: "abc".into() }).is_err());
    }

    fn record(entries: &[&str]) -> Vec<u8> {
        entries.iter().flat_map(|e| std::iter::once(e.len() as u8).chain(e.bytes())).collect()
    }

    #[test]
    fn a_record_from_the_network_is_checked() {
        let id = format!("id={ID}");
        assert_eq!(decode_txt(&record(&["v=1", "acct=rey", &id])), Some(advert("rey")));
        // Another key is ignored, and the order does not matter.
        assert_eq!(decode_txt(&record(&[&id, "x=1", "acct=rey", "v=1", "flag"])), Some(advert("rey")));
        // Version, account and id must be there and valid.
        assert_eq!(decode_txt(&record(&["acct=rey", &id])), None);
        assert_eq!(decode_txt(&record(&["v=2", "acct=rey", &id])), None);
        assert_eq!(decode_txt(&record(&["v=1", &id])), None);
        assert_eq!(decode_txt(&record(&["v=1", "acct=rey"])), None);
        assert_eq!(decode_txt(&record(&["v=1", "acct=re y", &id])), None);
        assert_eq!(decode_txt(&record(&["v=1", "acct=-rey", &id])), None);
        assert_eq!(decode_txt(&record(&["v=1", "acct=rey", "id=ZZ"])), None);
        assert_eq!(decode_txt(&record(&["v=1", "acct=rey", &format!("id={}", ID.to_uppercase())])), None);
        // A repeated key is not valid.
        assert_eq!(decode_txt(&record(&["v=1", "acct=rey", "acct=root", &id])), None);
        // A string that is cut short, or not UTF-8, is not valid.
        assert_eq!(decode_txt(&[9, b'v', b'=']), None);
        assert_eq!(decode_txt(&[2, 0xff, 0xfe]), None);
        assert_eq!(decode_txt(&[]), None);
    }

    #[test]
    fn the_instance_name_is_cut_at_a_character() {
        assert_eq!(instance_name("Studio"), "Studio");
        let long = "é".repeat(60);
        let cut = instance_name(&long);
        assert!(cut.len() <= MAX_NAME && cut.chars().all(|c| c == 'é'));
    }

    /// A manual check on this Mac, with `dns-sd -B _lazyworker-pair._tcp local.` open in another
    /// terminal: `cargo test advert_manual -- --ignored --nocapture`.
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore]
    fn advert_manual() {
        let registration = System.register("Lazy Worker manual check", &advert("rey"));
        println!("registered: {}", registration.is_some());
        assert!(registration.is_some());
        std::thread::sleep(std::time::Duration::from_secs(
            std::env::var("ADVERT_SECS").ok().and_then(|s| s.parse().ok()).unwrap_or(6),
        ));
        drop(registration);
        println!("deregistered");
        std::thread::sleep(std::time::Duration::from_secs(
            std::env::var("ADVERT_SECS").ok().and_then(|s| s.parse().ok()).unwrap_or(6),
        ));
    }
}
