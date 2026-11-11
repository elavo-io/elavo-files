//! The Trash. With GVfs, `trash:///` lists every trash can (home and each
//! drive) and knows where items came from. Without it, as in a bare
//! toolbox, the home trash (`$XDG_DATA_HOME/Trash`, freedesktop layout:
//! `files/` and `info/*.trashinfo`) is read directly, so Put Back and
//! Empty Trash still work there.

use std::path::{Path, PathBuf};

use gio::prelude::*;

pub const URI: &str = "trash:///";

/// Whether GVfs serves `trash:///` in this session.
pub fn gvfs() -> bool {
    gio::Vfs::default()
        .supported_uri_schemes()
        .iter()
        .any(|s| s == "trash")
}

pub fn is_trash_uri(uri: &str) -> bool {
    uri.starts_with("trash:")
}

/// `$XDG_DATA_HOME/Trash`.
pub fn home() -> PathBuf {
    gio::glib::user_data_dir().join("Trash")
}

/// What `g_file_trash` keeps beside a trashed item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Info {
    pub orig: PathBuf,
    pub deleted: String,
}

pub fn parse_info(text: &str) -> Option<Info> {
    let mut in_section = false;
    let mut orig = None;
    let mut deleted = String::new();
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_section = line == "[Trash Info]";
            continue;
        }
        if !in_section {
            continue;
        }
        if let Some(v) = line.strip_prefix("Path=") {
            let path = gio::glib::Uri::unescape_string(v, None::<&str>)?;
            orig = Some(PathBuf::from(path.as_str()));
        } else if let Some(v) = line.strip_prefix("DeletionDate=") {
            deleted = v.to_owned();
        }
    }
    Some(Info {
        orig: orig?,
        deleted,
    })
}

fn info_path(trash: &Path, name: &str) -> PathBuf {
    trash.join("info").join(format!("{name}.trashinfo"))
}

/// The item's name in `trash/files` when `file` is one of them.
fn local_name(file: &gio::File, trash: &Path) -> Option<String> {
    let path = file.path()?;
    (path.parent()? == trash.join("files"))
        .then(|| path.file_name().map(|n| n.to_string_lossy().into_owned()))?
}

/// Every item in a home trash, with where it came from.
pub fn local_items(trash: &Path) -> Vec<(String, Info)> {
    let Ok(dir) = std::fs::read_dir(trash.join("info")) else {
        return Vec::new();
    };
    dir.flatten()
        .filter_map(|e| {
            let file = e.file_name().to_string_lossy().into_owned();
            let name = file.strip_suffix(".trashinfo")?.to_owned();
            let info = parse_info(&std::fs::read_to_string(e.path()).ok()?)?;
            trash.join("files").join(&name).symlink_metadata().ok()?;
            Some((name, info))
        })
        .collect()
}

/// Where a Trash item came from: GVfs's `trash::orig-path`, or the
/// `.trashinfo` beside it in a home trash.
pub fn orig_path(file: &gio::File, trash: &Path) -> Option<PathBuf> {
    if file.uri_scheme().as_deref() == Some("trash") {
        let info = file
            .query_info(
                "trash::orig-path",
                gio::FileQueryInfoFlags::NONE,
                gio::Cancellable::NONE,
            )
            .ok()?;
        return info
            .attribute_byte_string("trash::orig-path")
            .map(|p| PathBuf::from(p.as_str()));
    }
    let name = local_name(file, trash)?;
    let text = std::fs::read_to_string(info_path(trash, &name)).ok()?;
    parse_info(&text).map(|i| i.orig)
}

/// Drop the `.trashinfo` of an item that has left a home trash.
pub fn forget(file: &gio::File, trash: &Path) {
    if let Some(name) = local_name(file, trash) {
        let _ = std::fs::remove_file(info_path(trash, &name));
    }
}

/// The most recently trashed item that came from `orig`, for Undo.
pub fn find(orig: &Path, trash: &Path) -> Option<gio::File> {
    if gvfs() {
        let root = gio::File::for_uri(URI);
        let children = root
            .enumerate_children(
                "standard::name,trash::orig-path,trash::deletion-date",
                gio::FileQueryInfoFlags::NONE,
                gio::Cancellable::NONE,
            )
            .ok()?;
        let mut best: Option<(String, gio::FileInfo)> = None;
        for info in children.flatten() {
            let from = info.attribute_byte_string("trash::orig-path");
            if from.as_deref().map(Path::new) != Some(orig) {
                continue;
            }
            let date = info
                .attribute_string("trash::deletion-date")
                .map(|d| d.to_string())
                .unwrap_or_default();
            if best.as_ref().is_none_or(|(d, _)| date > *d) {
                best = Some((date, info));
            }
        }
        return best.map(|(_, info)| root.child(info.name()));
    }
    local_items(trash)
        .into_iter()
        .filter(|(_, info)| info.orig == orig)
        .max_by(|a, b| a.1.deleted.cmp(&b.1.deleted))
        .map(|(name, _)| gio::File::for_path(trash.join("files").join(name)))
}

/// Everything in the Trash, top level only.
pub fn items(trash: &Path) -> Vec<gio::File> {
    if gvfs() {
        let root = gio::File::for_uri(URI);
        let Ok(children) = root.enumerate_children(
            "standard::name",
            gio::FileQueryInfoFlags::NONE,
            gio::Cancellable::NONE,
        ) else {
            return Vec::new();
        };
        return children.flatten().map(|i| root.child(i.name())).collect();
    }
    let Ok(dir) = std::fs::read_dir(trash.join("files")) else {
        return Vec::new();
    };
    dir.flatten()
        .map(|e| gio::File::for_path(e.path()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_home_trash() {
        let trash = std::env::temp_dir().join(format!("elavo-files-trash-{}", std::process::id()));
        std::fs::create_dir_all(trash.join("files")).unwrap();
        std::fs::create_dir_all(trash.join("info")).unwrap();
        std::fs::write(trash.join("files/a b.txt"), b"x").unwrap();
        std::fs::write(
            trash.join("info/a b.txt.trashinfo"),
            "[Trash Info]\nPath=/home/me/a%20b.txt\nDeletionDate=2026-11-10T15:00:00\n",
        )
        .unwrap();
        // An info file whose item is gone is ignored.
        std::fs::write(
            trash.join("info/gone.trashinfo"),
            "[Trash Info]\nPath=/home/me/gone\nDeletionDate=2026-11-10T15:00:00\n",
        )
        .unwrap();
        let items = local_items(&trash);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].1.orig, PathBuf::from("/home/me/a b.txt"));
        let file = gio::File::for_path(trash.join("files/a b.txt"));
        assert_eq!(
            orig_path(&file, &trash),
            Some(PathBuf::from("/home/me/a b.txt"))
        );
        if !gvfs() {
            assert_eq!(
                find(Path::new("/home/me/a b.txt"), &trash).and_then(|f| f.path()),
                file.path()
            );
        }
        forget(&file, &trash);
        assert!(!trash.join("info/a b.txt.trashinfo").exists());
        std::fs::remove_dir_all(&trash).unwrap();
    }
}
