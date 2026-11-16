//! Elavo application bundles: `Name.eapp` folders made by
//! elavo-applications, each with an `entry.json` and a `launch.desktop`.
//!
//! Files shows a bundle as the application, not as a folder: its own name
//! (no `.eapp`) and icon, and opening it starts the application. With both
//! Show all filename extensions and Show application bundle extensions on,
//! it shows its file name, `.eapp` and all, instead. The rules follow
//! elavo-applications' Nautilus module:
//!
//! - The shared Applications folder is `/Applications` (or
//!   `$ELAVO_SHARED_APPLICATIONS`); each person also has `~/Applications`.
//! - A bundle in one of those is an install and starts from its
//!   `launch.desktop`. A copy anywhere else is opened with
//!   `elavo-applications-ctl open`, which puts a missing Flatpak back.
//! - The icon is the person's `custom-icon.*`, then the `Icon=` that the
//!   service wrote to `launch.desktop` (a theme name or a file; a file in
//!   another bundle means the one this was copied from, so the same file
//!   here is used), then `standard-icon.png`, then the app's own icon.
//! - An install is never moved or deleted by Files. Moving one to the Trash
//!   or deleting it asks the service (`elavo-applications-ctl remove`), and
//!   is refused while it runs.

use std::path::{Path, PathBuf};

use gio::glib;
use gio::prelude::*;

pub const SUFFIX: &str = ".eapp";
pub const FALLBACK_ICON: &str = "application-x-executable";
const CUSTOM: [&str; 3] = ["custom-icon.png", "custom-icon.svg", "custom-icon.webp"];
const BUNDLED: [&str; 3] = ["standard-icon.png", "icon.png", "icon.svg"];
/// Set on a `FileInfo` once looked at: "1" for a bundle, "0" otherwise.
const MARK: &str = "elavo-files::eapp";
/// "1" on a macOS application bundle, which Files shows as one item, as
/// Finder does, though it can't start it.
const PACKAGE: &str = "elavo-files::package";
pub const MAC_SUFFIX: &str = ".app";
pub const MAC_KIND: &str = "macOS Application";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Icon {
    File(PathBuf),
    Name(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bundle {
    pub path: PathBuf,
    pub name: String,
    pub icon: Icon,
}

impl Bundle {
    pub fn desktop(&self) -> PathBuf {
        self.path.join("launch.desktop")
    }
}

/// The program elavo-applications installs, or `$ELAVO_APPLICATIONS_CTL`.
pub fn ctl() -> String {
    std::env::var("ELAVO_APPLICATIONS_CTL")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "elavo-applications-ctl".to_owned())
}

/// The shared Applications folder, as given (not resolved): `/Applications`
/// unless `$ELAVO_SHARED_APPLICATIONS` says otherwise.
pub fn shared_dir() -> PathBuf {
    std::env::var_os("ELAVO_SHARED_APPLICATIONS")
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/Applications"))
}

pub fn user_dir() -> PathBuf {
    glib::home_dir().join("Applications")
}

fn real(path: &Path) -> Option<PathBuf> {
    std::fs::canonicalize(path).ok()
}

/// A folder named `*.eapp` with an `entry.json`.
pub fn is_bundle(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.ends_with(SUFFIX) && n.len() > SUFFIX.len())
        && path.join("entry.json").is_file()
}

/// A macOS application: a folder named `*.app` with `Contents/Info.plist`.
pub fn is_mac_app(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.ends_with(MAC_SUFFIX) && n.len() > MAC_SUFFIX.len())
        && path.join("Contents").join("Info.plist").is_file()
}

/// Inside `~/Applications` or the shared folder (or one of them).
pub fn in_applications(path: &Path) -> bool {
    let Some(path) = real(path) else {
        return false;
    };
    [user_dir(), shared_dir()]
        .iter()
        .filter_map(|d| real(d))
        .any(|d| path.starts_with(d))
}

/// A bundle directly in the shared folder: an install for everyone, which
/// GIO cannot trash (it is on /var) and only the service removes.
pub fn in_shared(path: &Path) -> bool {
    let (Some(path), Some(shared)) = (real(path), real(&shared_dir())) else {
        return false;
    };
    is_bundle(&path) && path.parent() == Some(shared.as_path())
}

/// An install: a bundle inside one of the Applications folders.
pub fn is_install(path: &Path) -> bool {
    is_bundle(path) && in_applications(path)
}

/// What `entry.json` says about a bundle, for Get Info.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Details {
    pub id: String,
    /// `flatpak`, `appimage` or `system`.
    pub kind: String,
    pub origin: String,
    /// The file it was installed from, such as an AppImage.
    pub source: String,
}

pub fn details(path: &Path) -> Option<Details> {
    let text = std::fs::read_to_string(path.join("entry.json")).ok()?;
    let get = |k: &str| json_field(&text, k).unwrap_or_default();
    Some(Details {
        id: get("id"),
        kind: get("kind"),
        origin: get("origin"),
        source: get("source"),
    })
}

/// How Get Info names the kind of install.
pub fn kind_label(kind: &str) -> &str {
    match kind {
        "flatpak" => "Flatpak",
        "appimage" => "AppImage",
        "system" => "Part of Elavo",
        "" => "Application",
        other => other,
    }
}

/// A version in a file name: `1.98.60` in
/// `Brave_Origin-1.98.60-1-anylinux-x86_64.AppImage`.
pub fn version_in(name: &str) -> Option<String> {
    name.split(['-', '_', ' '])
        .map(|part| {
            let part = part.trim_start_matches(['v', 'V']);
            let end = part
                .find(|c: char| !(c.is_ascii_digit() || c == '.'))
                .unwrap_or(part.len());
            part[..end].trim_end_matches('.')
        })
        .find(|v| v.contains('.') && v.split('.').all(|p| !p.is_empty()))
        .map(str::to_owned)
}

/// The value of `"key": "..."` in a flat JSON object, enough for
/// `entry.json`.
pub fn json_field(text: &str, key: &str) -> Option<String> {
    let at = text.find(&format!("\"{key}\""))? + key.len() + 2;
    let rest = text[at..].trim_start().strip_prefix(':')?.trim_start();
    let mut chars = rest.strip_prefix('"')?.chars();
    let mut out = String::new();
    while let Some(c) = chars.next() {
        match c {
            '"' => return Some(out),
            '\\' => match chars.next()? {
                'n' => out.push('\n'),
                't' => out.push('\t'),
                'u' => {
                    let hex: String = chars.by_ref().take(4).collect();
                    out.push(char::from_u32(u32::from_str_radix(&hex, 16).ok()?)?);
                }
                other => out.push(other),
            },
            c => out.push(c),
        }
    }
    None
}

fn desktop_value(bundle: &Path, key: &str) -> Option<String> {
    let file = glib::KeyFile::new();
    file.load_from_file(bundle.join("launch.desktop"), glib::KeyFileFlags::NONE)
        .ok()?;
    let value = if key == "Name" {
        file.locale_string("Desktop Entry", key, None)
    } else {
        file.string("Desktop Entry", key)
    };
    value.ok().map(|v| v.to_string()).filter(|v| !v.is_empty())
}

/// The name without `.eapp`.
pub fn plain_name(path: &Path) -> String {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    name.strip_suffix(SUFFIX).unwrap_or(&name).to_owned()
}

/// What the bundle is called: the entry's name, then the desktop file's,
/// then the folder name without `.eapp`.
pub fn name(path: &Path) -> String {
    std::fs::read_to_string(path.join("entry.json"))
        .ok()
        .and_then(|t| json_field(&t, "name"))
        .filter(|n| !n.trim().is_empty())
        .or_else(|| desktop_value(path, "Name"))
        .unwrap_or_else(|| plain_name(path))
}

/// `own` is the `Icon=` of `launch.desktop`. A theme name counts only if
/// `themed` says the icon theme has it; otherwise the bundle's own icon file
/// is used, as the name may come from a Flatpak the theme can't see.
pub fn icon(bundle: &Path, own: Option<&str>, themed: &dyn Fn(&str) -> bool) -> Icon {
    if let Some(p) = CUSTOM.iter().map(|n| bundle.join(n)).find(|p| p.is_file()) {
        return Icon::File(p);
    }
    match own.map(str::trim).filter(|s| !s.is_empty()) {
        Some(own) if own.starts_with('/') => {
            let own = Path::new(own);
            // A file in another bundle: the original this was copied from,
            // which may be gone. Use the same file in this one.
            let from_bundle = own
                .ancestors()
                .find(|a| a.extension().is_some_and(|e| e == "eapp"))
                .and_then(|b| own.strip_prefix(b).ok())
                .map(|rel| bundle.join(rel))
                .filter(|p| p.is_file());
            if let Some(p) = from_bundle {
                return Icon::File(p);
            }
            if own.is_file() {
                return Icon::File(own.to_owned());
            }
        }
        Some(own) if !own.contains('/') && themed(own) => return Icon::Name(own.to_owned()),
        _ => {}
    }
    if let Some(p) = BUNDLED.iter().map(|n| bundle.join(n)).find(|p| p.is_file()) {
        return Icon::File(p);
    }
    Icon::Name(FALLBACK_ICON.to_owned())
}

pub fn read(path: &Path, themed: &dyn Fn(&str) -> bool) -> Option<Bundle> {
    if !is_bundle(path) {
        return None;
    }
    Some(Bundle {
        path: path.to_owned(),
        name: name(path),
        icon: icon(path, desktop_value(path, "Icon").as_deref(), themed),
    })
}

pub fn gicon(icon: &Icon) -> gio::Icon {
    match icon {
        Icon::File(p) => gio::FileIcon::new(&gio::File::for_path(p)).upcast(),
        // One name only: GTK tries every name in a theme before the next
        // theme, so a fallback would win over an app icon in hicolor.
        Icon::Name(n) => gio::ThemedIcon::new(n).upcast(),
    }
}

/// Turn a listed bundle into the application: its name, icon and kind.
/// Cheap after the first call. True for a bundle.
pub fn decorate(info: &gio::FileInfo, themed: &dyn Fn(&str) -> bool) -> bool {
    if let Some(mark) = info.attribute_string(MARK) {
        return mark == "1";
    }
    let is_dir = info.file_type() == gio::FileType::Directory;
    let path = || crate::entry::file(info).and_then(|f| f.path());
    let name = info.name();
    let name = name.to_str().unwrap_or_default();
    let bundle = (is_dir && name.ends_with(SUFFIX))
        .then(path)
        .flatten()
        .and_then(|p| read(&p, themed));
    let Some(bundle) = bundle else {
        // A macOS app keeps its name (renaming it must keep the .app) but
        // shows as an application.
        if is_dir && name.ends_with(MAC_SUFFIX) && path().is_some_and(|p| is_mac_app(&p)) {
            info.set_icon(&gio::ThemedIcon::new(FALLBACK_ICON).upcast::<gio::Icon>());
            crate::entry::set_kind(info, MAC_KIND);
            info.set_attribute_string(PACKAGE, "1");
        }
        info.set_attribute_string(MARK, "0");
        return false;
    };
    info.set_display_name(&bundle.name);
    info.set_icon(&gicon(&bundle.icon));
    crate::entry::set_kind(info, "Application");
    info.set_attribute_string(MARK, "1");
    true
}

/// Whether `decorate` found a bundle.
pub fn is_marked(info: &gio::FileInfo) -> bool {
    info.attribute_string(MARK).is_some_and(|m| m == "1")
}

/// A folder shown as one item: an Elavo bundle or a macOS application.
/// Columns previews it instead of opening it as a folder.
pub fn is_package(info: &gio::FileInfo) -> bool {
    is_marked(info) || info.attribute_string(PACKAGE).is_some_and(|m| m == "1")
}

/// What a bundle is shown as: the application's name, or its file name,
/// `.eapp` included, when bundle extensions are shown.
pub fn shown_name(app: &str, file_name: &str, with_suffix: bool) -> String {
    if with_suffix {
        file_name.to_owned()
    } else {
        app.to_owned()
    }
}

/// How the bundle at `path` is shown, following Files Settings; `None` for
/// anything that isn't a bundle.
pub fn shown_path_name(path: &Path) -> Option<String> {
    if !is_bundle(path) {
        return None;
    }
    let file_name = path.file_name()?.to_string_lossy().into_owned();
    let with_suffix = crate::prefs::show_bundle_extensions();
    let app = if with_suffix {
        String::new()
    } else {
        name(path)
    };
    Some(shown_name(&app, &file_name, with_suffix))
}

/// The text a bundle's rename field starts with: its file name, less the
/// `.eapp` when that is hidden.
pub fn rename_text(file_name: &str, with_suffix: bool) -> String {
    if with_suffix {
        file_name.to_owned()
    } else {
        file_name
            .strip_suffix(SUFFIX)
            .unwrap_or(file_name)
            .to_owned()
    }
}

/// How much of a bundle's rename field is selected at first: the name,
/// not the `.eapp`.
pub fn rename_stem_chars(text: &str) -> usize {
    text.strip_suffix(SUFFIX).unwrap_or(text).chars().count()
}

/// The file name a bundle's rename field asks for. The `.eapp` stays on
/// whether or not it was shown, so a rename can't turn the application
/// back into a folder.
pub fn renamed(text: &str) -> String {
    if text.ends_with(SUFFIX) && text.len() > SUFFIX.len() {
        text.to_owned()
    } else {
        format!("{text}{SUFFIX}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundle_names() {
        assert_eq!(shown_name("Notes", "notes-app.eapp", false), "Notes");
        assert_eq!(
            shown_name("Notes", "notes-app.eapp", true),
            "notes-app.eapp"
        );
        assert_eq!(rename_text("Notes.eapp", false), "Notes");
        assert_eq!(rename_text("Notes.eapp", true), "Notes.eapp");
        assert_eq!(rename_stem_chars("Notes.eapp"), 5);
        assert_eq!(rename_stem_chars("Notes"), 5);
        assert_eq!(rename_stem_chars("Né.v2.eapp"), 5);
    }

    #[test]
    fn renaming_keeps_the_suffix() {
        // Hidden: the field has no .eapp; it goes back on.
        assert_eq!(renamed("Jot"), "Jot.eapp");
        // Shown: kept as typed, or put back if it was deleted.
        assert_eq!(renamed("Jot.eapp"), "Jot.eapp");
        assert_eq!(renamed("Jot.eap"), "Jot.eap.eapp");
        assert_eq!(renamed("Jot.v2"), "Jot.v2.eapp");
        assert_eq!(renamed(".eapp"), ".eapp.eapp");
        // An unchanged field gives the old file name back, so no rename.
        for shown in [false, true] {
            assert_eq!(renamed(&rename_text("Notes.eapp", shown)), "Notes.eapp");
        }
    }

    #[test]
    fn shown_path_names() {
        let d = scratch("shown");
        let b = bundle(&d, "notes-app.eapp", r#"{"name": "Notes"}"#, "");
        let plain = d.join("Folder.eapp");
        std::fs::create_dir_all(&plain).unwrap();
        // Settings start with bundle extensions hidden.
        assert_eq!(shown_path_name(&b).as_deref(), Some("Notes"));
        assert_eq!(shown_path_name(&plain), None);
        assert_eq!(shown_path_name(&d), None);
        let _ = std::fs::remove_dir_all(&d);
    }

    fn bundle(dir: &Path, name: &str, json: &str, desktop: &str) -> PathBuf {
        let b = dir.join(name);
        std::fs::create_dir_all(&b).unwrap();
        std::fs::write(b.join("entry.json"), json).unwrap();
        std::fs::write(b.join("launch.desktop"), desktop).unwrap();
        b
    }

    fn scratch(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("elavo-eapp-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn packages() {
        let d = scratch("packages");
        let b = bundle(&d, "Notes.eapp", r#"{"name": "Notes"}"#, "");
        let mac = d.join("Safari.app");
        std::fs::create_dir_all(mac.join("Contents")).unwrap();
        std::fs::write(mac.join("Contents/Info.plist"), b"plist").unwrap();
        let plain = d.join("Photos.app");
        std::fs::create_dir_all(&plain).unwrap();
        assert!(is_mac_app(&mac) && !is_mac_app(&plain) && !is_mac_app(&b));

        let info = |p: &Path| {
            let file = gio::File::for_path(p);
            let info = file
                .query_info(
                    "standard::*",
                    gio::FileQueryInfoFlags::NONE,
                    gio::Cancellable::NONE,
                )
                .unwrap();
            info.set_attribute_object("standard::file", &file);
            decorate(&info, &|_| false);
            info
        };
        let (b, mac, plain) = (info(&b), info(&mac), info(&plain));
        assert!(is_marked(&b) && is_package(&b));
        assert_eq!(b.display_name(), "Notes");
        assert!(!is_marked(&mac) && is_package(&mac));
        assert_eq!(mac.display_name(), "Safari.app");
        assert!(!is_package(&plain));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn json_strings() {
        let t = r#"{ "kind": "flatpak", "name" : "Caf\u00e9 \"Q\"", "x": 1 }"#;
        assert_eq!(json_field(t, "name").as_deref(), Some("Caf\u{e9} \"Q\""));
        assert_eq!(json_field(t, "kind").as_deref(), Some("flatpak"));
        assert_eq!(json_field(t, "missing"), None);
    }

    #[test]
    fn names_and_icons() {
        let d = scratch("names");
        let b = bundle(
            &d,
            "GIMP.eapp",
            r#"{"name": "GNU Image Manipulation Program"}"#,
            "[Desktop Entry]\nName=GIMP\nIcon=org.gimp.GIMP\nExec=true\n",
        );
        let read = read(&b, &|_| true).unwrap();
        assert_eq!(read.name, "GNU Image Manipulation Program");
        assert_eq!(read.icon, Icon::Name("org.gimp.GIMP".into()));

        // A copy points at the original's icon file; use its own.
        let c = bundle(
            &d,
            "Copy.eapp",
            "{}",
            "[Desktop Entry]\nName=Copied\nIcon=/Applications/Gone.eapp/standard-icon.png\n",
        );
        std::fs::write(c.join("standard-icon.png"), b"png").unwrap();
        let read = super::read(&c, &|_| true).unwrap();
        assert_eq!(read.name, "Copied");
        assert_eq!(read.icon, Icon::File(c.join("standard-icon.png")));

        // The person's own icon wins.
        std::fs::write(c.join("custom-icon.svg"), b"svg").unwrap();
        assert_eq!(
            icon(&c, Some("x"), &|_| true),
            Icon::File(c.join("custom-icon.svg"))
        );

        // Not a bundle without entry.json; the plain name otherwise.
        std::fs::create_dir_all(d.join("Plain.eapp")).unwrap();
        assert!(super::read(&d.join("Plain.eapp"), &|_| true).is_none());
        let e = bundle(&d, "Empty Name.eapp", r#"{"name": ""}"#, "");
        assert_eq!(name(&e), "Empty Name");
        assert_eq!(icon(&e, None, &|_| true), Icon::Name(FALLBACK_ICON.into()));
        // A theme name the theme lacks gives way to the bundle's file.
        std::fs::write(e.join("icon.png"), b"").unwrap();
        assert_eq!(
            icon(&e, Some("org.x.Y"), &|_| false),
            Icon::File(e.join("icon.png"))
        );
        assert_eq!(
            icon(&e, Some("org.x.Y"), &|_| true),
            Icon::Name("org.x.Y".into())
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn reads_details_and_versions() {
        let d = scratch("details");
        let b = bundle(
            &d,
            "Brave Origin.eapp",
            r#"{"kind": "appimage", "id": "brave-origin", "name": "Brave Origin", "origin": "", "source": "Brave_Origin-1.98.60-1-anylinux-x86_64.AppImage"}"#,
            "",
        );
        let info = details(&b).unwrap();
        assert_eq!(info.id, "brave-origin");
        assert_eq!(kind_label(&info.kind), "AppImage");
        assert_eq!(version_in(&info.source).as_deref(), Some("1.98.60"));
        assert_eq!(
            version_in("Cursor-3.24.12-x86_64.AppImage").as_deref(),
            Some("3.24.12")
        );
        assert_eq!(version_in("tool-v2.1.AppImage").as_deref(), Some("2.1"));
        assert_eq!(version_in("x86_64.AppImage"), None);
        assert_eq!(version_in("plain"), None);
    }
}
