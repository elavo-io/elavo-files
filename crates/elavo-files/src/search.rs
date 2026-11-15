//! Search by name, as Finder's search field: everything under a folder
//! (Home, or the folder shown) whose name contains what was typed, found
//! as it goes. LocalSearch (tracker) answers for Home when it is running;
//! otherwise, and always for a folder, Files walks the folders itself, a
//! batch at a time on GIO's threads, so the window never waits.
//!
//! A search is a future on the main loop: dropping or aborting it stops
//! every GIO call it has in flight.

use std::cell::Cell;
use std::collections::VecDeque;
use std::rc::Rc;

use elavo_files_core::entry;
use gtk::prelude::*;
use gtk::{gio, glib};

/// No more than this many results, as a runaway search helps nobody.
pub const LIMIT: usize = 5000;

/// Folders a person never means to search, and that are huge.
const SKIP: &[&str] = &[
    "node_modules",
    "target",
    ".venv",
    "venv",
    ".npm",
    ".cargo",
    ".rustup",
    ".local/share/flatpak",
    ".local/share/containers",
    ".git",
    ".hg",
    ".svn",
    "__pycache__",
    ".cache",
    ".Trash",
    ".Trash-1000",
    ".var",
    ".local/share/Trash",
];

const EXTRA: &str = ",unix::device,standard::is-symlink";

thread_local! {
    /// Whether LocalSearch answered once; `None` until tried.
    static TRACKER: Cell<Option<bool>> = const { Cell::new(None) };
}

/// Whether a name matches: case-insensitively, anywhere in it.
pub fn matches(name: &str, query: &str) -> bool {
    !query.is_empty() && name.to_lowercase().contains(&query.to_lowercase())
}

/// Whether to go into a folder found on the way.
pub fn descend(name: &str, rel: &str, hidden: bool, show_hidden: bool) -> bool {
    if hidden && !show_hidden {
        return false;
    }
    !SKIP.contains(&name) && !SKIP.contains(&rel)
}

/// Where a result is, for the Location column: `~/Documents/Project`.
pub fn location(info: &gio::FileInfo) -> String {
    let Some(parent) = entry::file(info).and_then(|f| f.parent()) else {
        return String::new();
    };
    let Some(path) = parent.path() else {
        return parent.uri().to_string();
    };
    let home = glib::home_dir();
    match path.strip_prefix(&home) {
        Ok(rel) if rel.as_os_str().is_empty() => "~".to_owned(),
        Ok(rel) => format!("~/{}", rel.display()),
        Err(_) => path.display().to_string(),
    }
}

pub struct Progress {
    pub found: Cell<usize>,
    pub folders: Cell<usize>,
}

/// Search `root` for `query`, adding what it finds to `store`.
pub async fn run(
    root: gio::File,
    query: String,
    show_hidden: bool,
    whole_home: bool,
    store: gio::ListStore,
    progress: Rc<Progress>,
) {
    if whole_home && TRACKER.with(|t| t.get()) != Some(false) {
        match tracker(&root, &query, show_hidden, &store, &progress).await {
            Some(()) if progress.found.get() > 0 => {
                TRACKER.with(|t| t.set(Some(true)));
                return;
            }
            // The index can be empty (or not cover this home yet).
            // A successful empty query must not hide files that exist.
            Some(()) => TRACKER.with(|t| t.set(Some(false))),
            None => TRACKER.with(|t| t.set(Some(false))),
        }
    }
    walk(root, &query, show_hidden, &store, &progress).await;
}

fn attributes() -> String {
    format!("{}{EXTRA}", entry::ATTRIBUTES)
}

async fn walk(
    root: gio::File,
    query: &str,
    show_hidden: bool,
    store: &gio::ListStore,
    progress: &Progress,
) {
    let attrs = attributes();
    let device = root
        .query_info_future(&attrs, gio::FileQueryInfoFlags::NONE, glib::Priority::LOW)
        .await
        .ok()
        .map(|i| i.attribute_uint32("unix::device"));
    let mut queue = VecDeque::from([root.clone()]);
    while let Some(dir) = queue.pop_front() {
        progress.folders.set(progress.folders.get() + 1);
        let Ok(en) = dir
            .enumerate_children_future(
                &attrs,
                gio::FileQueryInfoFlags::NOFOLLOW_SYMLINKS,
                glib::Priority::LOW,
            )
            .await
        else {
            continue;
        };
        loop {
            let Ok(infos) = en.next_files_future(200, glib::Priority::LOW).await else {
                break;
            };
            if infos.is_empty() {
                break;
            }
            let mut found = Vec::new();
            for info in infos {
                let name = info.name();
                let name = name.to_string_lossy();
                let hidden = entry::is_hidden(&info);
                if hidden && !show_hidden {
                    continue;
                }
                let child = dir.child(&*name);
                if info.file_type() == gio::FileType::Directory && !info.is_symlink() {
                    let rel = root
                        .relative_path(&child)
                        .map(|p| p.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    // Stay on one filesystem: no network or FUSE mounts.
                    let same_fs = device.is_none_or(|d| info.attribute_uint32("unix::device") == d);
                    if same_fs && descend(&name, &rel, hidden, show_hidden) {
                        queue.push_back(child.clone());
                    }
                }
                if matches(&entry::name(&info), query) {
                    info.set_attribute_object("standard::file", &child);
                    found.push(info);
                }
            }
            if !found.is_empty() {
                let room = LIMIT.saturating_sub(progress.found.get());
                found.truncate(room);
                progress.found.set(progress.found.get() + found.len());
                store.extend_from_slice(&found);
                if progress.found.get() >= LIMIT {
                    return;
                }
            }
        }
    }
}

/// Ask LocalSearch; `None` when it isn't there to ask.
async fn tracker(
    root: &gio::File,
    query: &str,
    show_hidden: bool,
    store: &gio::ListStore,
    progress: &Progress,
) -> Option<()> {
    let prefix = format!("{}/", root.uri().trim_end_matches('/'));
    let sparql = "SELECT ?url WHERE { ?f a nfo:FileDataObject ; nie:url ?url ; \
                  nfo:fileName ?name . FILTER (CONTAINS(LCASE(?name), ~q) && \
                  STRSTARTS(?url, ~root)) } LIMIT 5000";
    let q_arg = format!("q:s:{}", query.to_lowercase());
    let root_arg = format!("root:s:{prefix}");
    let argv: [&std::ffi::OsStr; 10] = [
        "tinysparql".as_ref(),
        "query".as_ref(),
        "--dbus-service".as_ref(),
        "org.freedesktop.LocalSearch3".as_ref(),
        "-a".as_ref(),
        q_arg.as_ref(),
        "-a".as_ref(),
        root_arg.as_ref(),
        "-q".as_ref(),
        sparql.as_ref(),
    ];
    let proc = gio::Subprocess::newv(
        &argv,
        gio::SubprocessFlags::STDOUT_PIPE | gio::SubprocessFlags::STDERR_SILENCE,
    )
    .ok()?;
    let (out, _) = proc.communicate_utf8_future(None).await.ok()?;
    if !proc.is_successful() {
        return None;
    }
    let attrs = attributes();
    let mut found = Vec::new();
    for line in out.unwrap_or_default().lines() {
        let uri = line.trim();
        if !uri.starts_with(&prefix) {
            continue;
        }
        let rel = &uri[prefix.len()..];
        if !show_hidden && rel.split('/').any(|p| p.starts_with('.')) {
            continue;
        }
        let file = gio::File::for_uri(uri);
        let Ok(info) = file
            .query_info_future(&attrs, gio::FileQueryInfoFlags::NONE, glib::Priority::LOW)
            .await
        else {
            continue;
        };
        info.set_attribute_object("standard::file", &file);
        found.push(info);
        if found.len() >= 100 {
            progress.found.set(progress.found.get() + found.len());
            store.extend_from_slice(&found);
            found.clear();
        }
    }
    progress.found.set(progress.found.get() + found.len());
    store.extend_from_slice(&found);
    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_names_and_skips_heavy_folders() {
        assert!(matches("Quarterly Report.PDF", "report"));
        assert!(!matches("Notes.txt", "report"));
        assert!(!matches("anything", ""));
        assert!(descend("Documents", "Documents", false, false));
        assert!(!descend("node_modules", "web/node_modules", false, false));
        assert!(!descend(".config", ".config", true, false));
        assert!(descend(".config", ".config", true, true));
        assert!(!descend(".git", ".git", true, true));
    }
}
