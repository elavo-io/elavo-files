//! Multicast discovery: announce, listen, and answer announcements.

use gio::glib;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gio::prelude::*;

use super::proto::{Device, Info};
use super::service::Shared;
use super::{client, log, MULTICAST_GROUP};

/// How often to announce again, so devices that answer stay listed.
const EVERY: Duration = Duration::from_secs(45);

/// `ELAVO_LOCALSEND_TTL=0` keeps announcements on this computer, for tests.
fn ttl() -> u32 {
    std::env::var("ELAVO_LOCALSEND_TTL")
        .ok()
        .and_then(|t| t.parse().ok())
        .unwrap_or(1)
}

/// The multicast port: LocalSend's, or `ELAVO_LOCALSEND_DISCOVERY_PORT`
/// so tests stay apart from a LocalSend already running here.
pub(crate) fn port() -> u16 {
    std::env::var("ELAVO_LOCALSEND_DISCOVERY_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .filter(|p| *p > 0)
        .unwrap_or(super::DEFAULT_PORT)
}

fn group() -> gio::InetSocketAddress {
    let addr = gio::InetAddress::from_string(MULTICAST_GROUP).expect("a multicast address");
    gio::InetSocketAddress::new(&addr, port())
}

fn sender() -> Result<gio::Socket, glib::Error> {
    let s = gio::Socket::new(
        gio::SocketFamily::Ipv4,
        gio::SocketType::Datagram,
        gio::SocketProtocol::Udp,
    )?;
    s.set_multicast_ttl(ttl());
    s.set_multicast_loopback(true);
    Ok(s)
}

/// Send who this device is to the group: an announcement, or with
/// `announce` false the fallback answer to one.
pub(crate) fn announce(shared: &Shared, announce: bool) {
    let mut info = shared.info();
    info.announce = Some(announce);
    info.announcement = Some(announce);
    let Ok(json) = serde_json::to_vec(&info) else {
        return;
    };
    match sender().and_then(|s| s.send_to(Some(&group()), &json, gio::Cancellable::NONE)) {
        Ok(_) => log(&format!(
            "multicast {}",
            if announce { "announce" } else { "answer" }
        )),
        Err(e) => log(&format!("multicast send: {}", e.message())),
    }
}

fn listener(port: u16) -> Result<gio::Socket, glib::Error> {
    let s = gio::Socket::new(
        gio::SocketFamily::Ipv4,
        gio::SocketType::Datagram,
        gio::SocketProtocol::Udp,
    )?;
    let any = gio::InetAddress::new_any(gio::SocketFamily::Ipv4);
    // Shared with other LocalSend apps on this computer.
    s.bind(&gio::InetSocketAddress::new(&any, port), true)?;
    let group = gio::InetAddress::from_string(MULTICAST_GROUP).expect("a multicast address");
    s.join_multicast_group(&group, false, None)?;
    s.set_multicast_loopback(true);
    s.set_timeout(1);
    Ok(s)
}

fn heard(shared: &Arc<Shared>, data: &[u8], ip: String) {
    let Ok(info) = serde_json::from_slice::<Info>(data) else {
        return;
    };
    let ours = &shared.identity.fingerprint;
    if info
        .fingerprint
        .as_deref()
        .is_none_or(|f| f.eq_ignore_ascii_case(ours))
    {
        return;
    }
    let Some(device) = Device::from_info(&info, &ip, None) else {
        return;
    };
    log(&format!("heard {} at {ip}", device.alias));
    shared.remember(device.clone());
    if info.wants_answer() {
        let shared = shared.clone();
        std::thread::spawn(move || match client::register(&shared, &device) {
            Ok(d) => shared.remember(d),
            Err(e) => {
                log(&format!(
                    "register at {}: {e}; answering by multicast",
                    device.ip
                ));
                announce(&shared, false);
            }
        });
    }
}

fn run(shared: Arc<Shared>, port: u16) {
    let socket = match listener(port) {
        Ok(s) => s,
        Err(e) => {
            log(&format!("multicast: {}", e.message()));
            shared.event(super::Event::Error(format!(
                "Nearby devices can't be found: {}",
                e.message()
            )));
            return;
        }
    };
    let mut last: Option<Instant> = None;
    let mut sent = 0;
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        if shared.stop.load(Ordering::Relaxed) {
            return;
        }
        // Twice at start, as a packet is easily lost, then now and then.
        let due = match last {
            None => true,
            Some(t) if sent < 2 => t.elapsed() > Duration::from_secs(2),
            Some(t) => t.elapsed() > EVERY,
        };
        if due {
            announce(&shared, true);
            last = Some(Instant::now());
            sent += 1;
            shared.forget_old();
        }
        match socket.receive_from(&mut buf, gio::Cancellable::NONE) {
            Ok((n, addr)) => {
                let ip = addr
                    .downcast_ref::<gio::InetSocketAddress>()
                    .map(|a| a.address().to_str().to_string())
                    .unwrap_or_default();
                heard(&shared, &buf[..n], ip);
            }
            Err(e) if e.matches(gio::IOErrorEnum::TimedOut) => {}
            Err(e) => {
                log(&format!("multicast receive: {}", e.message()));
                std::thread::sleep(Duration::from_secs(1));
            }
        }
    }
}

pub(crate) fn start(shared: Arc<Shared>, port: u16) {
    std::thread::Builder::new()
        .name("localsend-discovery".into())
        .spawn(move || run(shared, port))
        .expect("a thread");
}
