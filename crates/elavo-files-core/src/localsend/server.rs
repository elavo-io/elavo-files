//! The receiving side: the HTTP(S) API on the LocalSend port.

use gio::glib;
use std::collections::BTreeMap;
use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use super::http::{self, Request};
use super::proto::{self, Device, PrepareResponse, PrepareUpload};
use super::service::{Decision, Event, Incoming, SendState, Session, Shared, Transfer};
use super::{log, tls};

/// How long an accepted session may take.
const SESSION_FOR: Duration = Duration::from_secs(6 * 3600);

type Reply = (u16, Vec<u8>);

fn json(v: &impl serde::Serialize) -> Reply {
    (200, serde_json::to_vec(v).unwrap_or_default())
}

fn empty(status: u16) -> Reply {
    (status, Vec::new())
}

pub(crate) fn start(shared: Arc<Shared>, listener: TcpListener) {
    std::thread::Builder::new()
        .name("localsend-server".into())
        .spawn(move || loop {
            if shared.stop.load(Ordering::Relaxed) {
                return;
            }
            match listener.accept() {
                Ok((tcp, addr)) => {
                    let shared = shared.clone();
                    std::thread::spawn(move || {
                        if let Err(e) = handle(&shared, tcp, addr) {
                            log(&format!("request from {}: {e}", addr.ip()));
                        }
                    });
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(100));
                }
                Err(e) => {
                    log(&format!("accept: {e}"));
                    std::thread::sleep(Duration::from_millis(500));
                }
            }
        })
        .expect("a thread");
}

fn handle(shared: &Arc<Shared>, tcp: TcpStream, addr: SocketAddr) -> io::Result<()> {
    tcp.set_nonblocking(false)?;
    let ip = addr.ip().to_canonical().to_string();
    let mut s = tls::server(tcp, &shared.identity, 60)?;
    let Some(head) = http::read_head(&mut s.reader)? else {
        return Ok(());
    };
    let req = Request::parse(head)?;
    if req
        .head
        .header("expect")
        .is_some_and(|v| v.eq_ignore_ascii_case("100-continue"))
    {
        s.writer.write_all(b"HTTP/1.1 100 Continue\r\n\r\n")?;
        s.writer.flush()?;
    }
    if req.method != "POST" && !(req.method == "GET" && req.path.ends_with("/info")) {
        return http::write_response(&mut s.writer, 400, None, b"");
    }
    let route = req
        .path
        .trim_end_matches('/')
        .strip_prefix("/api/localsend/")
        .unwrap_or("");
    let (version, name) = route.split_once('/').unwrap_or(("", ""));
    let v1 = version == "v1";
    log(&format!("{} {} from {ip}", req.method, req.path));
    let peer = s.peer.clone();
    let body = http::body(&mut s.reader, &req.head, false);
    let (status, reply) = match (version, name) {
        ("v1" | "v2", "info") => json(&shared.info()),
        ("v1" | "v2", "register") => register(shared, body, &ip, peer.as_deref()),
        ("v2", "prepare-upload") | ("v1", "send-request") => {
            prepare(shared, &req, body, &ip, peer.as_deref(), v1)
        }
        ("v2", "upload") | ("v1", "send") => upload(shared, &req, body, &ip, v1),
        ("v1" | "v2", "cancel") => cancel(shared, &req, &ip),
        _ => empty(404),
    };
    log(&format!("  -> {status}"));
    let kind = (!reply.is_empty()).then_some("application/json");
    http::write_response(&mut s.writer, status, kind, &reply)
}

fn register(shared: &Shared, body: Box<dyn Read + '_>, ip: &str, peer: Option<&str>) -> Reply {
    let Ok(data) = http::read_all(body, 64 * 1024) else {
        return empty(400);
    };
    let Ok(info) = serde_json::from_slice::<proto::Info>(&data) else {
        return empty(400);
    };
    if let Some(device) = Device::from_info(&info, ip, peer) {
        log(&format!("registered {} at {ip}", device.alias));
        shared.remember(device);
    }
    json(&shared.info())
}

fn prepare(
    shared: &Shared,
    req: &Request,
    body: Box<dyn Read + '_>,
    ip: &str,
    peer: Option<&str>,
    v1: bool,
) -> Reply {
    let config = shared.config();
    // Previews of text messages ride along, so allow a few megabytes.
    let Ok(data) = http::read_all(body, 16 * 1024 * 1024) else {
        return empty(400);
    };
    let Ok(request) = serde_json::from_slice::<PrepareUpload>(&data) else {
        return empty(400);
    };
    if !config.receive {
        return empty(403);
    }
    let sender = Device::from_info(&request.info, ip, peer).unwrap_or_else(|| Device {
        alias: request.info.alias.clone(),
        model: request.info.device_model.clone(),
        kind: proto::DeviceType::parse(request.info.device_type.as_deref()),
        fingerprint: ip.to_owned(),
        ip: ip.to_owned(),
        port: request.info.port.unwrap_or(super::DEFAULT_PORT),
        https: shared.identity.https(),
        version: request.info.version.clone().unwrap_or_else(|| "1.0".into()),
    });
    // Favorites Only: anyone else is turned away before being asked for
    // a PIN or shown to the person.
    if config.declines(&sender.fingerprint) {
        log(&format!("declined {}: not a favorite", sender.alias));
        return empty(403);
    }
    if !config.pin_ok(req.param("pin")) {
        return empty(401);
    }
    if request.files.is_empty()
        || request.files.len() > 10000
        || request
            .files
            .values()
            .any(|f| proto::safe_relative(&f.file_name).is_none())
    {
        return empty(400);
    }
    let files: Vec<_> = request.files.values().cloned().collect();
    let id = proto::random_id();
    let Some(bytes) = files
        .iter()
        .try_fold(0u64, |sum, f| sum.checked_add(f.size))
    else {
        return empty(400);
    };
    // A text message is shown, not saved: nothing to transfer.
    if files.len() == 1 && files[0].is_message() {
        shared.event(Event::Incoming(Incoming {
            id,
            sender,
            files,
            bytes,
            accepted: true,
            expires_in: None,
        }));
        return empty(204);
    }
    // One transfer at a time, as in the official app.
    if shared.busy.swap(true, Ordering::SeqCst) {
        return empty(409);
    }
    if !shared.sessions.lock().unwrap().is_empty() {
        shared.busy.store(false, Ordering::SeqCst);
        return empty(409);
    }
    let decision = if config.asks(&sender.fingerprint) {
        let ask_for = Duration::from_secs(u64::from(config.ask_for));
        let (tx, rx) = mpsc::channel();
        shared
            .decisions
            .lock()
            .unwrap()
            .insert(id.clone(), (tx, ip.to_owned()));
        shared.event(Event::Incoming(Incoming {
            id: id.clone(),
            sender: sender.clone(),
            files: files.clone(),
            bytes,
            accepted: false,
            expires_in: Some(ask_for),
        }));
        let started = Instant::now();
        loop {
            match rx.recv_timeout(Duration::from_millis(500)) {
                Ok(d) => break d,
                Err(mpsc::RecvTimeoutError::Disconnected) => break Decision::Decline,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if started.elapsed() > ask_for || shared.stop.load(Ordering::Relaxed) {
                        shared.decisions.lock().unwrap().remove(&id);
                        shared.event(Event::Dismiss(id.clone()));
                        break Decision::Decline;
                    }
                }
            }
        }
    } else {
        shared.event(Event::Incoming(Incoming {
            id: id.clone(),
            sender: sender.clone(),
            files: files.clone(),
            bytes,
            accepted: true,
            expires_in: None,
        }));
        Decision::Accept
    };
    if decision == Decision::Decline {
        shared.busy.store(false, Ordering::SeqCst);
        log("declined");
        return empty(403);
    }
    let tokens: BTreeMap<String, String> = request
        .files
        .keys()
        .map(|k| (k.clone(), proto::random_id()))
        .collect();
    let session = Session {
        ip: ip.to_owned(),
        v1,
        sender: sender.alias.clone(),
        total: bytes,
        done: 0,
        saved: Vec::new(),
        last_event: Instant::now(),
        files: request
            .files
            .iter()
            .map(|(k, f)| (k.clone(), (f.clone(), tokens[k].clone())))
            .collect(),
        folder: config.save_folder.clone(),
        cancelled: Arc::new(AtomicBool::new(false)),
        expires: Instant::now() + SESSION_FOR,
    };
    shared.sessions.lock().unwrap().insert(id.clone(), session);
    shared.busy.store(false, Ordering::SeqCst);
    log(&format!(
        "accepted {} files from {}",
        tokens.len(),
        sender.alias
    ));
    if v1 {
        json(&tokens)
    } else {
        json(&PrepareResponse {
            session_id: id,
            files: tokens,
        })
    }
}

/// Where a received file goes: its folders made, its name made unique.
fn destination(folder: &Path, name: &str) -> Option<(PathBuf, PathBuf)> {
    let rel = proto::safe_relative(name)?;
    std::fs::create_dir_all(folder).ok()?;
    // Never follow a folder supplied by a peer through a local symlink.
    let mut dir = std::fs::canonicalize(folder).ok()?;
    for component in rel.parent().unwrap_or(Path::new("")).components() {
        dir.push(component);
        match std::fs::symlink_metadata(&dir) {
            Ok(md) if md.is_dir() && !md.file_type().is_symlink() => {}
            Ok(_) => return None,
            Err(e) if e.kind() == io::ErrorKind::NotFound => std::fs::create_dir(&dir).ok()?,
            Err(_) => return None,
        }
    }
    let base = rel.file_name()?.to_string_lossy().into_owned();
    let unique = if dir.join(&base).exists() {
        crate::naming::keep_both(&base, false, |c| dir.join(c).exists())
    } else {
        base
    };
    let top = match rel.components().count() {
        1 => dir.join(&unique),
        _ => folder.join(rel.components().next()?),
    };
    Some((dir.join(unique), top))
}

fn set_modified(path: &Path, iso: Option<&str>) {
    let Some(when) = iso.and_then(|s| glib::DateTime::from_iso8601(s, None).ok()) else {
        return;
    };
    let secs = when.to_unix();
    if secs <= 0 {
        return;
    }
    if let Ok(f) = std::fs::File::options().write(true).open(path) {
        let _ = f.set_modified(std::time::UNIX_EPOCH + Duration::from_secs(secs as u64));
    }
}

fn upload(
    shared: &Shared,
    req: &Request,
    mut body: Box<dyn Read + '_>,
    ip: &str,
    v1: bool,
) -> Reply {
    let (Some(file_id), Some(token)) = (req.param("fileId"), req.param("token")) else {
        return empty(400);
    };
    let found = {
        let mut sessions = shared.sessions.lock().unwrap();
        let found_id = if v1 {
            sessions
                .iter()
                .find(|(_, s)| s.v1 && s.ip == ip)
                .map(|(id, _)| id.clone())
        } else {
            req.param("sessionId").map(str::to_owned)
        };
        let session = found_id
            .as_ref()
            .and_then(|id| sessions.get_mut(id).map(|s| (id, s)));
        match session {
            None => None,
            Some((id, s)) => match s.files.get_mut(file_id) {
                Some((meta, t)) if t == token && s.ip == ip && !t.is_empty() => {
                    t.clear();
                    Some((
                        id.clone(),
                        meta.clone(),
                        s.folder.clone(),
                        s.cancelled.clone(),
                        s.sender.clone(),
                    ))
                }
                _ => return empty(403),
            },
        }
    };
    let Some((id, meta, folder, cancelled, sender)) = found else {
        return empty(if req.param("sessionId").is_none() && !v1 {
            400
        } else {
            403
        });
    };
    let Some((mut dest, mut top)) = destination(&folder, &meta.file_name) else {
        return empty(400);
    };
    let part = dest.with_file_name(format!(
        ".{}.localsend-{}.part",
        dest.file_name().unwrap_or_default().to_string_lossy(),
        &token[..token.len().min(8)]
    ));
    let fail = |status: u16| {
        let _ = std::fs::remove_file(&part);
        if let Some(s) = shared.sessions.lock().unwrap().remove(&id) {
            s.cancelled.store(true, Ordering::Relaxed);
            shared.event(Event::Transfer(Transfer {
                id: id.clone(),
                device: s.sender,
                receiving: true,
                file_name: meta.file_name.clone(),
                done: s.done,
                total: s.total,
                state: SendState::Error(format!(
                    "Couldn't receive “{}” ({status}).",
                    meta.file_name
                )),
                saved: s.saved,
            }));
        }
        empty(status)
    };
    let Ok(mut out) = std::fs::File::options()
        .write(true)
        .create_new(true)
        .open(&part)
    else {
        return empty(500);
    };
    let mut sum = glib::Checksum::new(glib::ChecksumType::Sha256).expect("SHA-256");
    let mut buf = vec![0u8; 256 * 1024];
    let mut got: u64 = 0;
    loop {
        if cancelled.load(Ordering::Relaxed) || shared.stop.load(Ordering::Relaxed) {
            return fail(409);
        }
        let n = match body.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) => {
                log(&format!("upload read: {e}"));
                return fail(500);
            }
        };
        // No more than the file offered, even with a chunked body.
        if got.saturating_add(n as u64) > meta.size {
            return fail(400);
        }
        if out.write_all(&buf[..n]).is_err() {
            return fail(500);
        }
        if meta.sha256.is_some() {
            sum.update(&buf[..n]);
        }
        got += n as u64;
        progress(shared, &id, n as u64, &meta.file_name, &sender, false);
    }
    drop(out);
    if got != meta.size {
        log(&format!("{}: {got} of {} bytes", meta.file_name, meta.size));
        return fail(400);
    }
    if let Some(want) = &meta.sha256 {
        if !sum.string().unwrap_or_default().eq_ignore_ascii_case(want) {
            return fail(422);
        }
    }
    // Link without replacing anything. If another file arrived with the
    // same name meanwhile, pick another and try again.
    loop {
        match std::fs::hard_link(&part, &dest) {
            Ok(()) => {
                let _ = std::fs::remove_file(&part);
                break;
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                let Some((next, next_top)) = destination(&folder, &meta.file_name) else {
                    return fail(500);
                };
                dest = next;
                top = next_top;
            }
            Err(_) => return fail(500),
        }
    }
    set_modified(
        &dest,
        meta.metadata.as_ref().and_then(|m| m.modified.as_deref()),
    );
    log(&format!("saved {}", dest.display()));
    let finished = {
        let mut sessions = shared.sessions.lock().unwrap();
        let Some(s) = sessions.get_mut(&id) else {
            return empty(200);
        };
        s.files.remove(file_id);
        if !s.saved.contains(&top) {
            s.saved.push(top);
        }
        if s.files.is_empty() {
            sessions.remove(&id)
        } else {
            None
        }
    };
    if let Some(s) = finished {
        shared.event(Event::Transfer(Transfer {
            id,
            device: s.sender,
            receiving: true,
            file_name: meta.file_name,
            done: s.total,
            total: s.total,
            state: SendState::Done,
            saved: s.saved,
        }));
    }
    empty(200)
}

/// Count `n` more bytes, telling the UI now and then.
fn progress(shared: &Shared, id: &str, n: u64, name: &str, sender: &str, force: bool) {
    let mut sessions = shared.sessions.lock().unwrap();
    let Some(s) = sessions.get_mut(id) else {
        return;
    };
    s.done += n;
    if !force && s.last_event.elapsed() < Duration::from_millis(150) {
        return;
    }
    s.last_event = Instant::now();
    shared.event(Event::Transfer(Transfer {
        id: id.to_owned(),
        device: sender.to_owned(),
        receiving: true,
        file_name: name.to_owned(),
        done: s.done,
        total: s.total,
        state: SendState::Sending,
        saved: Vec::new(),
    }));
}

fn cancel(shared: &Shared, req: &Request, ip: &str) -> Reply {
    let mut sessions = shared.sessions.lock().unwrap();
    let id = req.param("sessionId").map(str::to_owned).or_else(|| {
        sessions
            .iter()
            .find(|(_, s)| s.ip == ip)
            .map(|(k, _)| k.clone())
    });
    if let Some(s) = id.as_ref().and_then(|id| match sessions.get(id) {
        Some(s) if s.ip == ip => sessions.remove(id),
        _ => None,
    }) {
        s.cancelled.store(true, Ordering::Relaxed);
        shared.event(Event::Transfer(Transfer {
            id: id.unwrap_or_default(),
            device: s.sender,
            receiving: true,
            file_name: String::new(),
            done: s.done,
            total: s.total,
            state: SendState::Cancelled,
            saved: s.saved,
        }));
        return empty(200);
    }
    drop(sessions);
    // Cancelled before an answer: the sender has no session id yet.
    let waiting: Vec<String> = shared
        .decisions
        .lock()
        .unwrap()
        .iter()
        .filter(|(_, (_, from))| from == ip)
        .map(|(k, _)| k.clone())
        .collect();
    for id in waiting {
        if let Some((tx, _)) = shared.decisions.lock().unwrap().remove(&id) {
            let _ = tx.send(Decision::Decline);
        }
        shared.event(Event::Dismiss(id));
    }
    empty(200)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn received_paths_do_not_traverse_links_or_overwrite() {
        let dir = std::env::temp_dir().join(format!("ls-paths-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("in")).unwrap();
        std::fs::create_dir_all(dir.join("elsewhere")).unwrap();
        std::os::unix::fs::symlink(dir.join("elsewhere"), dir.join("in/link")).unwrap();
        assert!(destination(&dir.join("in"), "link/x.txt").is_none());
        assert!(destination(&dir.join("in"), "../x.txt").is_none());
        let (one, _) = destination(&dir.join("in"), "sub/x.txt").unwrap();
        std::fs::write(&one, b"one").unwrap();
        let (two, _) = destination(&dir.join("in"), "sub/x.txt").unwrap();
        assert_ne!(one, two);
        assert_eq!(two.file_name().unwrap(), "x 2.txt");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
