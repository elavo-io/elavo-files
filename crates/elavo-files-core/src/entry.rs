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

/// A folder, or an item in Recents or Trash that stands for one.
pub fn is_dir(info: &gio::FileInfo) -> bool {
    info.file_type() == gio::FileType::Directory
        || info.file_type() == gio::FileType::Mountable
        || content_type(info) == "inode/directory"
}

/// Dot files and backup files (`name~`), as Nautilus hides them.
pub fn is_hidden(info: &gio::FileInfo) -> bool {
    info.is_hidden() || info.is_backup()
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

pub fn size(info: &gio::FileInfo) -> u64 {
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
