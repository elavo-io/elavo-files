//! The sending side: register with a device, and send it files.

use gio::glib;
use std::collections::BTreeMap;
use std::io::{self, Read, Write};
use std::net::{IpAddr, SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::http;
use super::proto::{self, Device, FileMeta, Info, Metadata, PrepareResponse, PrepareUpload};
use super::service::{Event, SendState, Shared, Transfer};
use super::{log, tls};

fn connect(shared: &Shared, dev: &Device, pinned: bool, timeout_s: u32) -> io::Result<tls::Stream> {
    let ip: IpAddr = dev
        .ip
        .parse()
        .map_err(|_| io::Error::other(format!("bad address {}", dev.ip)))?;
    let tcp = TcpStream::connect_timeout(&SocketAddr::new(ip, dev.port), Duration::from_secs(4))?;
    let expect = pinned.then_some(dev.fingerprint.as_str());
    tls::client(tcp, &shared.identity, dev.https, expect, timeout_s)
}

fn write_head(
    w: &mut dyn Write,
    dev: &Device,
    path: &str,
    kind: &str,
    length: u64,
) -> io::Result<()> {
    let host = match dev.ip.contains(':') {
        true => format!("[{}]:{}", dev.ip, dev.port),
        false => format!("{}:{}", dev.ip, dev.port),
    };
    write!(
        w,
        "POST {path} HTTP/1.1\r\nHost: {host}\r\nUser-Agent: Elavo Files\r\nAccept: */*\r\n\
         Connection: close\r\nContent-Type: {kind}\r\nContent-Length: {length}\r\n\r\n"
    )
}

fn response(s: &mut tls::Stream) -> io::Result<(u16, Vec<u8>)> {
    let head = http::read_head(&mut s.reader)?
        .ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "no response"))?;
    let status = http::status_of(&head)?;
    let body = http::read_all(http::body(&mut s.reader, &head, true), 16 * 1024 * 1024)?;
    Ok((status, body))
}

/// A JSON POST, and its answer.
fn post(
    shared: &Shared,
    dev: &Device,
    pinned: bool,
    path: &str,
    body: &[u8],
    timeout_s: u32,
) -> io::Result<(u16, Vec<u8>)> {
    let mut s = connect(shared, dev, pinned, timeout_s)?;
    write_head(
        &mut s.writer,
        dev,
        path,
        "application/json",
        body.len() as u64,
    )?;
    s.writer.write_all(body)?;
    s.writer.flush()?;
    response(&mut s)
}

/// Tell `dev` who this device is; it answers with who it is.
pub(crate) fn register(shared: &Shared, dev: &Device) -> io::Result<Device> {
    let body = serde_json::to_vec(&shared.info())?;
    let mut s = connect(shared, dev, false, 5)?;
    write_head(
        &mut s.writer,
        dev,
        &dev.api("register"),
        "application/json",
        body.len() as u64,
    )?;
    s.writer.write_all(&body)?;
    s.writer.flush()?;
    let (status, reply) = response(&mut s)?;
    if status != 200 {
        return Err(io::Error::other(format!("register answered {status}")));
    }
    let mut info: Info = serde_json::from_slice(&reply)?;
    // The answer leaves out how to reach it again.
    info.port = info.port.or(Some(dev.port));
    info.protocol = info
        .protocol
        .or_else(|| Some(if dev.https { "https" } else { "http" }.to_owned()));
    info.version = info.version.or_else(|| Some(dev.version.clone()));
    Device::from_info(&info, &dev.ip, s.peer.as_deref())
        .ok_or_else(|| io::Error::other("no fingerprint"))
}

fn mime(path: &Path) -> String {
    let (kind, _) = gio::content_type_guess(Some(path), None);
    gio::content_type_get_mime_type(&kind)
        .map(|m| m.to_string())
        .unwrap_or_else(|| "application/octet-stream".to_owned())
}

fn iso(t: io::Result<std::time::SystemTime>) -> Option<String> {
    let secs = t
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs();
    let dt = glib::DateTime::from_unix_utc(secs as i64).ok()?;
    dt.format_iso8601().ok().map(|s| s.to_string())
}

fn meta_for(path: &Path, name: String, md: &std::fs::Metadata) -> FileMeta {
    FileMeta {
        id: proto::random_id(),
        file_name: name,
        size: md.len(),
        file_type: mime(path),
        sha256: None,
        preview: None,
        metadata: Some(Metadata {
            modified: iso(md.modified()),
            accessed: iso(md.accessed()),
        }),
    }
}

fn walk(dir: &Path, prefix: &str, out: &mut Vec<(FileMeta, PathBuf)>) {
    let Ok(read) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = read.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let path = e.path();
        let name = format!("{prefix}/{}", e.file_name().to_string_lossy());
        let Ok(link) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if link.is_dir() {
            walk(&path, &name, out);
        } else if let Ok(md) = std::fs::metadata(&path) {
            // Links to files go as the file; links to folders are skipped.
            if md.is_file() {
                out.push((meta_for(&path, name, &md), path));
            }
        }
    }
}

/// Every file to send: folders become their files, named with the
/// folder's relative path, as LocalSend sends a folder.
pub fn collect(paths: &[PathBuf]) -> Vec<(FileMeta, PathBuf)> {
    let mut out = Vec::new();
    for p in paths {
        let Ok(md) = std::fs::metadata(p) else {
            continue;
        };
        let name = p
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "file".into());
        if md.is_dir() {
            walk(p, &name, &mut out);
        } else if md.is_file() {
            out.push((meta_for(p, name, &md), p.clone()));
        }
    }
    out
}

struct Reporter<'a> {
    shared: &'a Shared,
    id: &'a str,
    device: String,
    file_name: String,
    done: u64,
    total: u64,
    last: Instant,
}

impl Reporter<'_> {
    fn emit(&mut self, state: SendState) {
        self.last = Instant::now();
        self.shared.event(Event::Transfer(Transfer {
            id: self.id.to_owned(),
            device: self.device.clone(),
            receiving: false,
            file_name: self.file_name.clone(),
            done: self.done,
            total: self.total,
            state,
            saved: Vec::new(),
        }));
    }

    fn tick(&mut self, n: u64) {
        self.done += n;
        if self.last.elapsed() > Duration::from_millis(150) {
            self.emit(SendState::Sending);
        }
    }
}

fn upload_one(
    shared: &Shared,
    dev: &Device,
    path: &str,
    file: &Path,
    size: u64,
    cancel: &AtomicBool,
    report: &mut Reporter,
) -> io::Result<u16> {
    let mut s = connect(shared, dev, true, 60)?;
    write_head(&mut s.writer, dev, path, "application/octet-stream", size)?;
    let mut f = std::fs::File::open(file)?;
    let mut buf = vec![0u8; 256 * 1024];
    let mut left = size;
    while left > 0 {
        if cancel.load(Ordering::Relaxed) {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
        }
        let want = buf.len().min(left as usize);
        let n = f.read(&mut buf[..want])?;
        if n == 0 {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "file shrank"));
        }
        s.writer.write_all(&buf[..n])?;
        left -= n as u64;
        report.tick(n as u64);
    }
    s.writer.flush()?;
    Ok(response(&mut s)?.0)
}

/// Send `paths` to `dev`, telling the UI how it goes under `id`.
pub(crate) fn send(
    shared: &Shared,
    id: &str,
    dev: &Device,
    paths: Vec<PathBuf>,
    pin: Option<String>,
    cancel: Arc<AtomicBool>,
) {
    let mut report = Reporter {
        shared,
        id,
        device: dev.alias.clone(),
        file_name: String::new(),
        done: 0,
        total: 0,
        last: Instant::now(),
    };
    report.emit(SendState::Preparing);
    let files = collect(&paths);
    if files.is_empty() {
        report.emit(SendState::Error("There's nothing to send.".into()));
        return;
    }
    report.total = files.iter().map(|(m, _)| m.size).sum();
    report.file_name = files[0].0.file_name.clone();
    let mut info = shared.info();
    let mut metas: BTreeMap<String, FileMeta> = BTreeMap::new();
    for (m, _) in &files {
        let mut m = m.clone();
        if dev.is_v1() {
            m.file_type = proto::v1_file_type(&m.file_type).to_owned();
            m.metadata = None;
        }
        metas.insert(m.id.clone(), m);
    }
    if dev.is_v1() {
        info.version = None;
        info.port = None;
        info.protocol = None;
        info.download = None;
    }
    let body = match serde_json::to_vec(&PrepareUpload { info, files: metas }) {
        Ok(b) => b,
        Err(e) => return report.emit(SendState::Error(e.to_string())),
    };
    let mut path = dev.api(if dev.is_v1() {
        "send-request"
    } else {
        "prepare-upload"
    });
    if let Some(pin) = pin.as_deref().filter(|p| !p.is_empty()) {
        path.push_str(&format!("?pin={}", http::percent_encode(pin)));
    }
    report.emit(SendState::Waiting);
    log(&format!(
        "asking {} to take {} files",
        dev.alias,
        files.len()
    ));
    // The person on the other side may take a while to answer.
    let (status, reply) = match post(shared, dev, true, &path, &body, 330) {
        Ok(r) => r,
        Err(e) => return report.emit(SendState::Error(e.to_string())),
    };
    log(&format!("prepare answered {status}"));
    let tokens: BTreeMap<String, String>;
    let session: Option<String>;
    match status {
        200 if dev.is_v1() => {
            session = None;
            tokens = serde_json::from_slice(&reply).unwrap_or_default();
        }
        200 => match serde_json::from_slice::<PrepareResponse>(&reply) {
            Ok(r) => {
                session = Some(r.session_id);
                tokens = r.files;
            }
            Err(e) => return report.emit(SendState::Error(e.to_string())),
        },
        204 => return report.emit(SendState::Done),
        401 => return report.emit(SendState::NeedsPin),
        403 => return report.emit(SendState::Declined),
        409 => return report.emit(SendState::Busy),
        429 => {
            return report.emit(SendState::Error(
                "Too many requests. Try again soon.".into(),
            ))
        }
        s => return report.emit(SendState::Error(format!("The device answered {s}."))),
    }
    let cancel_session = |shared: &Shared| {
        let mut p = dev.api("cancel");
        if let Some(s) = &session {
            p.push_str(&format!("?sessionId={}", http::percent_encode(s)));
        }
        let _ = post(shared, dev, true, &p, b"", 5);
    };
    if cancel.load(Ordering::Relaxed) {
        cancel_session(shared);
        return report.emit(SendState::Cancelled);
    }
    report.emit(SendState::Sending);
    for (meta, file) in &files {
        let Some(token) = tokens.get(&meta.id) else {
            // Not wanted (a partial accept).
            report.done += meta.size;
            continue;
        };
        report.file_name = meta.file_name.clone();
        let mut p = format!(
            "{}?fileId={}&token={}",
            dev.api(if dev.is_v1() { "send" } else { "upload" }),
            http::percent_encode(&meta.id),
            http::percent_encode(token)
        );
        if let Some(s) = &session {
            p.push_str(&format!("&sessionId={}", http::percent_encode(s)));
        }
        match upload_one(shared, dev, &p, file, meta.size, &cancel, &mut report) {
            Ok(200) => {}
            Ok(s) => {
                cancel_session(shared);
                return report.emit(SendState::Error(format!(
                    "“{}” wasn't taken ({s}).",
                    meta.file_name
                )));
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {
                cancel_session(shared);
                return report.emit(SendState::Cancelled);
            }
            Err(e) => {
                cancel_session(shared);
                return report.emit(SendState::Error(e.to_string()));
            }
        }
    }
    report.done = report.total;
    log(&format!("sent {} files to {}", files.len(), dev.alias));
    report.emit(SendState::Done);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collects_folders_as_relative_names() {
        let dir = std::env::temp_dir().join(format!("ls-collect-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Proj/sub")).unwrap();
        std::fs::write(dir.join("Proj/a.txt"), b"aa").unwrap();
        std::fs::write(dir.join("Proj/sub/b.png"), b"bbb").unwrap();
        std::fs::write(dir.join("c.pdf"), b"c").unwrap();
        let got = collect(&[dir.join("Proj"), dir.join("c.pdf")]);
        let names: Vec<_> = got.iter().map(|(m, _)| m.file_name.as_str()).collect();
        assert_eq!(names, ["Proj/a.txt", "Proj/sub/b.png", "c.pdf"]);
        assert_eq!(got[1].0.size, 3);
        assert_eq!(got[2].0.file_type, "application/pdf");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
