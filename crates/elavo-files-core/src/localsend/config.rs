//! LocalSend's settings, in `~/.config/elavo/localsend.conf`:
//!
//! ```ini
//! [LocalSend]
//! Receive=true
//! FavoritesOnly=false
//! AutoAcceptFavorites=true
//! SaveFolder=/home/kevin/Downloads
//! RequireApproval=true
//! RequirePIN=false
//! Port=53317
//! Encryption=true
//! AskFor=300
//!
//! [Favorites]
//! 3F2A…(SHA-256 fingerprint)=Kevin's iPhone
//! ```
//!
//! They are the device's, not Files': System Settings (General → Sharing)
//! owns them, and Files follows the file as it changes.
//!
//! Receive from Off / Favorites Only / Everyone is `Receive` and
//! `FavoritesOnly`, so an older Files still reads `Receive`. LocalSend has
//! no favorites-only mode of its own: Elavo declines requests from devices
//! whose fingerprint is not a favorite.
//!
//! The PIN is in the keyring ([`crate::secret`], setting
//! [`PIN_SECRET`]). A `PIN=` left in this file by an older Files is moved
//! there the first time it is read, see [`resolve_pin`].
//!
//! The name other devices see is the computer's, set in System Settings
//! (General → About → Name), see [`super::name`]. An `Alias=` here
//! overrides it; one Files made up before (an adjective and a fruit) is
//! dropped when read.

use gio::glib;
use std::path::PathBuf;

const FILE: &str = "localsend.conf";
const GROUP: &str = "LocalSend";
const FAVORITES: &str = "Favorites";
/// The keyring entry for the PIN.
pub const PIN_SECRET: &str = "localsend-pin";

/// A device trusted by its fingerprint, with the name it had when added.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Favorite {
    pub fingerprint: String,
    pub alias: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    /// The name other devices show: `custom_alias`, or else the computer's.
    pub alias: String,
    /// A name set by hand in this file, instead of the computer's.
    pub custom_alias: Option<String>,
    /// Take files at all; off, every request is declined.
    pub receive: bool,
    /// Take files only from `favorites`; others are declined.
    pub favorites_only: bool,
    /// Accept from favorites without asking, the official app's Quick Save
    /// for Favorites.
    pub auto_accept_favorites: bool,
    pub favorites: Vec<Favorite>,
    pub save_folder: PathBuf,
    /// Ask before accepting; off is the official app's Quick Save.
    pub require_approval: bool,
    /// Senders must give `pin`.
    pub require_pin: bool,
    /// The PIN, from the keyring (or a `PIN=` an older Files left in the
    /// file, until it is moved). Never written to the file.
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
            favorites_only: false,
            auto_accept_favorites: true,
            favorites: Vec::new(),
            save_folder: downloads(),
            require_approval: true,
            require_pin: false,
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
        let legacy_pin = s("PIN").unwrap_or_default();
        let favorites = f
            .keys(FAVORITES)
            .map(|keys| {
                keys.iter()
                    .filter(|k| !k.trim().is_empty())
                    .map(|k| Favorite {
                        fingerprint: k.to_string(),
                        alias: f
                            .string(FAVORITES, k)
                            .map(|a| a.to_string())
                            .unwrap_or_default(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        Self {
            alias: custom_alias.clone().unwrap_or(d.alias),
            custom_alias,
            receive: b("Receive", d.receive),
            favorites_only: b("FavoritesOnly", d.favorites_only),
            auto_accept_favorites: b("AutoAcceptFavorites", d.auto_accept_favorites),
            favorites,
            save_folder: s("SaveFolder").map(PathBuf::from).unwrap_or(d.save_folder),
            require_approval: b("RequireApproval", d.require_approval),
            // A PIN from before RequirePIN existed was in force.
            require_pin: b("RequirePIN", !legacy_pin.is_empty()),
            pin: legacy_pin,
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
        f.set_boolean(GROUP, "FavoritesOnly", self.favorites_only);
        f.set_boolean(GROUP, "AutoAcceptFavorites", self.auto_accept_favorites);
        f.set_string(GROUP, "SaveFolder", &self.save_folder.to_string_lossy());
        f.set_boolean(GROUP, "RequireApproval", self.require_approval);
        f.set_boolean(GROUP, "RequirePIN", self.require_pin);
        // The PIN stays in the keyring; `PIN=` is only removed, by
        // `drop_legacy_pin`, once it is safely there.
        let _ = f.remove_group(FAVORITES);
        for fav in &self.favorites {
            f.set_string(FAVORITES, &fav.fingerprint, &fav.alias);
        }
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

impl Config {
    pub fn is_favorite(&self, fingerprint: &str) -> bool {
        self.favorites
            .iter()
            .any(|f| f.fingerprint.eq_ignore_ascii_case(fingerprint))
    }

    /// Whether a request from `fingerprint` is turned away before anyone
    /// is asked: receiving is off, or only favorites may send.
    pub fn declines(&self, fingerprint: &str) -> bool {
        !self.receive || (self.favorites_only && !self.is_favorite(fingerprint))
    }

    /// Whether a request from `fingerprint` waits for Accept.
    pub fn asks(&self, fingerprint: &str) -> bool {
        self.require_approval && !(self.auto_accept_favorites && self.is_favorite(fingerprint))
    }

    /// Whether `given` opens the door. With Require PIN on and no PIN to
    /// compare (the keyring could not be read), nothing does.
    pub fn pin_ok(&self, given: Option<&str>) -> bool {
        !self.require_pin || (!self.pin.is_empty() && given == Some(self.pin.as_str()))
    }
}

/// Fill in `c.pin` from the keyring. A `PIN=` in the file from an older
/// Files moves to the keyring and leaves the file.
pub fn resolve_pin(c: &mut Config) {
    match crate::secret::lookup(PIN_SECRET) {
        Ok(Some(pin)) => {
            c.pin = pin;
            if load_file().has_key(GROUP, "PIN").unwrap_or(false) {
                drop_legacy_pin();
            }
        }
        Ok(None) => {
            if !c.pin.is_empty()
                && crate::secret::store(PIN_SECRET, "LocalSend PIN", &c.pin).is_ok()
            {
                drop_legacy_pin();
            }
        }
        Err(e) => super::log(&format!("keyring: {e}")),
    }
}

/// Remove `PIN=` from the file.
pub fn drop_legacy_pin() {
    let f = load_file();
    if f.remove_key(GROUP, "PIN").is_ok() {
        let _ = f.save_to_file(path());
    }
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
            favorites_only: true,
            auto_accept_favorites: false,
            favorites: vec![Favorite {
                fingerprint: "ABCD".into(),
                alias: "Phone".into(),
            }],
            save_folder: "/tmp/in".into(),
            require_approval: false,
            require_pin: true,
            pin: String::new(),
            port: 53318,
            https: false,
            ask_for: 30,
        };
        c.to_keyfile(&f);
        assert_eq!(Config::from_keyfile(&f), c);
        assert!(f.string(GROUP, "PIN").is_err());
        f.set_string(GROUP, "Port", "0");
        assert_eq!(Config::from_keyfile(&f).port, 53317);
    }

    #[test]
    fn an_old_pin_in_the_file_is_still_required() {
        let f = glib::KeyFile::new();
        f.set_string(GROUP, "PIN", "1234");
        let c = Config::from_keyfile(&f);
        assert!(c.require_pin);
        assert_eq!(c.pin, "1234");
        assert!(c.pin_ok(Some("1234")));
        assert!(!c.pin_ok(Some("0000")));
        assert!(!c.pin_ok(None));
        // Writing keeps the PIN out of the file only by `drop_legacy_pin`.
        c.to_keyfile(&f);
        assert!(f.boolean(GROUP, "RequirePIN").unwrap());
    }

    #[test]
    fn require_pin_without_a_pin_turns_everyone_away() {
        let c = Config {
            require_pin: true,
            ..Config::default()
        };
        assert!(!c.pin_ok(Some("")));
        assert!(!c.pin_ok(None));
        assert!(Config::default().pin_ok(None));
    }

    #[test]
    fn favorites_only_and_auto_accept() {
        let mut c = Config {
            favorites: vec![Favorite {
                fingerprint: "AB12".into(),
                alias: "Phone".into(),
            }],
            ..Config::default()
        };
        assert!(!c.declines("ff00"));
        assert!(c.asks("ff00"));
        assert!(!c.asks("ab12"));
        c.favorites_only = true;
        assert!(c.declines("ff00"));
        assert!(!c.declines("AB12"));
        c.auto_accept_favorites = false;
        assert!(c.asks("AB12"));
        c.receive = false;
        assert!(c.declines("AB12"));
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
