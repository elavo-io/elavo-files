//! The device's certificate, and the connections over it.
//!
//! The certificate is self-signed, made once with `openssl` and kept in
//! `~/.config/elavo/localsend/`. Its fingerprint, the uppercase hex SHA-256
//! of its DER form, is the device's id: peers check it against what was
//! announced, as Files does with theirs. Both sides show a certificate,
//! since the official app asks the sender for one too.

use gio::glib;
use std::io::{self, BufReader, Read, Write};
use std::net::TcpStream;
use std::os::fd::OwnedFd;
use std::path::Path;
use std::process::Command;

use gio::prelude::*;

/// Who this device is: its certificate (and key), PEM, and fingerprint.
#[derive(Clone, Debug)]
pub struct Identity {
    pub pem: Option<String>,
    pub fingerprint: String,
}

impl Identity {
    pub fn https(&self) -> bool {
        self.pem.is_some()
    }

    /// A fresh certificate object: GIO's are made per thread.
    pub fn certificate(&self) -> Option<gio::TlsCertificate> {
        gio::TlsCertificate::from_pem(self.pem.as_deref()?).ok()
    }

    /// Plain HTTP: a random fingerprint, as the protocol has it.
    pub fn plain() -> Self {
        Self {
            pem: None,
            fingerprint: super::proto::random_id().replace('-', "").to_uppercase(),
        }
    }
}

/// The uppercase hex SHA-256 of a certificate's DER form.
pub fn fingerprint(cert: &gio::TlsCertificate) -> Option<String> {
    let der = cert.certificate()?;
    Some(sha256_hex(&der))
}

pub fn sha256_hex(data: &[u8]) -> String {
    let mut sum =
        glib::Checksum::new(glib::ChecksumType::Sha256).expect("SHA-256 is always available");
    sum.update(data);
    sum.string().unwrap_or_default().to_uppercase()
}

fn make(dir: &Path) -> io::Result<()> {
    std::fs::create_dir_all(dir)?;
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    let status = Command::new("openssl")
        .args(["req", "-x509", "-newkey", "ec", "-pkeyopt"])
        .arg("ec_paramgen_curve:prime256v1")
        .args(["-nodes", "-days", "3650", "-subj", "/CN=LocalSend User"])
        .arg("-keyout")
        .arg(dir.join("key.pem"))
        .arg("-out")
        .arg(dir.join("cert.pem"))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()?;
    if !status.success() {
        return Err(io::Error::other("openssl couldn't make a certificate"));
    }
    std::fs::set_permissions(dir.join("key.pem"), std::fs::Permissions::from_mode(0o600))
}

/// The certificate in `dir`, made the first time; plain HTTP when it
/// can't be had or `https` is off.
pub fn identity(dir: &Path, https: bool) -> Identity {
    if !https {
        return Identity::plain();
    }
    let load = || -> Option<Identity> {
        let cert = std::fs::read_to_string(dir.join("cert.pem")).ok()?;
        let key = std::fs::read_to_string(dir.join("key.pem")).ok()?;
        let pem = format!("{cert}\n{key}");
        let parsed = gio::TlsCertificate::from_pem(&pem).ok()?;
        Some(Identity {
            fingerprint: fingerprint(&parsed)?,
            pem: Some(pem),
        })
    };
    if let Some(id) = load() {
        return id;
    }
    if let Err(err) = make(dir) {
        super::log(&format!("no certificate, using HTTP: {err}"));
        return Identity::plain();
    }
    load().unwrap_or_else(Identity::plain)
}

/// A connection, plain or TLS, read through a buffer.
pub struct Stream {
    pub reader: BufReader<Box<dyn Read>>,
    pub writer: Box<dyn Write>,
    /// The fingerprint of the certificate the peer showed, if any.
    pub peer: Option<String>,
    _tls: Option<gio::TlsConnection>,
}

fn gio_err(e: glib::Error) -> io::Error {
    io::Error::other(e.message().to_owned())
}

fn base(tcp: TcpStream, timeout_s: u32) -> io::Result<gio::SocketConnection> {
    let socket = gio::Socket::from_fd(OwnedFd::from(tcp)).map_err(gio_err)?;
    socket.set_timeout(timeout_s);
    Ok(socket.connection_factory_create_connection())
}

fn over(tls: gio::TlsConnection) -> io::Result<Stream> {
    tls.set_require_close_notify(false);
    tls.handshake(gio::Cancellable::NONE).map_err(gio_err)?;
    let peer = tls.peer_certificate().as_ref().and_then(fingerprint);
    let reader: Box<dyn Read> = Box::new(tls.input_stream().into_read());
    let writer: Box<dyn Write> = Box::new(tls.output_stream().into_write());
    Ok(Stream {
        reader: BufReader::with_capacity(64 * 1024, reader),
        writer,
        peer,
        _tls: Some(tls),
    })
}

fn plain(tcp: TcpStream, timeout_s: u32) -> io::Result<Stream> {
    let t = (timeout_s > 0).then(|| std::time::Duration::from_secs(u64::from(timeout_s)));
    tcp.set_read_timeout(t)?;
    tcp.set_write_timeout(t)?;
    let writer: Box<dyn Write> = Box::new(tcp.try_clone()?);
    let reader: Box<dyn Read> = Box::new(tcp);
    Ok(Stream {
        reader: BufReader::with_capacity(64 * 1024, reader),
        writer,
        peer: None,
        _tls: None,
    })
}

/// The server's side of a connection just accepted.
pub fn server(tcp: TcpStream, id: &Identity, timeout_s: u32) -> io::Result<Stream> {
    let Some(cert) = id.certificate() else {
        return plain(tcp, timeout_s);
    };
    let conn = base(tcp, timeout_s)?;
    let tls = gio::TlsServerConnection::new(&conn, Some(&cert)).map_err(gio_err)?;
    // Ask for the sender's certificate, as the official app does, and take
    // any: it is checked against the fingerprint, not a CA.
    tls.set_authentication_mode(gio::TlsAuthenticationMode::Requested);
    tls.connect_accept_certificate(|_, _, _| true);
    over(tls.upcast())
}

/// The client's side: over TLS when `https`, taking only the certificate
/// whose fingerprint is `expect`, when given.
pub fn client(
    tcp: TcpStream,
    id: &Identity,
    https: bool,
    expect: Option<&str>,
    timeout_s: u32,
) -> io::Result<Stream> {
    if !https {
        return plain(tcp, timeout_s);
    }
    let conn = base(tcp, timeout_s)?;
    let tls =
        gio::TlsClientConnection::new(&conn, None::<&gio::SocketConnectable>).map_err(gio_err)?;
    if let Some(cert) = id.certificate() {
        tls.set_certificate(&cert);
    }
    let expect_after = expect.map(str::to_owned);
    let expect = expect_after.clone();
    tls.connect_accept_certificate(move |_, cert, _| match &expect {
        Some(want) => fingerprint(cert).is_some_and(|got| got.eq_ignore_ascii_case(want)),
        None => true,
    });
    let stream = over(tls.upcast())?;
    // A CA-signed peer may pass without accept-certificate being emitted;
    // it must still be the device picked, not just any valid certificate.
    if let Some(want) = expect_after {
        if stream
            .peer
            .as_ref()
            .is_none_or(|got| !got.eq_ignore_ascii_case(&want))
        {
            return Err(io::Error::other("The device's certificate changed."));
        }
    }
    Ok(stream)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_like_localsend() {
        assert_eq!(
            sha256_hex(b"abc"),
            "BA7816BF8F01CFEA414140DE5DAE2223B00361A396177A9CB410FF61F20015AD"
        );
        let p = Identity::plain();
        assert!(!p.https());
        assert_eq!(p.fingerprint.len(), 32);
    }
}
