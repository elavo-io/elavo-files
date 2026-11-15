//! LocalSend (localsend.org), so Files can send to and receive from the
//! official app and any other client of its protocol, version 2, with the
//! version 1 routes for older peers.
//!
//! Devices find each other with a multicast announcement on
//! 224.0.0.167:53317 and answer it with `POST /register` (or, failing that,
//! a multicast reply). Files go over HTTPS on port 53317: the sender asks
//! with `prepare-upload`, the receiver accepts or declines, and each file
//! is then `upload`ed with the token it was given. The certificate is
//! self-signed, made once with `openssl`, and a device's fingerprint is the
//! uppercase hex SHA-256 of it in DER form, which each side checks against
//! what the other announced. Without `openssl` Files falls back to plain
//! HTTP with a random fingerprint, as the official app does when its
//! encryption is off.
//!
//! The network runs on its own threads and talks to the app only through
//! [`Event`]s and the [`Service`] handle, so it needs no GTK.

pub mod client;
pub mod config;
mod discovery;
pub mod http;
pub mod name;
pub mod proto;
mod server;
pub mod service;
pub mod tls;

pub use config::Config;
pub use proto::{Device, DeviceType, FileMeta, Info};
pub use service::{Decision, Event, Incoming, SendState, Service, Transfer};

/// The multicast group and the port, LocalSend's defaults.
pub const MULTICAST_GROUP: &str = "224.0.0.167";
pub const DEFAULT_PORT: u16 = 53317;
/// The protocol version announced.
pub const PROTOCOL_VERSION: &str = "2.1";
/// What other devices show under the alias.
pub const DEVICE_MODEL: &str = "Elavo";
/// How long a request waits for Accept or Decline by default, in seconds
/// (`AskFor` in the settings).
pub const ASK_FOR: u32 = 300;

/// `ELAVO_FILES_LOG=1` logs the protocol to stderr, for tests.
pub(crate) fn log(msg: &str) {
    if std::env::var_os("ELAVO_FILES_LOG").is_some_and(|v| !v.is_empty() && v != "0") {
        eprintln!("elavo-files: localsend: {msg}");
    }
}
