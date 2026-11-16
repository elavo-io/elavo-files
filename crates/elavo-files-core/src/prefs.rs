//! Files Settings, kept with the rest of `~/.config/elavo/files.conf`:
//!
//! ```ini
//! [General]
//! NewWindowFolder=home    # home, recents, applications, desktop, documents or downloads
//! FoldersFirst=false      # keep folders on top when sorting by name
//! [Sidebar]
//! Recents=true
//! Home=true
//! Drives=true             # mounted drives and volumes
//! Network=true
//! Trash=true
//! LocalSend=false         # a placeholder until sharing arrives
//! [Advanced]
//! ShowAllExtensions=true
//! WarnBeforeEmptyingTrash=true
//! ConfirmBeforeDeleting=true   # Delete Immediately asks first
//! ```
//!
//! Show Hidden Files by default is `[View] ShowHidden`, which the window
//! already keeps.

use std::sync::atomic::{AtomicBool, Ordering};

use gio::glib;

const FILE: &str = "files.conf";

/// What New Window opens.
pub const NEW_WINDOW_FOLDERS: [&str; 6] = [
    "home",
    "recents",
    "applications",
    "desktop",
    "documents",
    "downloads",
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Prefs {
    pub new_window_folder: String,
    pub folders_first: bool,
    pub show_recents: bool,
    pub show_home: bool,
    pub show_drives: bool,
    pub show_network: bool,
    pub show_trash: bool,
    pub show_localsend: bool,
    pub show_extensions: bool,
    /// Bundles keep their `.eapp` even with all extensions shown, unless
    /// this is on too.
    pub show_bundle_extensions: bool,
    pub warn_empty_trash: bool,
    pub confirm_delete: bool,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            new_window_folder: "home".to_owned(),
            folders_first: false,
            show_recents: true,
            show_home: true,
            show_drives: true,
            show_network: true,
            show_trash: true,
            show_localsend: true,
            show_extensions: true,
            show_bundle_extensions: false,
            warn_empty_trash: true,
            confirm_delete: true,
        }
    }
}

pub fn path() -> std::path::PathBuf {
    glib::user_config_dir().join("elavo").join(FILE)
}

impl Prefs {
    pub fn from_keyfile(f: &glib::KeyFile) -> Self {
        let d = Self::default();
        let b = |g: &str, k: &str, def: bool| f.boolean(g, k).unwrap_or(def);
        let folder = f
            .string("General", "NewWindowFolder")
            .ok()
            .map(|s| s.to_string())
            .filter(|s| NEW_WINDOW_FOLDERS.contains(&s.as_str()))
            .unwrap_or(d.new_window_folder);
        Self {
            new_window_folder: folder,
            folders_first: b("General", "FoldersFirst", d.folders_first),
            show_recents: b("Sidebar", "Recents", d.show_recents),
            show_home: b("Sidebar", "Home", d.show_home),
            show_drives: b("Sidebar", "Drives", d.show_drives),
            show_network: b("Sidebar", "Network", d.show_network),
            show_trash: b("Sidebar", "Trash", d.show_trash),
            show_localsend: b("Sidebar", "LocalSend", d.show_localsend),
            show_extensions: b("Advanced", "ShowAllExtensions", d.show_extensions),
            show_bundle_extensions: b("Advanced", "ShowBundleExtensions", d.show_bundle_extensions),
            warn_empty_trash: b("Advanced", "WarnBeforeEmptyingTrash", d.warn_empty_trash),
            confirm_delete: b("Advanced", "ConfirmBeforeDeleting", d.confirm_delete),
        }
    }

    /// Write into `f`, leaving its other groups as they are.
    pub fn to_keyfile(&self, f: &glib::KeyFile) {
        f.set_string("General", "NewWindowFolder", &self.new_window_folder);
        f.set_boolean("General", "FoldersFirst", self.folders_first);
        f.set_boolean("Sidebar", "Recents", self.show_recents);
        f.set_boolean("Sidebar", "Home", self.show_home);
        f.set_boolean("Sidebar", "Drives", self.show_drives);
        f.set_boolean("Sidebar", "Network", self.show_network);
        f.set_boolean("Sidebar", "Trash", self.show_trash);
        f.set_boolean("Sidebar", "LocalSend", self.show_localsend);
        f.set_boolean("Advanced", "ShowAllExtensions", self.show_extensions);
        f.set_boolean(
            "Advanced",
            "ShowBundleExtensions",
            self.show_bundle_extensions,
        );
        f.set_boolean("Advanced", "WarnBeforeEmptyingTrash", self.warn_empty_trash);
        f.set_boolean("Advanced", "ConfirmBeforeDeleting", self.confirm_delete);
    }
}

fn load_file() -> glib::KeyFile {
    let f = glib::KeyFile::new();
    let _ = f.load_from_file(path(), glib::KeyFileFlags::KEEP_COMMENTS);
    f
}

pub fn read() -> Prefs {
    Prefs::from_keyfile(&load_file())
}

pub fn write(p: &Prefs) -> Result<(), glib::Error> {
    let f = load_file();
    p.to_keyfile(&f);
    let path = path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    f.save_to_file(&path)
}

static FOLDERS_FIRST: AtomicBool = AtomicBool::new(false);
static SHOW_EXTENSIONS: AtomicBool = AtomicBool::new(true);
static SHOW_BUNDLE_EXTENSIONS: AtomicBool = AtomicBool::new(false);

/// Make the settings that change how names sort and read take effect.
pub fn apply(p: &Prefs) {
    FOLDERS_FIRST.store(p.folders_first, Ordering::Relaxed);
    SHOW_EXTENSIONS.store(p.show_extensions, Ordering::Relaxed);
    SHOW_BUNDLE_EXTENSIONS.store(p.show_bundle_extensions, Ordering::Relaxed);
}

pub fn folders_first() -> bool {
    FOLDERS_FIRST.load(Ordering::Relaxed)
}

pub fn show_extensions() -> bool {
    SHOW_EXTENSIONS.load(Ordering::Relaxed)
}

/// Whether bundles show their `.eapp`: both settings have to be on.
pub fn show_bundle_extensions() -> bool {
    show_extensions() && SHOW_BUNDLE_EXTENSIONS.load(Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_when_missing() {
        let f = glib::KeyFile::new();
        assert_eq!(Prefs::from_keyfile(&f), Prefs::default());
        assert!(!Prefs::default().show_bundle_extensions);
    }

    #[test]
    fn bundle_extensions_key() {
        let f = glib::KeyFile::new();
        f.load_from_data(
            "[Advanced]\nShowBundleExtensions=true\n",
            glib::KeyFileFlags::NONE,
        )
        .unwrap();
        assert!(Prefs::from_keyfile(&f).show_bundle_extensions);
        Prefs::default().to_keyfile(&f);
        assert!(!f.boolean("Advanced", "ShowBundleExtensions").unwrap());
    }

    #[test]
    fn round_trips_and_keeps_other_groups() {
        let f = glib::KeyFile::new();
        f.load_from_data("[View]\nMode=list\n", glib::KeyFileFlags::NONE)
            .unwrap();
        let p = Prefs {
            new_window_folder: "downloads".to_owned(),
            folders_first: true,
            show_recents: false,
            show_trash: false,
            show_localsend: true,
            show_extensions: false,
            show_bundle_extensions: true,
            warn_empty_trash: false,
            confirm_delete: false,
            ..Prefs::default()
        };
        p.to_keyfile(&f);
        assert_eq!(Prefs::from_keyfile(&f), p);
        assert_eq!(f.string("View", "Mode").unwrap(), "list");
    }

    #[test]
    fn unknown_folder_falls_back_to_home() {
        let f = glib::KeyFile::new();
        f.load_from_data(
            "[General]\nNewWindowFolder=/etc\n",
            glib::KeyFileFlags::NONE,
        )
        .unwrap();
        assert_eq!(Prefs::from_keyfile(&f).new_window_folder, "home");
    }
}
