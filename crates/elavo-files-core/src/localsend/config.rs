//! LocalSend's settings, in `~/.config/elavo/localsend.conf`:
//!
//! ```ini
//! [LocalSend]
//! Receive=true
//! SaveFolder=/home/kevin/Downloads
//! RequireApproval=true
//! PIN=
//! Port=53317
//! Encryption=true
//! AskFor=300
//! ```
//!
//! They are the device's, not Files': System Settings is meant to own them
//! (its General section) and Files to follow the file.
//!
//! The name other devices see is the computer's, set in System Settings
//! (General → About → Name), see [`super::name`]. An `Alias=` here
//! overrides it; one Files made up before (an adjective and a fruit) is
//! dropped when read.

use gio::glib;
use std::path::PathBuf;

const FILE: &str = "localsend.conf";
const GROUP: &str = "LocalSend";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    /// The name other devices show: `custom_alias`, or else the computer's.
    pub alias: String,
    /// A name set by hand in this file, instead of the computer's.
    pub custom_alias: Option<String>,
    /// Take files at all; off, every request is declined.
    pub receive: bool,
    pub save_folder: PathBuf,
    /// Ask before accepting; off is the official app's Quick Save.
    pub require_approval: bool,
    /// Senders must give this PIN; empty for none.
    pub pin: String,
    pub port: u16,
    /// HTTPS, as the official app does by default.
    pub https: bool,
    /// How long a request waits for Accept or Decline before it is
    /// declined, in seconds.
    pub ask_for: u32,
}

pub fn path() -> PathBuf {
    glib::user_config_dir().join("elavo").join(FILE)
}

/// Where the certificate and its key are kept.
pub fn identity_dir() -> PathBuf {
    glib::user_config_dir().join("elavo").join("localsend")
}

pub fn downloads() -> PathBuf {
    glib::user_special_dir(glib::UserDirectory::Downloads)
        .unwrap_or_else(|| glib::home_dir().join("Downloads"))
}

impl Default for Config {
    fn default() -> Self {
        Self {
            alias: String::new(),
            custom_alias: None,
            receive: true,
            save_folder: downloads(),
            require_approval: true,
            pin: String::new(),
            port: super::DEFAULT_PORT,
            https: true,
            ask_for: super::ASK_FOR,
        }
    }
}

impl Config {
    pub fn from_keyfile(f: &glib::KeyFile) -> Self {
        let d = Self::default();
        let s = |k: &str| {
            f.string(GROUP, k)
                .ok()
                .map(|s| s.to_string())
                .filter(|s| !s.trim().is_empty())
        };
        let b = |k: &str, def: bool| f.boolean(GROUP, k).unwrap_or(def);
        let custom_alias = s("Alias").filter(|a| !super::proto::is_random_alias(a.trim()));
        Self {
            alias: custom_alias.clone().unwrap_or(d.alias),
            custom_alias,
            receive: b("Receive", d.receive),
            save_folder: s("SaveFolder").map(PathBuf::from).unwrap_or(d.save_folder),
            require_approval: b("RequireApproval", d.require_approval),
            pin: s("PIN").unwrap_or_default(),
            port: f
                .integer(GROUP, "Port")
                .ok()
                .and_then(|p| u16::try_from(p).ok())
                .filter(|p| *p > 0)
                .unwrap_or(d.port),
            https: b("Encryption", d.https),
            ask_for: f
                .integer(GROUP, "AskFor")
                .ok()
                .and_then(|n| u32::try_from(n).ok())
                .filter(|n| *n > 0)
                .unwrap_or(d.ask_for),
        }
    }

    pub fn to_keyfile(&self, f: &glib::KeyFile) {
        match &self.custom_alias {
            Some(a) => f.set_string(GROUP, "Alias", a),
            None => {
                let _ = f.remove_key(GROUP, "Alias");
            }
        }
        f.set_boolean(GROUP, "Receive", self.receive);
        f.set_string(GROUP, "SaveFolder", &self.save_folder.to_string_lossy());
        f.set_boolean(GROUP, "RequireApproval", self.require_approval);
        f.set_string(GROUP, "PIN", &self.pin);
        f.set_integer(GROUP, "Port", i32::from(self.port));
        f.set_boolean(GROUP, "Encryption", self.https);
        if self.ask_for != super::ASK_FOR {
            f.set_integer(GROUP, "AskFor", self.ask_for as i32);
        } else {
            let _ = f.remove_key(GROUP, "AskFor");
        }
    }
}

fn load_file() -> glib::KeyFile {
    let f = glib::KeyFile::new();
    let _ = f.load_from_file(path(), glib::KeyFileFlags::KEEP_COMMENTS);
    f
}

/// The settings. Until the computer's name is known (see
/// [`super::name`]) the alias is the host name. A made-up alias left by an
/// older Files is removed from the file.
pub fn read() -> Config {
    let f = load_file();
    let mut c = Config::from_keyfile(&f);
    let stale = f
        .string(GROUP, "Alias")
        .is_ok_and(|a| super::proto::is_random_alias(a.trim()));
    if stale {
        let _ = write(&c);
    }
    if c.alias.is_empty() {
        c.alias = super::name::fallback();
    }
    c
}

pub fn write(c: &Config) -> Result<(), glib::Error> {
    let f = load_file();
    c.to_keyfile(&f);
    let path = path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    f.save_to_file(&path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_defaults() {
        let f = glib::KeyFile::new();
        let d = Config::from_keyfile(&f);
        assert!(d.receive && d.require_approval && d.https);
        assert_eq!(d.port, 53317);
        let c = Config {
            alias: "Kev".into(),
            custom_alias: Some("Kev".into()),
            receive: false,
            save_folder: "/tmp/in".into(),
            require_approval: false,
            pin: "1234".into(),
            port: 53318,
            https: false,
            ask_for: 30,
        };
        c.to_keyfile(&f);
        assert_eq!(Config::from_keyfile(&f), c);
        f.set_string(GROUP, "Port", "0");
        assert_eq!(Config::from_keyfile(&f).port, 53317);
    }

    #[test]
    fn made_up_aliases_give_way_to_the_computers_name() {
        let f = glib::KeyFile::new();
        f.set_string(GROUP, "Alias", "Efficient Avocado");
        let c = Config::from_keyfile(&f);
        assert_eq!(c.custom_alias, None);
        assert_eq!(c.alias, "");
        c.to_keyfile(&f);
        assert!(f.string(GROUP, "Alias").is_err());
        // A name typed by hand stays, as an override.
        f.set_string(GROUP, "Alias", "Kevin's Dell");
        let c = Config::from_keyfile(&f);
        assert_eq!(c.custom_alias.as_deref(), Some("Kevin's Dell"));
        assert_eq!(c.alias, "Kevin's Dell");
    }
}
