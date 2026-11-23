//! What both sides of a pairing share: the messages, their checks, and the keys.
//!
//! The protocol is `lw-pair-1`, in JSON lines with binary values in base64. The two devices run
//! SPAKE2 on the 6-digit code, so the code is proved and never sent. Each side then proves the
//! shared key with a MAC over a hash of everything that was said: the messages, both device
//! identities, and the host key of the target. A change to any of it breaks the MAC.
//!
//! RustCrypto's `spake2` crate has not had an independent audit.

use super::{
    destination::valid_user,
    identity::{parse_public_key, DeviceId},
    peers::clean_name,
    wire::{read_line, LineError, MAX_REQUEST_LINE},
};
use crate::devices::Kind;
use base64::{engine::general_purpose::STANDARD, Engine};
use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use sha2::{Digest, Sha256};
use spake2::{Ed25519Group, Identity, Password, Spake2};
use std::{
    io::{BufRead, Write},
    sync::Arc,
    time::Duration,
};

pub const VERSION: &str = "lw-pair-1";
pub const CODE_LENGTH: usize = 6;
/// The longest a pairing session lasts, on either side.
pub const SESSION_TIMEOUT: Duration = Duration::from_secs(60);
/// A SPAKE2 message on the ed25519 group: one byte for the side and a 32-byte point.
const PAKE_LENGTH: usize = 33;
const MAC_LENGTH: usize = 32;
const CONTROLLER_IDENTITY: &[u8] = b"lazy-worker controller";
const TARGET_IDENTITY: &[u8] = b"lazy-worker target";
const CONTROLLER_INFO: &[u8] = b"lw-pair-1 controller";
const TARGET_INFO: &[u8] = b"lw-pair-1 target";
/// Longest text of a base64 field of a message, in characters.
const MAX_BASE64: usize = 64;
const MAX_HOST_KEY: usize = 256;
const MAX_KEY: usize = 256;

/// The error codes that the other side may name. Any other text is read as `pairingFailed`.
const KNOWN_ERRORS: [&str; 16] = [
    "pairingClosed",
    "pairingBusy",
    "wrongCode",
    "pairingSaveFailed",
    "pairingSelf",
    "badMessage",
    "badHello",
    "lineTooLong",
    "peersFull",
    "appNotRunning",
    "remoteUnavailable",
    "commandNotAllowed",
    "argumentsInvalid",
    "responseTooLong",
    "notPaired",
    "pairingFailed",
];

/// This device as a party of a pairing. The id and the key come from the identity.
#[derive(Clone, Debug)]
pub struct Own {
    pub id: DeviceId,
    pub name: String,
    pub kind: Kind,
    pub public_key: String,
}

/// Reads this device. It may run `ssh-keygen` and `tmutil`, so call it off the main thread.
pub type OwnDevice = Arc<dyn Fn() -> Result<Own, &'static str> + Send + Sync>;

/// The controller, as the target learns it in the first message.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ControllerInfo {
    pub id: String,
    pub name: String,
    pub kind: Kind,
    pub public_key: String,
}

/// The target, as the controller learns it in the reply.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TargetInfo {
    pub id: String,
    pub name: String,
    pub kind: Kind,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StartMessage {
    pub start: Start,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Start {
    pub version: String,
    pub pake: String,
    pub device: ControllerInfo,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplyMessage {
    pub reply: Reply,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Reply {
    pub pake: String,
    pub device: TargetInfo,
    /// The target's own `ssh-ed25519` host key, with no comment.
    pub host_key: String,
    /// The account on the target that the controller connects as.
    pub account: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfirmMessage {
    pub confirm: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ErrorMessage {
    error: String,
}

pub fn valid_code(code: &str) -> bool {
    code.len() == CODE_LENGTH && code.bytes().all(|byte| byte.is_ascii_digit())
}

fn valid_name(name: &str) -> bool {
    !name.is_empty() && name == clean_name(name)
}

impl ControllerInfo {
    pub fn validate(&self) -> Result<(), &'static str> {
        let key = parse_public_key(&self.public_key).map_err(|_| "badMessage")?;
        let valid = DeviceId::parse(&self.id).is_ok()
            && valid_name(&self.name)
            && key == self.public_key
            && self.public_key.len() <= MAX_KEY;
        valid.then_some(()).ok_or("badMessage")
    }
}

impl TargetInfo {
    pub fn validate(&self) -> Result<(), &'static str> {
        (DeviceId::parse(&self.id).is_ok() && valid_name(&self.name))
            .then_some(())
            .ok_or("badMessage")
    }
}

impl Reply {
    pub fn validate(&self) -> Result<(), &'static str> {
        self.device.validate()?;
        let key = parse_public_key(&self.host_key).map_err(|_| "badMessage")?;
        let valid = key == self.host_key
            && self.host_key.len() <= MAX_HOST_KEY
            && valid_user(&self.account);
        valid.then_some(()).ok_or("badMessage")
    }
}

pub fn encode(bytes: &[u8]) -> String {
    STANDARD.encode(bytes)
}

/// Standard base64 of exactly `length` bytes, and nothing else.
fn decode(text: &str, length: usize) -> Result<Vec<u8>, &'static str> {
    if text.len() > MAX_BASE64 {
        return Err("badMessage");
    }
    match STANDARD.decode(text) {
        Ok(bytes) if bytes.len() == length => Ok(bytes),
        _ => Err("badMessage"),
    }
}

pub fn decode_pake(text: &str) -> Result<Vec<u8>, &'static str> {
    decode(text, PAKE_LENGTH)
}

pub fn decode_mac(text: &str) -> Result<Vec<u8>, &'static str> {
    decode(text, MAC_LENGTH)
}

/// The controller plays side A.
pub fn start_controller(code: &str) -> (Spake2<Ed25519Group>, Vec<u8>) {
    Spake2::<Ed25519Group>::start_a(
        &Password::new(code.as_bytes()),
        &Identity::new(CONTROLLER_IDENTITY),
        &Identity::new(TARGET_IDENTITY),
    )
}

/// The target plays side B.
pub fn start_target(code: &str) -> (Spake2<Ed25519Group>, Vec<u8>) {
    Spake2::<Ed25519Group>::start_b(
        &Password::new(code.as_bytes()),
        &Identity::new(CONTROLLER_IDENTITY),
        &Identity::new(TARGET_IDENTITY),
    )
}

/// One key for each direction, so a confirmation cannot be sent back as the other one.
pub struct Keys {
    controller: [u8; 32],
    target: [u8; 32],
}

pub fn derive(shared: &[u8]) -> Keys {
    let hkdf = Hkdf::<Sha256>::new(None, shared);
    let mut keys = Keys {
        controller: [0; 32],
        target: [0; 32],
    };
    // 32 bytes is far below the limit of HKDF-SHA256, so these cannot fail.
    let _ = hkdf.expand(CONTROLLER_INFO, &mut keys.controller);
    let _ = hkdf.expand(TARGET_INFO, &mut keys.target);
    keys
}

#[derive(Clone, Copy)]
pub enum Side {
    Controller,
    Target,
}

impl Keys {
    fn mac(&self, side: Side, hash: &[u8; 32]) -> Hmac<Sha256> {
        let key = match side {
            Side::Controller => &self.controller,
            Side::Target => &self.target,
        };
        let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC takes any key length");
        mac.update(hash);
        mac
    }

    /// The confirmation of one side.
    pub fn confirm(&self, side: Side, hash: &[u8; 32]) -> Vec<u8> {
        self.mac(side, hash).finalize().into_bytes().to_vec()
    }

    /// Checks the confirmation of one side, in constant time.
    pub fn verify(&self, side: Side, hash: &[u8; 32], tag: &[u8]) -> bool {
        self.mac(side, hash).verify_slice(tag).is_ok()
    }
}

fn field(hash: &mut Sha256, bytes: &[u8]) {
    hash.update((bytes.len() as u32).to_be_bytes());
    hash.update(bytes);
}

fn kind_word(kind: Kind) -> &'static str {
    match kind {
        Kind::Laptop => "laptop",
        Kind::Desktop => "desktop",
    }
}

/// The hash of everything both sides said, in a canonical form: every field has a 4-byte length
/// before it, so two different sets of fields never give the same bytes.
pub fn transcript(
    controller_pake: &[u8],
    target_pake: &[u8],
    controller: &ControllerInfo,
    target: &TargetInfo,
    host_key: &str,
    account: &str,
) -> [u8; 32] {
    let mut hash = Sha256::new();
    for bytes in [
        VERSION.as_bytes(),
        controller_pake,
        target_pake,
        controller.id.as_bytes(),
        controller.name.as_bytes(),
        kind_word(controller.kind).as_bytes(),
        controller.public_key.as_bytes(),
        target.id.as_bytes(),
        target.name.as_bytes(),
        kind_word(target.kind).as_bytes(),
        host_key.as_bytes(),
        account.as_bytes(),
    ] {
        field(&mut hash, bytes);
    }
    hash.finalize().into()
}

pub fn send(writer: &mut impl Write, message: &impl Serialize) -> Result<(), &'static str> {
    let mut text = serde_json::to_string(message).map_err(|_| "pairingFailed")?;
    text.push('\n');
    writer
        .write_all(text.as_bytes())
        .and_then(|()| writer.flush())
        .map_err(|_| "pairingFailed")
}

/// Reads one message. An `{"error":...}` line from the other side becomes its code when the
/// code is one that we know. Anything that does not fit the type is refused.
pub fn receive<T: DeserializeOwned>(reader: &mut impl BufRead) -> Result<T, &'static str> {
    let line = match read_line(reader, MAX_REQUEST_LINE) {
        Ok(Some(line)) => line,
        Ok(None) | Err(LineError::Io) => return Err("pairingFailed"),
        Err(LineError::TooLong) => return Err("lineTooLong"),
    };
    if let Ok(ErrorMessage { error }) = serde_json::from_slice(&line) {
        return Err(KNOWN_ERRORS
            .iter()
            .find(|known| **known == error)
            .copied()
            .unwrap_or("pairingFailed"));
    }
    serde_json::from_slice(&line).map_err(|_| "badMessage")
}

#[cfg(test)]
pub(crate) mod testing {
    use super::*;
    use crate::remote::peers::testing::{ID_A, ID_B, KEY};

    pub const HOST_KEY: &str =
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIDiFbQEPp+zd0e6F/i0IEqDDrxYfKTFowPI3v8NSyB+I";
    pub const OTHER_HOST_KEY: &str =
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAID8ZXmiaFqom1COnlczolZXMUWPTJ9+G8VXLl67Q9FJ9";

    pub fn controller() -> ControllerInfo {
        ControllerInfo {
            id: ID_A.into(),
            name: "Laptop".into(),
            kind: Kind::Laptop,
            public_key: KEY.into(),
        }
    }

    pub fn target() -> TargetInfo {
        TargetInfo {
            id: ID_B.into(),
            name: "Studio".into(),
            kind: Kind::Desktop,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{testing::*, *};
    use std::io::Cursor;

    #[test]
    fn a_code_is_six_ascii_digits() {
        assert!(valid_code("000000") && valid_code("123456"));
        for bad in ["", "12345", "1234567", "12345a", "１２３４５６", " 12345", "12 456", "-12345"] {
            assert!(!valid_code(bad), "{bad:?}");
        }
    }

    #[test]
    fn the_same_code_gives_the_same_keys_and_another_code_does_not() {
        let (a, message_a) = start_controller("123456");
        let (b, message_b) = start_target("123456");
        let (key_a, key_b) = (a.finish(&message_b).unwrap(), b.finish(&message_a).unwrap());
        assert_eq!(key_a, key_b);
        assert_eq!(message_a.len(), PAKE_LENGTH);
        let (wrong, message_w) = start_controller("123457");
        let (b, _) = start_target("123456");
        let key_w = wrong.finish(&message_b).unwrap();
        assert_ne!(key_w, b.finish(&message_w).unwrap());
    }

    #[test]
    fn the_confirmations_are_bound_to_their_side_and_to_the_transcript() {
        let keys = derive(b"shared key");
        let hash = transcript(&[1; 33], &[2; 33], &controller(), &target(), HOST_KEY, "me");
        let tag = keys.confirm(Side::Controller, &hash);
        assert!(keys.verify(Side::Controller, &hash, &tag));
        assert!(!keys.verify(Side::Target, &hash, &tag), "no reflection");
        assert!(!keys.verify(Side::Controller, &[0; 32], &tag));
        assert!(!keys.verify(Side::Controller, &hash, &tag[..31]));
        assert!(!derive(b"other key").verify(Side::Controller, &hash, &tag));
    }

    #[test]
    fn every_field_changes_the_transcript() {
        let base = |c: &ControllerInfo, t: &TargetInfo, host: &str, account: &str| {
            transcript(&[1; 33], &[2; 33], c, t, host, account)
        };
        let mut seen = vec![base(&controller(), &target(), HOST_KEY, "me")];
        let mut changed = |hash: [u8; 32]| {
            assert!(!seen.contains(&hash));
            seen.push(hash);
        };
        let mut c = controller();
        c.name = "Laptop2".into();
        changed(base(&c, &target(), HOST_KEY, "me"));
        let mut c = controller();
        c.kind = Kind::Desktop;
        changed(base(&c, &target(), HOST_KEY, "me"));
        let mut c = controller();
        c.id = "f".repeat(32);
        changed(base(&c, &target(), HOST_KEY, "me"));
        let mut t = target();
        t.name = "Other".into();
        changed(base(&controller(), &t, HOST_KEY, "me"));
        changed(base(&controller(), &target(), OTHER_HOST_KEY, "me"));
        changed(base(&controller(), &target(), HOST_KEY, "you"));
        changed(transcript(&[3; 33], &[2; 33], &controller(), &target(), HOST_KEY, "me"));
        changed(transcript(&[1; 33], &[3; 33], &controller(), &target(), HOST_KEY, "me"));
        // The lengths keep a boundary from moving: "ab" and "c" are not "a" and "bc".
        let (mut a, mut b) = (controller(), controller());
        a.name = "ab".into();
        a.public_key = "c".into();
        b.name = "a".into();
        b.public_key = "bc".into();
        assert_ne!(
            base(&a, &target(), HOST_KEY, "me"),
            base(&b, &target(), HOST_KEY, "me")
        );
    }

    #[test]
    fn messages_are_checked_in_every_field() {
        assert!(controller().validate().is_ok() && target().validate().is_ok());
        let mut c = controller();
        c.id = "short".into();
        assert!(c.validate().is_err());
        for name in ["", "  ", "a\nb", " padded "] {
            let mut c = controller();
            c.name = name.into();
            assert!(c.validate().is_err(), "{name:?}");
        }
        let mut c = controller();
        c.public_key = format!("{} comment", c.public_key);
        assert!(c.validate().is_err());
        c.public_key = "ssh-rsa AAAA".into();
        assert!(c.validate().is_err());
        let reply = |host_key: &str, account: &str| Reply {
            pake: String::new(),
            device: target(),
            host_key: host_key.into(),
            account: account.into(),
        };
        assert!(reply(HOST_KEY, "me").validate().is_ok());
        assert!(reply("ssh-rsa AAAA", "me").validate().is_err());
        assert!(reply(&format!("{HOST_KEY} x"), "me").validate().is_err());
        for account in ["", "-oProxyCommand=x", "a b", "a;b", "a\nb"] {
            assert!(reply(HOST_KEY, account).validate().is_err(), "{account:?}");
        }
    }

    #[test]
    fn binary_fields_must_be_exact_base64() {
        let good = encode(&[7; 33]);
        assert_eq!(decode_pake(&good).unwrap(), vec![7; 33]);
        assert!(decode_pake(&encode(&[7; 32])).is_err());
        assert!(decode_pake(&encode(&[7; 34])).is_err());
        assert!(decode_pake("***").is_err());
        assert!(decode_pake(&"A".repeat(5000)).is_err());
        assert!(decode_mac(&encode(&[1; 32])).is_ok());
        assert!(decode_mac(&encode(&[1; 31])).is_err());
    }

    #[test]
    fn received_lines_are_typed_bounded_and_strict() {
        let read =
            |text: &str| receive::<ConfirmMessage>(&mut Cursor::new(text.as_bytes().to_vec()));
        assert_eq!(read("{\"confirm\":\"x\"}\n").unwrap().confirm, "x");
        assert_eq!(read("{\"confirm\":\"x\",\"more\":1}\n").err(), Some("badMessage"));
        assert_eq!(read("not json\n").err(), Some("badMessage"));
        assert_eq!(read("{}\n").err(), Some("badMessage"));
        assert_eq!(read("").err(), Some("pairingFailed"));
        assert_eq!(read("{\"error\":\"wrongCode\"}\n").err(), Some("wrongCode"));
        assert_eq!(read("{\"error\":\"<script>\"}\n").err(), Some("pairingFailed"));
        let huge = format!("{{\"confirm\":\"{}\"}}\n", "a".repeat(MAX_REQUEST_LINE));
        assert_eq!(read(&huge).err(), Some("lineTooLong"));
    }
}
