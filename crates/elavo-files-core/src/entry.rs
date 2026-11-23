//! What is asked of GIO for each file, and reading it back.

use gio::prelude::*;

use crate::format::{self, When};
use crate::sort::Fields;

/// Every attribute the views use, in one enumeration.
pub const ATTRIBUTES: &str = "standard::name,standard::display-name,standard::type,\
standard::is-hidden,standard::is-backup,standard::size,standard::icon,\
standard::symbolic-icon,standard::content-type,standard::fast-content-type,\
standard::target-uri,time::modified,thumbnail::path,thumbnail::failed,\
thumbnail::is-valid";

/// Where the Kind text is cached on a `FileInfo` once looked up.
const KIND: &str = "elavo-files::kind";

pub fn name(info: &gio::FileInfo) -> String {
    info.display_name().to_string()
}

/// The name a view shows: without its extension when Show All Filename
/// Extensions is off (folders keep theirs as they are). A bundle shows as
/// its application unless bundle extensions are shown too.
pub fn label(info: &gio::FileInfo) -> String {
    if crate::eapp::is_marked(info) {
        return title(info);
    }
    let name = name(info);
    if crate::prefs::show_extensions() || is_dir(info) {
        return name;
    }
    let (stem, _) = crate::naming::split(&name, false);
    stem.to_owned()
}

/// The name an item is called by on its own, in Get Info, Quick Look and
/// the preview: its full name, but a bundle's follows Show application
/// bundle extensions as in the views.
pub fn title(info: &gio::FileInfo) -> String {
    let shown = name(info);
    if !crate::eapp::is_marked(info) {
        return shown;
    }
    let file_name = info.name().to_string_lossy().into_owned();
    crate::eapp::shown_name(&shown, &file_name, crate::prefs::show_bundle_extensions())
}

/// A folder, or an item in Recents or Trash that stands for one.
pub fn is_dir(info: &gio::FileInfo) -> bool {
    info.file_type() == gio::FileType::Directory
        || info.file_type() == gio::FileType::Mountable
        || content_type(info) == "inode/directory"
}

/// Dot files and backup files (`name~`), as Nautilus hides them.
pub fn is_hidden(info: &gio::FileInfo) -> bool {
    // Some backends (WebDAV) leave these out.
    let has = |a: &str| info.has_attribute(a);
    (has("standard::is-hidden") && info.is_hidden())
        || (has("standard::is-backup") && info.is_backup())
}

pub fn content_type(info: &gio::FileInfo) -> String {
    info.content_type()
        .or_else(|| info.attribute_string("standard::fast-content-type"))
        .map(|s| s.to_string())
        .unwrap_or_default()
}

/// The Kind column: GIO's description of the content type, such as
/// `Folder` or `PNG image`.
pub fn kind(info: &gio::FileInfo) -> String {
    if let Some(cached) = info.attribute_string(KIND) {
        return cached.to_string();
    }
    let ct = content_type(info);
    let text = if ct.is_empty() {
        if is_dir(info) {
            "Folder".to_owned()
        } else {
            "Document".to_owned()
        }
    } else {
        gio::content_type_get_description(&ct).to_string()
    };
    info.set_attribute_string(KIND, &text);
    text
}

/// Set the Kind text ahead of time, such as `Application` for a bundle.
pub fn set_kind(info: &gio::FileInfo, text: &str) {
    info.set_attribute_string(KIND, text);
}

/// The size, or 0 when GIO doesn't know it (a WebDAV folder, say).
pub fn size(info: &gio::FileInfo) -> u64 {
    if !info.has_attribute("standard::size") {
        return 0;
    }
    info.size().max(0) as u64
}

/// Seconds since 1970, or 0 when unknown.
pub fn modified(info: &gio::FileInfo) -> i64 {
    info.modification_date_time()
        .map(|d| d.to_unix())
        .unwrap_or(0)
}

/// The Size column: `--` for folders.
pub fn size_text(info: &gio::FileInfo) -> String {
    if is_dir(info) {
        format::NO_SIZE.to_owned()
    } else {
        format::size(size(info))
    }
}

/// The Date Modified column, in local time.
pub fn modified_text(info: &gio::FileInfo) -> String {
    match info.modification_date_time() {
        Some(d) => date_time_text(&d),
        None => format::NO_SIZE.to_owned(),
    }
}

/// A Unix time the way the Date Modified column reads it.
pub fn date_text(secs: i64) -> String {
    match gio::glib::DateTime::from_unix_utc(secs) {
        Ok(d) if secs > 0 => date_time_text(&d),
        _ => format::NO_SIZE.to_owned(),
    }
}

fn date_time_text(d: &gio::glib::DateTime) -> String {
    let (Ok(local), Ok(now)) = (d.to_local(), gio::glib::DateTime::now_local()) else {
        return format::NO_SIZE.to_owned();
    };
    let ymd = |d: &gio::glib::DateTime| (d.year(), d.month() as u32, d.day_of_month() as u32);
    let yesterday = now.add_days(-1).map(|d| ymd(&d)).unwrap_or((0, 0, 0));
    let when = When {
        year: local.year(),
        month: local.month() as u32,
        day: local.day_of_month() as u32,
        hour: local.hour() as u32,
        minute: local.minute() as u32,
    };
    format::date(&when, ymd(&now), yesterday)
}

/// The file a row stands for. `gtk::DirectoryList` stores it under
/// `standard::file`. Items in Recents and Trash point elsewhere through
/// `standard::target-uri`, which is what opening them should use.
pub fn file(info: &gio::FileInfo) -> Option<gio::File> {
    info.attribute_object("standard::file")
        .and_then(|o| o.downcast::<gio::File>().ok())
}

pub fn target(info: &gio::FileInfo) -> Option<gio::File> {
    if let Some(uri) = info.attribute_string("standard::target-uri") {
        return Some(gio::File::for_uri(&uri));
    }
    file(info)
}

/// A cached freedesktop thumbnail that is still valid, if GIO knows one.
pub fn thumbnail(info: &gio::FileInfo) -> Option<std::path::PathBuf> {
    if info.boolean("thumbnail::failed") {
        return None;
    }
    if info.has_attribute("thumbnail::is-valid") && !info.boolean("thumbnail::is-valid") {
        return None;
    }
    let path = info.attribute_byte_string("thumbnail::path")?;
    let path = std::path::PathBuf::from(path.as_str());
    path.exists().then_some(path)
}

/// The values the sorters compare. Borrowed strings live in `scratch`.
pub fn fields<'a>(info: &gio::FileInfo, scratch: &'a mut (String, String)) -> Fields<'a> {
    scratch.0 = name(info);
    scratch.1 = kind(info);
    Fields {
        name: &scratch.0,
        is_dir: is_dir(info),
        size: size(info),
        modified: modified(info),
        kind: &scratch.1,
    }
}

/// Whether a later query of the same file should replace the listed
/// attributes: the size, time or type changed after the file first
/// appeared (a screenshot still being written, for example).
pub fn stale(listed: &gio::FileInfo, fresh: &gio::FileInfo) -> bool {
    size(listed) != size(fresh)
        || modified(listed) != modified(fresh)
        || content_type(listed) != content_type(fresh)
}

/// Whether this directory-monitor event means the file's contents may
/// have changed after it was first listed.
///
/// `gtk::DirectoryList` already handles created, deleted, renamed and
/// `AttributeChanged`. It ignores `Changed` and `ChangesDoneHint`,
/// which is how a write to an existing name is reported.
pub fn content_event(event: gio::FileMonitorEvent) -> bool {
    matches!(
        event,
        gio::FileMonitorEvent::Changed | gio::FileMonitorEvent::ChangesDoneHint
    )
}

/// Copy a fresh query onto a listed `FileInfo`, keeping `standard::file`
/// (what `gtk::DirectoryList` stores the file under). Cached Kind text
/// and other `elavo-files::` values are dropped, so the views read the
/// new size, type and thumbnail.
pub fn refresh_from(dest: &gio::FileInfo, fresh: &gio::FileInfo) {
    let file = dest.attribute_object("standard::file");
    for ns in ["thumbnail", "elavo-files"] {
        for attr in dest.list_attributes(Some(ns)) {
            dest.remove_attribute(&attr);
        }
    }
    fresh.copy_into(dest);
    if let Some(file) = file {
        dest.set_attribute_object("standard::file", &file);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bundle_shows_as_its_application() {
        let dir = std::env::temp_dir().join(format!("elavo-files-label-{}", std::process::id()));
        let b = dir.join("notes-app.eapp");
        std::fs::create_dir_all(&b).unwrap();
        std::fs::write(b.join("entry.json"), r#"{"name": "Notes"}"#).unwrap();
        let info = gio::File::for_path(&b)
            .query_info(
                ATTRIBUTES,
                gio::FileQueryInfoFlags::NONE,
                gio::Cancellable::NONE,
            )
            .unwrap();
        info.set_attribute_object("standard::file", &gio::File::for_path(&b));
        assert!(crate::eapp::decorate(&info, &|_| false));
        // Bundle extensions start hidden, whatever Show all filename
        // extensions says.
        assert_eq!(label(&info), "Notes");
        assert_eq!(title(&info), "Notes");
        assert_eq!(info.name().to_str(), Some("notes-app.eapp"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reads_a_real_folder() {
        let dir = std::env::temp_dir().join(format!("elavo-files-test-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("inner")).unwrap();
        std::fs::write(dir.join("a.txt"), b"hello").unwrap();
        std::fs::write(dir.join(".hidden"), b"").unwrap();
        let folder = gio::File::for_path(&dir);
        let mut seen = Vec::new();
        let children = folder
            .enumerate_children(
                ATTRIBUTES,
                gio::FileQueryInfoFlags::NONE,
                gio::Cancellable::NONE,
            )
            .unwrap();
        while let Some(info) = children.next_file(gio::Cancellable::NONE).unwrap() {
            seen.push((
                name(&info),
                is_dir(&info),
                is_hidden(&info),
                size_text(&info),
            ));
        }
        seen.sort();
        assert_eq!(
            seen,
            vec![
                (".hidden".into(), false, true, "Zero bytes".into()),
                ("a.txt".into(), false, false, "5 bytes".into()),
                ("inner".into(), true, false, "--".into()),
            ]
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn content_writes_are_the_events_directory_list_skips() {
        use gio::FileMonitorEvent as Event;
        assert!(content_event(Event::Changed));
        assert!(content_event(Event::ChangesDoneHint));
        assert!(!content_event(Event::Created));
        assert!(!content_event(Event::Deleted));
        assert!(!content_event(Event::AttributeChanged));
        assert!(!content_event(Event::MovedIn));
        assert!(!content_event(Event::Renamed));
    }

    #[test]
    fn refresh_from_updates_size_and_type_and_clears_caches() {
        let dir = std::env::temp_dir().join(format!("elavo-files-refresh-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("shot.png");
        std::fs::write(&path, b"").unwrap();
        let file = gio::File::for_path(&path);
        let listed = file
            .query_info(
                ATTRIBUTES,
                gio::FileQueryInfoFlags::NONE,
                gio::Cancellable::NONE,
            )
            .unwrap();
        listed.set_attribute_object("standard::file", &file);
        listed.set_attribute_string("elavo-files::kind", "Document");
        listed.set_attribute_boolean("thumbnail::failed", true);
        assert_eq!(size(&listed), 0);
        assert_eq!(kind(&listed), "Document");

        // A 1-by-1 PNG, so the type can become an image once it has bytes.
        let png: &[u8] = &[
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48,
            0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00,
            0x00, 0x1F, 0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0A, 0x49, 0x44, 0x41, 0x54, 0x78,
            0x9C, 0x63, 0x00, 0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00,
            0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
        ];
        std::fs::write(&path, png).unwrap();
        let fresh = file
            .query_info(
                ATTRIBUTES,
                gio::FileQueryInfoFlags::NONE,
                gio::Cancellable::NONE,
            )
            .unwrap();
        assert!(stale(&listed, &fresh));
        refresh_from(&listed, &fresh);
        assert_eq!(size(&listed), png.len() as u64);
        assert!(!listed.boolean("thumbnail::failed"));
        assert!(listed.attribute_string("elavo-files::kind").is_none());
        let kept = listed
            .attribute_object("standard::file")
            .and_then(|o| o.downcast::<gio::File>().ok())
            .expect("standard::file is kept");
        assert!(kept.equal(&file));
        let kind = kind(&listed);
        assert_ne!(kind, "Document");
        let ct = content_type(&listed);
        assert!(
            ct.starts_with("image/")
                || kind.to_lowercase().contains("png")
                || kind.to_lowercase().contains("image"),
            "content type {ct}, kind {kind}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
