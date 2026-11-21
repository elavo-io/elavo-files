//! The handle the UI keeps, and the events it takes from the network.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use super::proto::{Device, FileMeta, Info};
use super::Config;

#[derive(Clone, Debug)]
pub struct Incoming {
    /// An id for the local dialog, not the session id sent to the peer.
    pub id: String,
    pub sender: Device,
    pub files: Vec<FileMeta>,
    pub bytes: u64,
    /// Accepted already (approval off), so the UI shows only progress.
    pub accepted: bool,
    /// How long the request waits for an answer before it is declined;
    /// none when there is nothing to answer.
    pub expires_in: Option<std::time::Duration>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    Accept,
    Decline,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SendState {
    Preparing,
    Waiting,
    Sending,
    Done,
    Cancelled,
    Declined,
    /// The receiver wants a PIN, or the one given was wrong.
    NeedsPin,
    /// The receiver is busy with another transfer.
    Busy,
    Error(String),
}

#[derive(Clone, Debug)]
pub struct Transfer {
    pub id: String,
    pub device: String,
    pub receiving: bool,
    pub file_name: String,
    pub done: u64,
    pub total: u64,
    pub state: SendState,
    /// What was saved, once received: the top-level items.
    pub saved: Vec<PathBuf>,
}

#[derive(Clone, Debug)]
pub enum Event {
    Devices(Vec<Device>),
    Incoming(Incoming),
    /// The sender gave up, or the request timed out. Close its dialog.
    Dismiss(String),
    Transfer(Transfer),
    Error(String),
}

/// The files, ids and tokens of a receive session, bound to the peer IP.
pub(crate) struct Session {
    pub ip: String,
    pub v1: bool,
    pub sender: String,
    pub total: u64,
    pub done: u64,
    pub saved: Vec<PathBuf>,
    pub last_event: Instant,
    pub files: BTreeMap<String, (FileMeta, String)>,
    pub folder: PathBuf,
    pub cancelled: Arc<AtomicBool>,
    pub expires: Instant,
}

pub(crate) struct Shared {
    pub config: Mutex<Config>,
    pub identity: super::tls::Identity,
    pub devices: Mutex<HashMap<String, (Device, Instant)>>,
    /// Requests waiting for an answer, with the IP they came from.
    pub decisions: Mutex<HashMap<String, (mpsc::Sender<Decision>, String)>>,
    pub sessions: Mutex<HashMap<String, Session>>,
    pub sends: Mutex<HashMap<String, Arc<AtomicBool>>>,
    pub busy: AtomicBool,
    pub stop: AtomicBool,
    pub events: mpsc::Sender<Event>,
}

impl Shared {
    pub fn config(&self) -> Config {
        self.config.lock().unwrap().clone()
    }

    pub fn info(&self) -> Info {
        let c = self.config();
        Info {
            alias: c.alias,
            version: Some(super::PROTOCOL_VERSION.to_owned()),
            device_model: Some(super::DEVICE_MODEL.to_owned()),
            device_type: Some("desktop".to_owned()),
            fingerprint: Some(self.identity.fingerprint.clone()),
            port: Some(c.port),
            protocol: Some(
                if self.identity.https() {
                    "https"
                } else {
                    "http"
                }
                .to_owned(),
            ),
            download: Some(false),
            ..Default::default()
        }
    }

    pub fn event(&self, event: Event) {
        let _ = self.events.send(event);
    }

    pub fn remember(&self, device: Device) {
        if device.same_fingerprint(&self.identity.fingerprint) {
            return;
        }
        let changed = {
            let mut all = self.devices.lock().unwrap();
            let changed = all
                .get(&device.fingerprint)
                .is_none_or(|(old, _)| old != &device);
            all.insert(device.fingerprint.clone(), (device, Instant::now()));
            changed
        };
        if changed {
            self.devices_changed();
        }
    }

    pub fn devices(&self) -> Vec<Device> {
        let mut devices: Vec<_> = self
            .devices
            .lock()
            .unwrap()
            .values()
            .map(|(d, _)| d.clone())
            .collect();
        devices.sort_by_key(|d| d.alias.to_lowercase());
        devices
    }

    pub fn devices_changed(&self) {
        self.event(Event::Devices(self.devices()));
    }

    pub fn forget_old(&self) {
        let changed = {
            let mut all = self.devices.lock().unwrap();
            let n = all.len();
            all.retain(|_, (_, at)| at.elapsed() < Duration::from_secs(90));
            all.len() != n
        };
        if changed {
            self.devices_changed();
        }
        let mut sessions = self.sessions.lock().unwrap();
        sessions.retain(|_, s| s.expires > Instant::now());
    }
}

/// One background receiver/discovery service. Keep this handle until the
/// app exits; dropping it stops the threads.
#[derive(Clone)]
pub struct Service {
    pub(crate) shared: Arc<Shared>,
    owner: Arc<Owner>,
}

struct Owner(Arc<Shared>);
impl Drop for Owner {
    fn drop(&mut self) {
        self.0.stop.store(true, Ordering::Relaxed);
        for (_, (tx, _)) in self.0.decisions.lock().unwrap().drain() {
            let _ = tx.send(Decision::Decline);
        }
        for cancel in self.0.sends.lock().unwrap().values() {
            cancel.store(true, Ordering::Relaxed);
        }
    }
}

impl Service {
    /// Start both services. A taken TCP port is an error, rather than
    /// silently competing with another LocalSend receiver.
    pub fn start(config: Config) -> std::io::Result<(Self, mpsc::Receiver<Event>)> {
        Self::start_in(config, super::config::identity_dir())
    }

    /// As `start`, with a different identity directory (for isolated tests).
    pub fn start_in(
        config: Config,
        identity_dir: PathBuf,
    ) -> std::io::Result<(Self, mpsc::Receiver<Event>)> {
        Self::start_with(config, identity_dir, true)
    }

    /// As `start_in`, optionally without multicast discovery.
    pub fn start_with(
        config: Config,
        identity_dir: PathBuf,
        discovery: bool,
    ) -> std::io::Result<(Self, mpsc::Receiver<Event>)> {
        let listener = std::net::TcpListener::bind(("0.0.0.0", config.port))?;
        listener.set_nonblocking(true)?;
        let (tx, rx) = mpsc::channel();
        let identity = super::tls::identity(&identity_dir, config.https);
        let shared = Arc::new(Shared {
            config: Mutex::new(config),
            identity,
            devices: Mutex::new(HashMap::new()),
            decisions: Mutex::new(HashMap::new()),
            sessions: Mutex::new(HashMap::new()),
            sends: Mutex::new(HashMap::new()),
            busy: AtomicBool::new(false),
            stop: AtomicBool::new(false),
            events: tx,
        });
        super::log(&format!(
            "listening on {} ({}), as {}",
            shared.config().port,
            if shared.identity.https() {
                "HTTPS"
            } else {
                "HTTP"
            },
            shared.config().alias
        ));
        super::server::start(shared.clone(), listener);
        if discovery {
            super::discovery::start(shared.clone(), super::discovery::port());
        }
        Ok((
            Self {
                owner: Arc::new(Owner(shared.clone())),
                shared,
            },
            rx,
        ))
    }

    pub fn info(&self) -> Info {
        self.shared.info()
    }

    pub fn config(&self) -> Config {
        self.shared.config()
    }

    /// Alias, receive, folder, approval and PIN apply without a restart.
    /// A port or encryption change takes effect at the next start.
    pub fn configure(&self, mut c: Config) {
        let old = self.config();
        c.port = old.port;
        c.https = old.https;
        *self.shared.config.lock().unwrap() = c;
        self.discover();
    }

    pub fn devices(&self) -> Vec<Device> {
        self.shared.devices()
    }

    pub fn decide(&self, id: &str, decision: Decision) {
        if let Some((tx, _)) = self.shared.decisions.lock().unwrap().remove(id) {
            let _ = tx.send(decision);
        }
    }

    /// Announce again, for the Refresh button.
    pub fn discover(&self) {
        let shared = self.shared.clone();
        std::thread::spawn(move || super::discovery::announce(&shared, true));
    }

    /// Register a device at an address, for manual discovery or tests.
    pub fn register_at(&self, device: Device) {
        let shared = self.shared.clone();
        std::thread::spawn(move || match super::client::register(&shared, &device) {
            Ok(d) => shared.remember(d),
            Err(e) => shared.event(Event::Error(e.to_string())),
        });
    }

    /// Send a selection (folders become relative paths, as in LocalSend).
    pub fn send(&self, device: Device, paths: Vec<PathBuf>, pin: Option<String>) -> String {
        let id = super::proto::random_id();
        let cancel = Arc::new(AtomicBool::new(false));
        self.shared
            .sends
            .lock()
            .unwrap()
            .insert(id.clone(), cancel.clone());
        let shared = self.shared.clone();
        let send_id = id.clone();
        std::thread::spawn(move || {
            super::client::send(&shared, &send_id, &device, paths, pin, cancel);
            shared.sends.lock().unwrap().remove(&send_id);
        });
        id
    }

    pub fn cancel(&self, id: &str) {
        if let Some(c) = self.shared.sends.lock().unwrap().get(id) {
            c.store(true, Ordering::Relaxed);
        }
        if let Some(s) = self.shared.sessions.lock().unwrap().remove(id) {
            s.cancelled.store(true, Ordering::Relaxed);
        }
    }

    pub fn is_alive(&self) -> bool {
        Arc::strong_count(&self.owner) > 0 && !self.shared.stop.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::localsend::DeviceType;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("ls-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn start(
        name: &str,
        port: u16,
        https: bool,
        approval: bool,
        pin: &str,
    ) -> (Service, mpsc::Receiver<Event>, PathBuf) {
        let dir = tmp(name);
        let config = Config {
            alias: name.into(),
            custom_alias: None,
            receive: true,
            favorites_only: false,
            auto_accept_favorites: true,
            favorites: Vec::new(),
            save_folder: dir.join("in"),
            require_approval: approval,
            require_pin: !pin.is_empty(),
            pin: pin.into(),
            port,
            https,
            ask_for: super::super::ASK_FOR,
        };
        let (s, rx) = Service::start_with(config, dir.join("id"), false).unwrap();
        (s, rx, dir)
    }

    fn device(s: &Service) -> Device {
        let info = s.info();
        Device {
            alias: info.alias,
            model: None,
            kind: DeviceType::Desktop,
            fingerprint: info.fingerprint.unwrap(),
            ip: "127.0.0.1".into(),
            port: info.port.unwrap(),
            https: info.protocol.as_deref() == Some("https"),
            version: "2.1".into(),
        }
    }

    /// The state a send ends in.
    fn outcome(rx: &mpsc::Receiver<Event>, id: &str) -> SendState {
        loop {
            match rx.recv_timeout(Duration::from_secs(20)).expect("an event") {
                Event::Transfer(t) if t.id == id => match t.state {
                    SendState::Preparing | SendState::Waiting | SendState::Sending => {}
                    s => return s,
                },
                _ => {}
            }
        }
    }

    fn send_both_ways(https: bool, base: u16) {
        let (a, a_rx, a_dir) = start(&format!("a{https}"), base, https, false, "");
        let (b, b_rx, b_dir) = start(&format!("b{https}"), base + 1, https, true, "4321");
        assert_eq!(a.info().protocol.as_deref() == Some("https"), https);
        // Registering teaches each about the other.
        a.register_at(device(&b));
        let deadline = Instant::now() + Duration::from_secs(10);
        while b.devices().is_empty() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert_eq!(b.devices()[0].alias, a.info().alias);
        assert_eq!(b.devices()[0].fingerprint, a.info().fingerprint.unwrap());

        // B to A, which accepts by itself: a folder and a file.
        std::fs::create_dir_all(b_dir.join("out/Proj/sub")).unwrap();
        std::fs::write(b_dir.join("out/Proj/sub/x.txt"), vec![7u8; 700_000]).unwrap();
        std::fs::write(b_dir.join("out/note.md"), b"# hi").unwrap();
        let id = b.send(
            device(&a),
            vec![b_dir.join("out/Proj"), b_dir.join("out/note.md")],
            None,
        );
        assert_eq!(outcome(&b_rx, &id), SendState::Done);
        assert_eq!(std::fs::read(a_dir.join("in/note.md")).unwrap(), b"# hi");
        assert_eq!(
            std::fs::read(a_dir.join("in/Proj/sub/x.txt"))
                .unwrap()
                .len(),
            700_000
        );
        // Again: a second copy, not an overwrite.
        let id = b.send(device(&a), vec![b_dir.join("out/note.md")], None);
        assert_eq!(outcome(&b_rx, &id), SendState::Done);
        assert!(a_dir.join("in/note 2.md").exists());

        // A to B, which wants a PIN and asks.
        let id = a.send(device(&b), vec![b_dir.join("out/note.md")], None);
        assert_eq!(outcome(&a_rx, &id), SendState::NeedsPin);
        let id = a.send(
            device(&b),
            vec![b_dir.join("out/note.md")],
            Some("4321".into()),
        );
        let asked = loop {
            if let Event::Incoming(i) = b_rx.recv_timeout(Duration::from_secs(10)).unwrap() {
                break i;
            }
        };
        assert_eq!(asked.files[0].file_name, "note.md");
        assert!(!asked.accepted);
        assert_eq!(
            asked.expires_in,
            Some(Duration::from_secs(u64::from(super::super::ASK_FOR)))
        );
        b.decide(&asked.id, Decision::Decline);
        assert_eq!(outcome(&a_rx, &id), SendState::Declined);
        let id = a.send(
            device(&b),
            vec![b_dir.join("out/note.md")],
            Some("4321".into()),
        );
        let asked = loop {
            if let Event::Incoming(i) = b_rx.recv_timeout(Duration::from_secs(10)).unwrap() {
                break i;
            }
        };
        b.decide(&asked.id, Decision::Accept);
        assert_eq!(outcome(&a_rx, &id), SendState::Done);
        assert!(b_dir.join("in/note.md").exists());

        // A pinned send to the wrong fingerprint never connects.
        if https {
            let mut wrong = device(&b);
            wrong.fingerprint = "00".repeat(32);
            let id = a.send(wrong, vec![b_dir.join("out/note.md")], Some("4321".into()));
            assert!(matches!(outcome(&a_rx, &id), SendState::Error(_)));
        }
        drop((a, b));
        let _ = std::fs::remove_dir_all(a_dir);
        let _ = std::fs::remove_dir_all(b_dir);
    }

    #[test]
    fn sends_and_receives_over_https() {
        send_both_ways(true, 53417);
    }

    #[test]
    fn favorites_only_turns_others_away_and_takes_favorites_unasked() {
        let (a, a_rx, a_dir) = start("fav-a", 53431, false, false, "");
        let (b, _b_rx, b_dir) = start("fav-b", 53432, false, true, "");
        std::fs::create_dir_all(a_dir.join("out")).unwrap();
        std::fs::write(a_dir.join("out/f.txt"), b"fav").unwrap();
        let mut c = b.config();
        c.favorites_only = true;
        b.configure(c.clone());
        // A is not a favorite: declined without anyone being asked.
        let id = a.send(device(&b), vec![a_dir.join("out/f.txt")], None);
        assert_eq!(outcome(&a_rx, &id), SendState::Declined);
        assert!(!b_dir.join("in/f.txt").exists());
        // A favorite, with Automatically Accept: saved without asking,
        // though B asks everyone else.
        c.favorites.push(super::super::config::Favorite {
            fingerprint: a.info().fingerprint.unwrap(),
            alias: "fav-a".into(),
        });
        b.configure(c);
        let id = a.send(device(&b), vec![a_dir.join("out/f.txt")], None);
        assert_eq!(outcome(&a_rx, &id), SendState::Done);
        assert_eq!(std::fs::read(b_dir.join("in/f.txt")).unwrap(), b"fav");
    }

    #[test]
    fn sends_and_receives_over_http() {
        send_both_ways(false, 53427);
    }

    /// Nobody answers: the request goes away when it says, declined.
    #[test]
    fn unanswered_requests_expire() {
        let (a, a_rx, a_dir) = start("expire-a", 53447, true, false, "");
        let (b, b_rx, b_dir) = start("expire-b", 53448, true, true, "");
        let mut c = b.config();
        c.ask_for = 1;
        b.configure(c);
        std::fs::create_dir_all(a_dir.join("out")).unwrap();
        std::fs::write(a_dir.join("out/late.txt"), b"late").unwrap();
        let id = a.send(device(&b), vec![a_dir.join("out/late.txt")], None);
        let asked = loop {
            if let Event::Incoming(i) = b_rx.recv_timeout(Duration::from_secs(10)).unwrap() {
                break i;
            }
        };
        assert_eq!(asked.expires_in, Some(Duration::from_secs(1)));
        let gone = loop {
            if let Event::Dismiss(d) = b_rx.recv_timeout(Duration::from_secs(10)).unwrap() {
                break d;
            }
        };
        assert_eq!(gone, asked.id);
        assert_eq!(outcome(&a_rx, &id), SendState::Declined);
        // Answering afterwards does nothing.
        b.decide(&asked.id, Decision::Accept);
        assert!(!b_dir.join("in/late.txt").exists());
        drop((a, b));
        let _ = std::fs::remove_dir_all(a_dir);
        let _ = std::fs::remove_dir_all(b_dir);
    }
}

#[cfg(test)]
mod reference {
    use super::*;

    /// Against an independent peer (`ref.py serve`): LS_REF_FP=… cargo test -- --ignored
    #[test]
    #[ignore]
    fn sends_to_a_reference_peer() {
        let Ok(fp) = std::env::var("LS_REF_FP") else {
            return;
        };
        let dir = std::env::temp_dir().join("ls-ref-client");
        let _ = std::fs::create_dir_all(&dir);
        std::fs::write(dir.join("from-files.txt"), b"Sent by Elavo Files").unwrap();
        let config = Config {
            alias: "Files Client".into(),
            custom_alias: None,
            port: 53437,
            ..Config::default()
        };
        let (s, rx) = Service::start_with(config, dir.join("id"), false).unwrap();
        let dev = Device {
            alias: "Python Ref".into(),
            model: None,
            kind: crate::localsend::DeviceType::Headless,
            fingerprint: fp,
            ip: "127.0.0.1".into(),
            port: 53319,
            https: true,
            version: "2.1".into(),
        };
        let reg = super::super::client::register(&s.shared, &dev).unwrap();
        assert_eq!(reg.alias, "Python Ref");
        let id = s.send(dev, vec![dir.join("from-files.txt")], None);
        let end = loop {
            if let Event::Transfer(t) = rx.recv_timeout(Duration::from_secs(20)).unwrap() {
                if t.id == id
                    && !matches!(
                        t.state,
                        SendState::Preparing | SendState::Waiting | SendState::Sending
                    )
                {
                    break t.state;
                }
            }
        };
        assert_eq!(end, SendState::Done);
    }
}
