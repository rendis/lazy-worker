//! The controller side of pairing: the address and the code go in, and a saved device comes out.
//!
//! The host key comes from a scan, so it is untrusted until the target repeats it inside the
//! proof. The pairing runs over the fixed pairing key with its own ssh config, so the key of this
//! device is never offered to a host that is not yet known to be the right one.

use super::{
    controlled::{Controlled, Store},
    destination::Destination,
    identity::{self, DeviceId},
    known_hosts::{self, HostKey},
    now,
    pake::{
        self, ConfirmMessage, ControllerInfo, Own, OwnDevice, ReplyMessage, Side, Start,
        StartMessage, TargetInfo,
    },
    pairing_key,
    ssh::{self, RemoteCommand},
    write_atomic, Layout,
};
use std::{
    io::{BufRead, BufReader, Read, Write},
    process::Child,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

/// Who the controller has paired with, once both confirmations are checked.
#[derive(Clone, Debug, PartialEq)]
pub struct Paired {
    pub target: TargetInfo,
    /// The account on the target that the pairing named.
    pub account: String,
}

/// The same string for a key that the scan found and one that the target sent.
fn key_text(key: &HostKey) -> String {
    format!("{} {}", key.kind, key.key)
}

/// Runs the protocol over a reader and a writer. `connected` is the host key of the machine that
/// the transport is really connected to. Nothing leaves this function before the target has
/// proved that it holds that very key.
pub fn controller_session(
    reader: &mut impl BufRead,
    writer: &mut impl Write,
    code: &str,
    me: &Own,
    connected: &HostKey,
) -> Result<Paired, &'static str> {
    if !pake::valid_code(code) {
        return Err("pairingCodeInvalid");
    }
    let controller = ControllerInfo {
        id: me.id.as_str().to_owned(),
        name: me.name.clone(),
        kind: me.kind,
        public_key: me.public_key.clone(),
    };
    controller.validate()?;
    let (spake, controller_pake) = pake::start_controller(code);
    // A target that has no window answers at once and hangs up, so the write may fail. What
    // it said is then still waiting in the reader, and it says why.
    let _ = pake::send(
        writer,
        &StartMessage {
            start: Start {
                version: pake::VERSION.to_owned(),
                pake: pake::encode(&controller_pake),
                device: controller.clone(),
            },
        },
    );

    let ReplyMessage { reply } = pake::receive(reader)?;
    reply.validate()?;
    let target_pake = pake::decode_pake(&reply.pake)?;
    if reply.host_key != key_text(connected) {
        return Err("pairingHostMismatch");
    }
    if reply.device.id == controller.id {
        return Err("pairingSelf");
    }
    let shared = spake.finish(&target_pake).map_err(|_| "badMessage")?;
    let keys = pake::derive(&shared);
    let hash = pake::transcript(
        &controller_pake,
        &target_pake,
        &controller,
        &reply.device,
        &reply.host_key,
        &reply.account,
    );
    pake::send(
        writer,
        &ConfirmMessage {
            confirm: pake::encode(&keys.confirm(Side::Controller, &hash)),
        },
    )?;

    let ConfirmMessage { confirm } = pake::receive(reader)?;
    let proof = pake::decode_mac(&confirm)?;
    if !keys.verify(Side::Target, &hash, &proof) {
        return Err("pairingFailed");
    }
    Ok(Paired {
        target: reply.device,
        account: reply.account,
    })
}

/// Pins the host key under the id of the target and saves the device. A failure puts the pin
/// back as it was, so nothing stays half done.
fn conclude(
    layout: &Layout,
    store: &Store,
    destination: &Destination,
    key: &HostKey,
    paired: &Paired,
) -> Result<Controlled, &'static str> {
    let id = DeviceId::parse(&paired.target.id)?;
    // The account that the target named is the one that the controller connects as.
    let address = Destination::new(&paired.account, destination.host(), Some(destination.port()))?;
    let device = Controlled::new(&id, &paired.target.name, paired.target.kind, &address, now())?;
    let before = known_hosts::pinned(layout, &id)?;
    known_hosts::pin(layout, &id, key)?;
    if let Err(code) = store.insert(device.clone()) {
        let _ = match before {
            Some(before) => known_hosts::pin(layout, &id, &before),
            None => known_hosts::unpin(layout, &id),
        };
        return Err(code);
    }
    Ok(device)
}

/// Pairs with the device at `destination`. The user name of the destination is only a first
/// guess: the saved one is the account that the target names. Runs ssh and waits for it, so
/// call it off the main thread.
pub fn pair(
    layout: &Layout,
    store: &Store,
    own: &OwnDevice,
    destination: &Destination,
    code: &str,
) -> Result<Controlled, &'static str> {
    if !pake::valid_code(code) {
        return Err("pairingCodeInvalid");
    }
    let me = own()?;
    let key = known_hosts::host_key(destination)?;
    // The scanned key is pinned under a name that lasts only for this call.
    let temporary = identity::random_id()?;
    known_hosts::pin(layout, &temporary, &key)?;
    let result = over_ssh(layout, destination, &temporary, code, &me, &key)
        .and_then(|paired| conclude(layout, store, destination, &key, &paired));
    let _ = known_hosts::unpin(layout, &temporary);
    result
}

/// Starts ssh with the pairing key and the temporary pin, and runs the protocol on its pipes.
fn over_ssh(
    layout: &Layout,
    destination: &Destination,
    temporary: &DeviceId,
    code: &str,
    me: &Own,
    key: &HostKey,
) -> Result<Paired, &'static str> {
    ssh::write_pairing_config(layout)?;
    write_atomic(
        &layout.pairing_key_file(),
        pairing_key::PRIVATE.as_bytes(),
    )?;
    let mut command = ssh::command(layout, destination, temporary, RemoteCommand::Pair)?;
    let mut child = command.spawn().map_err(|_| "sshFailed")?;
    let (Some(mut stdin), Some(stdout), Some(mut stderr)) =
        (child.stdin.take(), child.stdout.take(), child.stderr.take())
    else {
        let _ = child.kill();
        let _ = child.wait();
        return Err("sshFailed");
    };
    let errors = Arc::new(Mutex::new(Vec::<u8>::new()));
    let collected = Arc::clone(&errors);
    thread::spawn(move || {
        let mut chunk = [0u8; 1024];
        while let Ok(read) = stderr.read(&mut chunk) {
            if read == 0 {
                break;
            }
            if let Ok(mut all) = collected.lock() {
                let room = (8 * 1024usize).saturating_sub(all.len());
                all.extend_from_slice(&chunk[..read.min(room)]);
            }
        }
    });

    let child = Arc::new(Mutex::new(child));
    let finished = Arc::new(AtomicBool::new(false));
    let timed_out = Arc::new(AtomicBool::new(false));
    let watchdog = {
        let (child, finished, timed_out) = (child.clone(), finished.clone(), timed_out.clone());
        thread::spawn(move || {
            let start = Instant::now();
            while !finished.load(Ordering::Acquire) {
                if start.elapsed() >= pake::SESSION_TIMEOUT {
                    timed_out.store(true, Ordering::Release);
                    if let Ok(mut child) = child.lock() {
                        let _ = child.kill();
                    }
                    return;
                }
                thread::sleep(Duration::from_millis(50));
            }
        })
    };

    let outcome = controller_session(&mut BufReader::new(stdout), &mut stdin, code, me, key);
    drop(stdin);
    finished.store(true, Ordering::Release);
    let _ = watchdog.join();
    let code = wait(&child);
    let outcome = outcome.map_err(|error| {
        if timed_out.load(Ordering::Acquire) {
            "pairingTimeout"
        } else if error == "pairingFailed" && code == Some(255) {
            // ssh itself ended, so its message says why.
            let text = errors
                .lock()
                .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
                .unwrap_or_default();
            ssh::classify_stderr(&text)
        } else {
            error
        }
    });
    outcome
}

/// Waits a moment for ssh to end, and ends it when it does not.
fn wait(child: &Arc<Mutex<Child>>) -> Option<i32> {
    let start = Instant::now();
    loop {
        let Ok(mut child) = child.lock() else {
            return None;
        };
        match child.try_wait() {
            Ok(Some(status)) => return status.code(),
            Ok(None) if start.elapsed() < Duration::from_secs(2) => {}
            _ => {
                let _ = child.kill();
                return child.wait().ok().and_then(|status| status.code());
            }
        }
        drop(child);
        thread::sleep(Duration::from_millis(20));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        devices::Kind,
        remote::{
            authorized_keys::AuthorizedKeys,
            log::Log,
            pake::testing::{HOST_KEY, OTHER_HOST_KEY},
            pairing::Pairing,
            peers::{testing::ID_A, testing::ID_B, testing::KEY, PeerPermissions, Peers},
            testing::scratch,
        },
    };
    use std::{fs, os::unix::net::UnixStream, path::PathBuf};

    const TARGET_KEY: &str =
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAID8ZXmiaFqom1COnlczolZXMUWPTJ9+G8VXLl67Q9FJ9";

    struct Rig {
        base: PathBuf,
        home: PathBuf,
        layout: Layout,
        peers: Arc<Peers>,
        log: Arc<Log>,
        store: Store,
        pairing: Arc<Pairing>,
        host_key: HostKey,
    }

    impl Drop for Rig {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.base);
        }
    }

    fn own(id: &str, name: &str, kind: Kind, key: &str) -> Own {
        Own {
            id: DeviceId::parse(id).unwrap(),
            name: name.into(),
            kind,
            public_key: key.into(),
        }
    }

    fn rig(label: &str) -> Rig {
        rig_with(label, Duration::from_secs(600))
    }

    fn rig_with(label: &str, length: Duration) -> Rig {
        rig_full(label, length, Arc::new(crate::remote::advert::Silent))
    }

    fn rig_full(
        label: &str,
        length: Duration,
        advertiser: Arc<dyn crate::remote::advert::Advertiser>,
    ) -> Rig {
        let base = scratch(label);
        let home = base.join("home");
        let remote = base.join("remote");
        fs::create_dir_all(&home).unwrap();
        crate::remote::ensure_private_dir(&remote).unwrap();
        let layout = Layout::for_paths(remote.clone(), None);
        let peers = Arc::new(Peers::open(layout.peers_file()).unwrap());
        let log = Arc::new(Log::open(layout.log_file()).unwrap());
        let keys = AuthorizedKeys::new(
            home.clone(),
            PathBuf::from("/Applications/Lazy Worker.app/Contents/MacOS/lazy-worker"),
        );
        let host_key_file = base.join("host.pub");
        fs::write(&host_key_file, format!("{HOST_KEY} root@studio\n")).unwrap();
        let target = own(ID_B, "Studio", Kind::Desktop, TARGET_KEY);
        let pairing = Arc::new(Pairing::for_test(
            peers.clone(),
            log.clone(),
            keys,
            Arc::new(move || Ok(target.clone())),
            host_key_file,
            length,
        ).with_advertiser(advertiser));
        Rig {
            store: Store::open(layout.devices_file()).unwrap(),
            host_key: HostKey::new("ssh-ed25519", HOST_KEY.split(' ').nth(1).unwrap()).unwrap(),
            base,
            home,
            layout,
            peers,
            log,
            pairing,
        }
    }

    impl Rig {
        fn authorized(&self) -> String {
            fs::read_to_string(self.home.join(".ssh/authorized_keys")).unwrap_or_default()
        }

        fn open(&self, control: bool, applications: bool, actions: bool) -> String {
            self.pairing
                .open(PeerPermissions {
                    control,
                    applications,
                    actions,
                    screen: false,
                    shell: false,
                })
                .unwrap()
                .code
        }

        /// Runs both sides over sockets. `middle` may change the lines in transit.
        fn run(
            &self,
            code: &str,
            connected: &HostKey,
            middle: impl Fn(Direction, String) -> String + Send + Sync + 'static,
        ) -> Result<Paired, &'static str> {
            let (client, relay_client) = UnixStream::pair().unwrap();
            let (relay_target, target) = UnixStream::pair().unwrap();
            let pairing = self.pairing.clone();
            let served = thread::spawn(move || {
                let mut writer = target.try_clone().unwrap();
                let mut reader = BufReader::new(target);
                pairing.serve(&mut reader, &mut writer);
            });
            let middle = Arc::new(middle);
            let relays: Vec<_> = [
                (Direction::ToTarget, relay_client.try_clone().unwrap(), relay_target.try_clone().unwrap()),
                (Direction::ToController, relay_target, relay_client),
            ]
            .into_iter()
            .map(|(direction, from, to)| {
                let middle = middle.clone();
                thread::spawn(move || {
                    let mut to = to;
                    let mut from = BufReader::new(from);
                    let mut line = String::new();
                    while from.read_line(&mut line).unwrap_or(0) > 0 {
                        let changed = middle(direction, line.trim_end().to_owned());
                        if to.write_all(format!("{changed}\n").as_bytes()).is_err() {
                            break;
                        }
                        line.clear();
                    }
                    let _ = to.shutdown(std::net::Shutdown::Both);
                })
            })
            .collect();
            let mut writer = client.try_clone().unwrap();
            let mut reader = BufReader::new(client.try_clone().unwrap());
            let me = own(ID_A, "Laptop", Kind::Laptop, KEY);
            let result = controller_session(&mut reader, &mut writer, code, &me, connected);
            let _ = client.shutdown(std::net::Shutdown::Both);
            served.join().unwrap();
            for relay in relays {
                relay.join().unwrap();
            }
            result
        }

        fn plain(&self, code: &str) -> Result<Paired, &'static str> {
            self.run(code, &self.host_key.clone(), |_, line| line)
        }
    }

    #[derive(Clone, Copy, PartialEq)]
    enum Direction {
        ToTarget,
        ToController,
    }

    fn nothing_saved(rig: &Rig) {
        assert!(rig.peers.list().is_empty());
        assert!(rig.store.list().is_empty());
        assert!(!rig.authorized().contains(ID_A));
        assert_eq!(
            known_hosts::pinned(&rig.layout, &DeviceId::parse(ID_B).unwrap()),
            Ok(None)
        );
    }

    #[test]
    fn the_right_code_saves_both_sides_and_closes_the_window() {
        let rig = rig("pair-ok");
        let code = rig.open(true, false, true);
        assert!(rig.authorized().contains("remote --pair"));
        assert!(rig.pairing.state().open);

        let paired = rig.plain(&code).unwrap();
        assert_eq!(paired.target.id, ID_B);
        assert_eq!(paired.target.name, "Studio");
        assert_eq!(paired.account, "me");

        // The target has the controller, with exactly the permissions chosen there.
        let peer = rig.peers.get(ID_A).unwrap();
        assert_eq!(peer.name, "Laptop");
        assert_eq!(
            peer.permissions,
            PeerPermissions {
                control: true,
                applications: false,
                actions: true,
                screen: false,
                shell: false
            }
        );
        let authorized = rig.authorized();
        assert!(authorized.contains(&format!("remote --peer {ID_A}")));
        assert!(!authorized.contains("remote --pair") && !authorized.contains("lazy-worker:pairing"));
        assert!(!rig.pairing.state().open);
        assert_eq!(rig.log.read()[0].request, "paired");
        assert_eq!(rig.log.read()[0].outcome, "ok");

        // The controller saves the target and pins its host key under its id.
        let destination = Destination::new("me", "studio.local", Some(22)).unwrap();
        let device = conclude(&rig.layout, &rig.store, &destination, &rig.host_key, &paired).unwrap();
        assert_eq!((device.id.as_str(), device.user.as_str()), (ID_B, "me"));
        assert_eq!(rig.store.get(ID_B).unwrap().host, "studio.local");
        assert_eq!(
            known_hosts::pinned(&rig.layout, &DeviceId::parse(ID_B).unwrap()),
            Ok(Some(rig.host_key.clone()))
        );
        assert!(fs::read_to_string(rig.layout.known_hosts_file())
            .unwrap()
            .starts_with(&format!("lw-{ID_B} ssh-ed25519 ")));
    }

    #[test]
    fn a_used_code_does_not_open_a_second_session() {
        let rig = rig("pair-reuse");
        let code = rig.open(true, true, true);
        rig.plain(&code).unwrap();
        assert_eq!(rig.plain(&code).err(), Some("pairingClosed"));
        assert_eq!(rig.peers.list().len(), 1);
    }

    #[test]
    fn three_wrong_codes_close_the_window_and_remove_the_line() {
        let rig = rig("pair-wrong");
        let code = rig.open(true, true, true);
        let wrong = if code == "000000" { "000001" } else { "000000" };
        assert_eq!(rig.pairing.state().attempts_left, 3);
        for left in [2, 1] {
            assert_eq!(rig.plain(wrong).err(), Some("wrongCode"));
            assert_eq!(rig.pairing.state().attempts_left, left);
            assert!(rig.authorized().contains("remote --pair"));
        }
        assert_eq!(rig.plain(wrong).err(), Some("wrongCode"));
        // The window and the line are gone before the third answer arrives.
        assert!(!rig.pairing.state().open);
        assert!(!rig.authorized().contains("remote --pair"));
        // Even the right code finds nothing now.
        assert_eq!(rig.plain(&code).err(), Some("pairingClosed"));
        nothing_saved(&rig);
        assert!(rig.log.read().iter().all(|entry| entry.request == "pair" && entry.outcome == "wrongCode"));
    }

    #[test]
    fn opening_again_gives_a_new_code_and_fresh_attempts() {
        let rig = rig("pair-reopen");
        let first = rig.open(true, true, true);
        let wrong = if first == "000000" { "000001" } else { "000000" };
        assert_eq!(rig.plain(wrong).err(), Some("wrongCode"));
        assert_eq!(rig.pairing.state().attempts_left, 2);
        let mut second = rig.open(false, false, false);
        while second == first {
            second = rig.open(false, false, false);
        }
        assert_eq!(rig.pairing.state().attempts_left, 3);
        assert_eq!(rig.authorized().matches("remote --pair").count(), 1);
        assert_eq!(rig.plain(&first).err(), Some("wrongCode"));
        rig.plain(&second).unwrap();
        assert_eq!(rig.peers.get(ID_A).unwrap().permissions, PeerPermissions::default());
    }

    #[test]
    fn a_closed_window_answers_pairing_closed_and_a_manual_close_removes_the_line() {
        let rig = rig("pair-closed");
        assert_eq!(rig.plain("123456").err(), Some("pairingClosed"));
        let code = rig.open(true, true, true);
        rig.pairing.close().unwrap();
        assert!(!rig.authorized().contains("remote --pair"));
        assert!(!rig.pairing.state().open);
        assert_eq!(rig.plain(&code).err(), Some("pairingClosed"));
        nothing_saved(&rig);
    }

    #[test]
    fn a_window_that_runs_out_of_time_closes_itself_and_its_line() {
        let rig = rig_with("pair-expire", Duration::from_millis(150));
        let code = rig.open(true, true, true);
        assert!(rig.pairing.state().open);
        let start = Instant::now();
        while rig.pairing.state().open || rig.authorized().contains("remote --pair") {
            assert!(start.elapsed() < Duration::from_secs(5), "the timer did not close the window");
            thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(rig.plain(&code).err(), Some("pairingClosed"));
    }

    /// Counts the live registrations, and keeps what each one announced.
    #[derive(Default)]
    struct Counting {
        live: Arc<std::sync::atomic::AtomicUsize>,
        total: std::sync::atomic::AtomicUsize,
        seen: std::sync::Mutex<Vec<(String, crate::remote::advert::Advert)>>,
    }

    struct Live(Arc<std::sync::atomic::AtomicUsize>);

    impl Drop for Live {
        fn drop(&mut self) {
            self.0.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
        }
    }

    impl crate::remote::advert::Advertiser for Counting {
        fn register(
            &self,
            name: &str,
            advert: &crate::remote::advert::Advert,
        ) -> Option<crate::remote::advert::Registration> {
            use std::sync::atomic::Ordering::SeqCst;
            self.live.fetch_add(1, SeqCst);
            self.total.fetch_add(1, SeqCst);
            self.seen.lock().unwrap().push((name.to_owned(), advert.clone()));
            Some(Box::new(Live(self.live.clone())))
        }
    }

    fn counted(label: &str, length: Duration) -> (Rig, Arc<Counting>) {
        let counting = Arc::new(Counting::default());
        (rig_full(label, length, counting.clone()), counting)
    }

    fn live(counting: &Counting) -> usize {
        counting.live.load(std::sync::atomic::Ordering::SeqCst)
    }

    #[test]
    fn the_announcement_lives_exactly_as_long_as_the_window_and_holds_no_secret() {
        let (rig, counting) = counted("advert-close", Duration::from_secs(600));
        assert_eq!(live(&counting), 0, "nothing is announced while no window is open");
        let code = rig.open(true, true, true);
        assert_eq!(live(&counting), 1);
        {
            let seen = counting.seen.lock().unwrap();
            assert_eq!(seen[0].0, "Studio");
            assert_eq!(seen[0].1.account, "me");
            assert_eq!(seen[0].1.id, ID_B);
            let bytes = crate::remote::advert::encode_txt(&seen[0].1).unwrap();
            assert!(!String::from_utf8_lossy(&bytes).contains(&code));
        }
        // A new code in the same window keeps one announcement.
        rig.open(true, true, true);
        assert_eq!(live(&counting), 1);
        rig.pairing.close().unwrap();
        assert_eq!(live(&counting), 0);
        // Closing again, with no window, changes nothing.
        rig.pairing.close().unwrap();
        assert_eq!(live(&counting), 0);
        rig.open(true, true, true);
        assert_eq!(live(&counting), 1);
    }

    #[test]
    fn the_announcement_ends_on_success_on_three_wrong_codes_and_on_expiry() {
        let (rig, counting) = counted("advert-end", Duration::from_secs(600));
        let code = rig.open(true, true, true);
        assert_eq!(live(&counting), 1);
        rig.plain(&code).unwrap();
        assert_eq!(live(&counting), 0, "success ends it");

        let code = rig.open(true, true, true);
        let wrong = if code == "000000" { "000001" } else { "000000" };
        for _ in 0..2 {
            assert_eq!(rig.plain(wrong).err(), Some("wrongCode"));
            assert_eq!(live(&counting), 1);
        }
        assert_eq!(rig.plain(wrong).err(), Some("wrongCode"));
        assert_eq!(live(&counting), 0, "the third wrong code ends it");

        let (rig, counting) = counted("advert-expire", Duration::from_millis(150));
        rig.open(true, true, true);
        assert_eq!(live(&counting), 1);
        let start = Instant::now();
        while live(&counting) != 0 {
            assert!(start.elapsed() < Duration::from_secs(5), "the timer did not end the announcement");
            thread::sleep(Duration::from_millis(20));
        }
        assert!(!rig.pairing.state().open);
    }

    #[test]
    fn a_window_that_cannot_be_announced_still_pairs() {
        let rig = rig("advert-failed");
        let code = rig.open(true, true, true);
        rig.plain(&code).unwrap();
    }

    #[test]
    fn a_host_key_that_is_not_the_connected_one_stops_the_controller_first() {
        let rig = rig("pair-mismatch");
        let code = rig.open(true, true, true);
        let other = HostKey::new("ssh-ed25519", OTHER_HOST_KEY.split(' ').nth(1).unwrap()).unwrap();
        let seen = Arc::new(Mutex::new(Vec::<String>::new()));
        let record = seen.clone();
        let result = rig.run(&code, &other, move |direction, line| {
            if direction == Direction::ToTarget {
                record.lock().unwrap().push(line.clone());
            }
            line
        });
        assert_eq!(result.err(), Some("pairingHostMismatch"));
        // Only the first message left the controller.
        let sent = seen.lock().unwrap().clone();
        assert_eq!(sent.len(), 1);
        assert!(sent[0].starts_with("{\"start\""));
        nothing_saved(&rig);
        // The target did not count an attempt and is still open for the real controller.
        assert_eq!(rig.pairing.state().attempts_left, 3);
    }

    #[test]
    fn a_message_changed_in_transit_breaks_the_proof_and_nothing_is_saved() {
        let rig = rig("pair-tamper");
        let code = rig.open(true, true, true);
        // The target name that the controller sees is not the one that the target signed.
        let result = rig.run(&code, &rig.host_key.clone(), |direction, line| {
            if direction == Direction::ToController && line.starts_with("{\"reply\"") {
                line.replace("Studio", "Studiq")
            } else {
                line
            }
        });
        assert_eq!(result.err(), Some("wrongCode"));
        nothing_saved(&rig);
        // The controller name changed on the way to the target.
        let result = rig.run(&code, &rig.host_key.clone(), |direction, line| {
            if direction == Direction::ToTarget && line.starts_with("{\"start\"") {
                line.replace("Laptop", "Laptoq")
            } else {
                line
            }
        });
        assert_eq!(result.err(), Some("wrongCode"));
        nothing_saved(&rig);
        assert_eq!(rig.pairing.state().attempts_left, 1);
    }

    #[test]
    fn a_confirmation_that_is_not_the_targets_is_refused() {
        let rig = rig("pair-fakeconfirm");
        let code = rig.open(true, true, true);
        let result = rig.run(&code, &rig.host_key.clone(), |direction, line| {
            if direction == Direction::ToController && line.starts_with("{\"confirm\"") {
                format!("{{\"confirm\":\"{}\"}}", pake::encode(&[9; 32]))
            } else {
                line
            }
        });
        assert_eq!(result.err(), Some("pairingFailed"));
        // The target did accept the controller, because the controller's proof was right.
        assert!(rig.peers.get(ID_A).is_some());
        assert!(rig.store.list().is_empty());
    }

    #[test]
    fn the_target_refuses_bad_messages_and_counts_no_attempt_for_them() {
        let rig = rig("pair-refuse");
        let code = rig.open(true, true, true);
        let start = |edit: &dyn Fn(String) -> String| {
            let line = start_line(&code);
            let line = edit(line);
            talk(&rig, &[&line])
        };
        // Unknown field, bad version, bad id, bad name, oversized, not JSON.
        assert_eq!(start(&|l| l.replacen("{\"start\":{", "{\"start\":{\"extra\":1,", 1)), "badMessage");
        assert_eq!(start(&|l| l.replace("lw-pair-1", "lw-pair-2")), "badMessage");
        assert_eq!(start(&|l| l.replace(ID_A, "nothex")), "badMessage");
        assert_eq!(start(&|l| l.replace("Laptop", "  ")), "badMessage");
        assert_eq!(start(&|l| l.replace("\"kind\":\"laptop\"", "\"kind\":\"phone\"")), "badMessage");
        assert_eq!(start(&|l| l.replace(KEY, "ssh-rsa AAAA")), "badMessage");
        assert_eq!(start(&|l| l.replacen("\"pake\":\"", "\"pake\":\"AAAA", 1)), "badMessage");
        assert_eq!(talk(&rig, &["not json"]), "badMessage");
        assert_eq!(talk(&rig, &[&"x".repeat(70_000)]), "lineTooLong");
        // A device that has the id of the target itself.
        assert_eq!(start(&|l| l.replace(ID_A, ID_B)), "pairingSelf");
        assert_eq!(rig.pairing.state().attempts_left, 3);
        assert!(rig.pairing.state().open);
        nothing_saved(&rig);
        // The real controller still gets in.
        rig.plain(&code).unwrap();
    }

    fn start_line(code: &str) -> String {
        let me = own(ID_A, "Laptop", Kind::Laptop, KEY);
        let (_, message) = pake::start_controller(code);
        serde_json::to_string(&StartMessage {
            start: Start {
                version: pake::VERSION.into(),
                pake: pake::encode(&message),
                device: ControllerInfo {
                    id: me.id.as_str().into(),
                    name: me.name,
                    kind: me.kind,
                    public_key: me.public_key,
                },
            },
        })
        .unwrap()
    }

    /// Sends raw lines to the target and returns the error code of its last answer.
    fn talk(rig: &Rig, lines: &[&str]) -> String {
        let (client, target) = UnixStream::pair().unwrap();
        let pairing = rig.pairing.clone();
        let served = thread::spawn(move || {
            let mut writer = target.try_clone().unwrap();
            pairing.serve(&mut BufReader::new(target), &mut writer);
        });
        let mut writer = client.try_clone().unwrap();
        for line in lines {
            let _ = writer.write_all(format!("{line}\n").as_bytes());
        }
        let mut reader = BufReader::new(client.try_clone().unwrap());
        let mut last = String::new();
        let mut line = String::new();
        // The target may answer with its reply first, and then with the error.
        while reader.read_line(&mut line).unwrap_or(0) > 0 {
            last = line.trim_end().to_owned();
            line.clear();
            if last.starts_with("{\"error\"") {
                break;
            }
            let _ = writer.shutdown(std::net::Shutdown::Write);
        }
        served.join().unwrap();
        serde_json::from_str::<serde_json::Value>(&last)
            .ok()
            .and_then(|value| value["error"].as_str().map(str::to_owned))
            .unwrap_or(last)
    }

    #[test]
    fn only_one_session_runs_at_a_time() {
        let rig = rig("pair-busy");
        let code = rig.open(true, true, true);
        // The first session is waiting for its start message.
        let (first_client, first_target) = UnixStream::pair().unwrap();
        let pairing = rig.pairing.clone();
        let first = thread::spawn(move || {
            let mut writer = first_target.try_clone().unwrap();
            pairing.serve(&mut BufReader::new(first_target), &mut writer);
        });
        thread::sleep(Duration::from_millis(500));
        assert_eq!(talk(&rig, &[&start_line(&code)]), "pairingBusy");
        // The first one ends, and the slot is free again.
        drop(first_client);
        first.join().unwrap();
        rig.plain(&code).unwrap();
    }

    #[test]
    fn the_controller_checks_the_code_and_the_replies_it_gets() {
        let me = own(ID_A, "Laptop", Kind::Laptop, KEY);
        let key = HostKey::new("ssh-ed25519", HOST_KEY.split(' ').nth(1).unwrap()).unwrap();
        let run = |code: &str, answer: &str| {
            controller_session(
                &mut BufReader::new(answer.as_bytes()),
                &mut Vec::new(),
                code,
                &me,
                &key,
            )
            .err()
        };
        assert_eq!(run("12345", ""), Some("pairingCodeInvalid"));
        assert_eq!(run("12345a", ""), Some("pairingCodeInvalid"));
        assert_eq!(run("123456", ""), Some("pairingFailed"));
        assert_eq!(run("123456", "{\"error\":\"pairingClosed\"}\n"), Some("pairingClosed"));
        assert_eq!(run("123456", "{\"error\":\"anything\"}\n"), Some("pairingFailed"));
        assert_eq!(run("123456", "garbage\n"), Some("badMessage"));
        assert_eq!(run("123456", "{\"reply\":{\"x\":1}}\n"), Some("badMessage"));
        let reply = |edit: &dyn Fn(String) -> String| {
            let (_, message) = pake::start_target("123456");
            let line = serde_json::to_string(&ReplyMessage {
                reply: pake::Reply {
                    pake: pake::encode(&message),
                    device: TargetInfo {
                        id: ID_B.into(),
                        name: "Studio".into(),
                        kind: Kind::Desktop,
                    },
                    host_key: HOST_KEY.into(),
                    account: "me".into(),
                },
            })
            .unwrap();
            run("123456", &format!("{}\n", edit(line)))
        };
        // A well formed reply goes on, and ends at the missing confirmation.
        assert_eq!(reply(&|l| l), Some("pairingFailed"));
        assert_eq!(reply(&|l| l.replace(HOST_KEY, OTHER_HOST_KEY)), Some("pairingHostMismatch"));
        assert_eq!(reply(&|l| l.replace("\"me\"", "\"-oProxyCommand=x\"")), Some("badMessage"));
        assert_eq!(reply(&|l| l.replace(ID_B, ID_A)), Some("pairingSelf"));
        assert_eq!(reply(&|l| l.replace("\"device\"", "\"extra\":1,\"device\"")), Some("badMessage"));
    }

    #[test]
    fn a_failed_save_puts_the_pin_back() {
        let rig = rig("pair-conclude");
        let destination = Destination::new("me", "studio.local", Some(22)).unwrap();
        let old = HostKey::new("ssh-ed25519", OTHER_HOST_KEY.split(' ').nth(1).unwrap()).unwrap();
        let id = DeviceId::parse(ID_B).unwrap();
        known_hosts::pin(&rig.layout, &id, &old).unwrap();
        let paired = Paired {
            target: TargetInfo {
                id: ID_B.into(),
                name: "Studio".into(),
                kind: Kind::Desktop,
            },
            account: "me".into(),
        };
        // A file that cannot be replaced makes the store fail.
        fs::create_dir(rig.layout.devices_file()).unwrap();
        assert!(conclude(&rig.layout, &rig.store, &destination, &rig.host_key, &paired).is_err());
        assert_eq!(known_hosts::pinned(&rig.layout, &id), Ok(Some(old)));
        assert!(rig.store.list().is_empty());
        fs::remove_dir(rig.layout.devices_file()).unwrap();
        // The account that the target names, not the one typed, is saved.
        let typed = Destination::new("someone", "studio.local", Some(2222)).unwrap();
        let device = conclude(&rig.layout, &rig.store, &typed, &rig.host_key, &paired).unwrap();
        assert_eq!((device.user.as_str(), device.port), ("me", 2222));
    }
}
