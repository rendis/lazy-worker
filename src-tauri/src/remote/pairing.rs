//! The target side of pairing: a short window with a code, and the session that checks it.
//!
//! While a window is open, one extra `authorized_keys` line lets the fixed pairing key run
//! `remote --pair`. That key is public, so the line is the door and the 6-digit code is the lock.
//! The window closes on its first success, when its attempts run out, at its end, or when the user
//! closes it, and the line goes with it. Nothing is saved before the controller proved the code.

use super::{
    advert::{self, Advert, Advertiser, Registration},
    authorized_keys::{self, AuthorizedKeys},
    identity::{parse_public_key, DeviceId},
    log::{Entry, Log},
    now,
    pake::{
        self, ConfirmMessage, ControllerInfo, OwnDevice, Reply, ReplyMessage, Side, StartMessage,
        TargetInfo,
    },
    peers::{Peer, PeerPermissions, Peers},
    read_small,
    wire::error_line,
};
use crate::devices::Pairing as State;
use rand_core::{OsRng, RngCore};
use serde::Serialize;
use std::{
    io::{BufRead, Write},
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Condvar, Mutex, MutexGuard,
    },
    thread,
    time::{Duration, Instant},
};

/// How long a window stays open.
pub const WINDOW: Duration = Duration::from_secs(10 * 60);
/// The wrong codes that one window allows.
pub const ATTEMPTS: u8 = 3;
/// The key that sshd uses for this device. Anyone can read it.
pub const HOST_KEY_FILE: &str = "/etc/ssh/ssh_host_ed25519_key.pub";
/// `expiry-time` of the line is the window end plus this much.
const GRACE: u64 = 60;

/// What `open` tells the user to show.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Opened {
    pub code: String,
    /// The account that the other device connects as.
    pub account: String,
    /// Unix seconds.
    pub expires: u64,
}

#[derive(Clone)]
struct Window {
    code: String,
    permissions: PeerPermissions,
    host_key: String,
    account: String,
    ends: u64,
    deadline: Instant,
    attempts: u8,
    generation: u64,
}

#[derive(Default)]
struct Inner {
    window: Option<Window>,
    generation: u64,
}

pub struct Pairing {
    peers: Arc<Peers>,
    log: Arc<Log>,
    keys: Result<AuthorizedKeys, &'static str>,
    own: OwnDevice,
    host_key_file: PathBuf,
    account: fn() -> Option<String>,
    length: Duration,
    state: Mutex<Inner>,
    /// Wakes the timer of a window when the window changes.
    changed: Condvar,
    busy: AtomicBool,
    /// Announces the open window on the local network, and only while it is open.
    advertiser: Arc<dyn Advertiser>,
    registration: Mutex<Option<Registration>>,
}

/// Holds the one session slot. Frees it however the session ends.
struct Busy<'a>(&'a AtomicBool);

impl<'a> Busy<'a> {
    fn take(flag: &'a AtomicBool) -> Option<Self> {
        flag.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .ok()
            .map(|_| Self(flag))
    }
}

impl Drop for Busy<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

/// Six digits, each of them equally likely. A value from the top of the range is dropped, so
/// the remainder of the division does not favor the low digits.
fn digits(mut next: impl FnMut() -> Result<u32, &'static str>) -> Result<String, &'static str> {
    const SPACE: u32 = 1_000_000;
    const LIMIT: u32 = u32::MAX - u32::MAX % SPACE;
    loop {
        let value = next()?;
        if value < LIMIT {
            return Ok(format!("{:06}", value % SPACE));
        }
    }
}

fn random_u32() -> Result<u32, &'static str> {
    let mut bytes = [0u8; 4];
    OsRng
        .try_fill_bytes(&mut bytes)
        .map_err(|_| "randomFailed")?;
    Ok(u32::from_be_bytes(bytes))
}

/// The user of this process.
pub fn current_account() -> Option<String> {
    super::access::current_user()
}

/// `expiry-time` wants local time, to the minute.
fn local_expiry(unix: u64) -> Result<String, &'static str> {
    use chrono::TimeZone;
    let moment = i64::try_from(unix)
        .ok()
        .and_then(|seconds| chrono::Local.timestamp_opt(seconds, 0).single())
        .ok_or("expiryInvalid")?;
    Ok(authorized_keys::expiry_time(&moment))
}

impl Pairing {
    pub fn new(
        peers: Arc<Peers>,
        log: Arc<Log>,
        keys: Result<AuthorizedKeys, &'static str>,
        own: OwnDevice,
    ) -> Self {
        Self {
            peers,
            log,
            keys,
            own,
            host_key_file: PathBuf::from(HOST_KEY_FILE),
            account: current_account,
            length: WINDOW,
            state: Mutex::new(Inner::default()),
            changed: Condvar::new(),
            busy: AtomicBool::new(false),
            advertiser: Arc::new(advert::System),
            registration: Mutex::new(None),
        }
    }

    #[cfg(test)]
    pub fn for_test(
        peers: Arc<Peers>,
        log: Arc<Log>,
        keys: Result<AuthorizedKeys, &'static str>,
        own: OwnDevice,
        host_key_file: PathBuf,
        length: Duration,
    ) -> Self {
        Self {
            host_key_file,
            account: || Some("me".to_owned()),
            length,
            // A test announces nothing on the network unless it asks for it.
            advertiser: Arc::new(advert::Silent),
            ..Self::new(peers, log, keys, own)
        }
    }

    #[cfg(test)]
    pub fn with_advertiser(self, advertiser: Arc<dyn Advertiser>) -> Self {
        Self { advertiser, ..self }
    }

    /// Makes the announcement match the window: registered while a window is open, gone otherwise.
    /// Every place that opens or ends a window calls it after the window changed, so the registration
    /// can never outlive the window. The registration lock is taken before the state lock, and no
    /// code takes them the other way round.
    fn sync_advert(&self) {
        let mut registration = self
            .registration
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let Ok(window) = self.current() else {
            // Dropping the registration deregisters the service.
            registration.take();
            return;
        };
        if registration.is_some() {
            return;
        }
        if let Ok(own) = (self.own)() {
            *registration = self.advertiser.register(
                &own.name,
                &Advert {
                    account: window.account,
                    id: own.id.as_str().to_owned(),
                },
            );
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.state.lock().unwrap_or_else(|error| error.into_inner())
    }

    /// Opens a window, or replaces the open one with a new code and fresh attempts. Reads the
    /// system and files, so call it off the main thread.
    pub fn open(
        self: &Arc<Self>,
        permissions: PeerPermissions,
    ) -> Result<Opened, &'static str> {
        let keys = self.keys.as_ref().map_err(|code| *code)?;
        let account = (self.account)().ok_or("accountUnknown")?;
        let host_key = read_small(&self.host_key_file)?
            .ok_or("hostKeyMissing")
            .and_then(|text| parse_public_key(&text).map_err(|_| "hostKeyInvalid"))?;
        let code = digits(random_u32)?;
        let ends = now() + self.length.as_secs();
        keys.install_pairing(&self.peers.networks(), &local_expiry(ends + GRACE)?)?;
        let generation = {
            let mut state = self.lock();
            state.generation += 1;
            let generation = state.generation;
            state.window = Some(Window {
                code: code.clone(),
                // The shell is never granted by pairing.
                permissions: PeerPermissions {
                    shell: false,
                    ..permissions
                },
                host_key,
                account: account.clone(),
                ends,
                deadline: Instant::now() + self.length,
                attempts: ATTEMPTS,
                generation,
            });
            self.changed.notify_all();
            generation
        };
        self.sync_advert();
        let timer = Arc::clone(self);
        let spawned = thread::Builder::new()
            .name("remote-pairing-timer".into())
            .spawn(move || timer.expire(generation));
        if spawned.is_err() {
            let _ = self.close();
            return Err("remoteIo");
        }
        Ok(Opened {
            code,
            account,
            expires: ends,
        })
    }

    /// Closes the window of `generation` when its time is up.
    fn expire(&self, generation: u64) {
        let mut state = self.lock();
        loop {
            let Some(window) = state.window.as_ref().filter(|w| w.generation == generation)
            else {
                return;
            };
            let left = window.deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            state = self
                .changed
                .wait_timeout(state, left)
                .unwrap_or_else(|error| error.into_inner())
                .0;
        }
        state.window = None;
        state.generation += 1;
        drop(state);
        self.changed.notify_all();
        self.sync_advert();
        let _ = self.remove_line();
    }

    fn remove_line(&self) -> Result<(), &'static str> {
        match &self.keys {
            Ok(keys) => keys.remove_pairing(),
            Err(_) => Ok(()),
        }
    }

    /// Closes the window, if one is open, and removes its line. The line goes in any case, so a
    /// stale one is cleaned up too.
    pub fn close(&self) -> Result<(), &'static str> {
        {
            let mut state = self.lock();
            state.window = None;
            state.generation += 1;
        }
        self.changed.notify_all();
        self.sync_advert();
        self.remove_line()
    }

    pub fn state(&self) -> State {
        match &self.lock().window {
            Some(window) if Instant::now() < window.deadline => State {
                open: true,
                expires: Some(window.ends),
                attempts_left: window.attempts,
            },
            _ => State::closed(),
        }
    }

    /// The window as a session sees it, or the reason that there is none.
    fn current(&self) -> Result<Window, &'static str> {
        match &self.lock().window {
            Some(window) if Instant::now() < window.deadline => Ok(window.clone()),
            _ => Err("pairingClosed"),
        }
    }

    /// Serves one pairing session on a connection. A failure is sent as one error line.
    pub fn serve(&self, reader: &mut impl BufRead, writer: &mut impl Write) {
        if let Err(code) = self.session(reader, writer) {
            let _ = writer
                .write_all(error_line(code).as_bytes())
                .and_then(|()| writer.flush());
        }
    }

    fn session(
        &self,
        reader: &mut impl BufRead,
        writer: &mut impl Write,
    ) -> Result<(), &'static str> {
        let window = self.current()?;
        let _slot = Busy::take(&self.busy).ok_or("pairingBusy")?;

        let StartMessage { start } = pake::receive(reader)?;
        if start.version != pake::VERSION {
            return Err("badMessage");
        }
        start.device.validate()?;
        let controller_pake = pake::decode_pake(&start.pake)?;
        let own = (self.own)()?;
        if start.device.id == own.id.as_str() {
            return Err("pairingSelf");
        }
        let target = TargetInfo {
            id: own.id.as_str().to_owned(),
            name: own.name,
            kind: own.kind,
        };
        let (spake, target_pake) = pake::start_target(&window.code);
        pake::send(
            writer,
            &ReplyMessage {
                reply: Reply {
                    pake: pake::encode(&target_pake),
                    device: target.clone(),
                    host_key: window.host_key.clone(),
                    account: window.account.clone(),
                },
            },
        )?;

        let shared = spake.finish(&controller_pake).map_err(|_| "badMessage")?;
        let keys = pake::derive(&shared);
        let hash = pake::transcript(
            &controller_pake,
            &target_pake,
            &start.device,
            &target,
            &window.host_key,
            &window.account,
        );
        let ConfirmMessage { confirm } = pake::receive(reader)?;
        let proof = pake::decode_mac(&confirm)?;
        if !keys.verify(Side::Controller, &hash, &proof) {
            return Err(self.wrong_code(&window, &start.device));
        }
        // The controller knows the code. Only now is anything saved. The window is single use,
        // so it is taken first, and a second session with the same code finds it closed.
        self.take(window.generation)?;
        let saved = self.save(&start.device, window.permissions);
        let outcome = saved.err().unwrap_or("ok");
        let _ = self.log.append(&Entry::new(
            Some(&start.device.id),
            Some(&start.device.name),
            "paired",
            outcome,
        ));
        saved.map_err(|_| "pairingSaveFailed")?;
        pake::send(
            writer,
            &ConfirmMessage {
                confirm: pake::encode(&keys.confirm(Side::Target, &hash)),
            },
        )
    }

    /// Counts a wrong code, and closes the window when the attempts are used. The window is
    /// closed and its line gone before the answer goes out.
    fn wrong_code(&self, window: &Window, controller: &ControllerInfo) -> &'static str {
        let exhausted = {
            let mut state = self.lock();
            match state.window.as_mut().filter(|w| w.generation == window.generation) {
                Some(current) => {
                    current.attempts = current.attempts.saturating_sub(1);
                    let exhausted = current.attempts == 0;
                    if exhausted {
                        state.window = None;
                        state.generation += 1;
                    }
                    exhausted
                }
                None => return "pairingClosed",
            }
        };
        if exhausted {
            self.changed.notify_all();
            self.sync_advert();
            let _ = self.remove_line();
        }
        // The name is only a claim until the code is proved, so the log keeps the id alone.
        let _ = self.log.append(&Entry::new(
            Some(&controller.id),
            None,
            "pair",
            "wrongCode",
        ));
        "wrongCode"
    }

    /// Closes the window of `generation` and removes its line, if it is still the open one.
    fn take(&self, generation: u64) -> Result<(), &'static str> {
        {
            let mut state = self.lock();
            let open = state
                .window
                .as_ref()
                .is_some_and(|w| w.generation == generation && Instant::now() < w.deadline);
            if !open {
                return Err("pairingClosed");
            }
            state.window = None;
            state.generation += 1;
        }
        self.changed.notify_all();
        self.sync_advert();
        // A line that stays is harmless: it expires, and the next start removes it.
        let _ = self.remove_line();
        Ok(())
    }

    /// Saves the controller as a peer with the permissions chosen here, and its line. A failure
    /// puts the peer list back as it was.
    fn save(
        &self,
        device: &ControllerInfo,
        permissions: PeerPermissions,
    ) -> Result<(), &'static str> {
        let keys = self.keys.as_ref().map_err(|code| *code)?;
        let id = DeviceId::parse(&device.id)?;
        let mut peer = Peer::new(&id, &device.name, device.kind, &device.public_key, now())?;
        peer.permissions = permissions;
        let previous = self.peers.get(&device.id);
        self.peers.insert(peer.clone())?;
        if let Err(code) = keys.install(&peer, &self.peers.networks()) {
            let _ = match previous {
                Some(previous) => self.peers.insert(previous),
                None => self.peers.remove(&device.id).map(|_| ()),
            };
            return Err(code);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digits_are_six_long_and_a_value_from_the_top_of_the_range_is_dropped() {
        assert_eq!(digits(|| Ok(0)).unwrap(), "000000");
        assert_eq!(digits(|| Ok(123_456)).unwrap(), "123456");
        assert_eq!(digits(|| Ok(1_000_007)).unwrap(), "000007");
        let mut values = vec![u32::MAX, 4_294_000_000, 4_293_999_999].into_iter();
        // The two that are at or over the limit are dropped. The last one is used.
        assert_eq!(digits(|| Ok(values.next().unwrap())).unwrap(), "999999");
        assert_eq!(digits(|| Err("randomFailed")), Err("randomFailed"));
        let code = digits(random_u32).unwrap();
        assert!(pake::valid_code(&code));
    }

    #[test]
    fn the_local_expiry_has_twelve_digits() {
        let text = local_expiry(1_800_000_000).unwrap();
        assert_eq!(text.len(), 12);
        assert!(text.bytes().all(|byte| byte.is_ascii_digit()));
    }
}
