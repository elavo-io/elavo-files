//! The person's Favorites in the sidebar, in their order.
//!
//! Kept in `~/.config/elavo/favorites`, in the GTK bookmarks format (one
//! `uri [name]` per line), so it reads like `~/.config/gtk-3.0/bookmarks`.
//! That file belongs to Nautilus and the GTK file chooser and is never
//! written. On the first run the folders bookmarked there are added after
//! Elavo's defaults; the standard folders (Pictures, Music...) are left
//! out, as Elavo's defaults decide which of those show.

use std::path::PathBuf;

use gio::glib;
use gio::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Favorite {
    pub uri: String,
    /// The bookmark's own name, if it has one; otherwise the folder's.
    pub name: Option<String>,
}

impl Favorite {
    pub fn new(uri: &str) -> Self {
        Self {
            uri: uri.to_owned(),
            name: None,
        }
    }
}

pub fn same_uri(a: &str, b: &str) -> bool {
    a.trim_end_matches('/') == b.trim_end_matches('/')
}

/// Applications, Desktop, Documents, Downloads.
pub fn defaults() -> Vec<Favorite> {
    crate::places::favorites()
        .into_iter()
        .filter(|p| p.shown)
        .map(|p| Favorite::new(&p.uri))
        .collect()
}

pub fn path() -> PathBuf {
    glib::user_config_dir().join("elavo").join("favorites")
}

pub fn gtk_bookmarks_path() -> PathBuf {
    glib::user_config_dir().join("gtk-3.0").join("bookmarks")
}

/// One `uri [name]` per line; blank lines, `#` comments and repeats are
/// skipped.
pub fn parse(text: &str) -> Vec<Favorite> {
    let mut out: Vec<Favorite> = Vec::new();
    for line in text.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (uri, name) = match line.split_once(' ') {
            Some((u, n)) => (u, Some(n.trim().to_owned()).filter(|n| !n.is_empty())),
            None => (line, None),
        };
        if !uri.contains(":/") || out.iter().any(|f| same_uri(&f.uri, uri)) {
            continue;
        }
        out.push(Favorite {
            uri: uri.to_owned(),
            name,
        });
    }
    out
}

pub fn serialize(list: &[Favorite]) -> String {
    let mut out = String::from("# Elavo Files: the Favorites in the sidebar, in order.\n");
    for f in list {
        out.push_str(&f.uri);
        if let Some(n) = &f.name {
            out.push(' ');
            out.push_str(n);
        }
        out.push('\n');
    }
    out
}

/// The standard folders: home and the XDG ones.
fn standard(uri: &str) -> bool {
    use glib::UserDirectory as D;
    let Some(path) = gio::File::for_uri(uri).path() else {
        return false;
    };
    path == glib::home_dir()
        || [
            D::Desktop,
            D::Documents,
            D::Downloads,
            D::Music,
            D::Pictures,
            D::PublicShare,
            D::Templates,
            D::Videos,
        ]
        .into_iter()
        .filter_map(glib::user_special_dir)
        .any(|d| d == path)
}

/// The bookmarks worth bringing over from GTK's file: local folders that
/// are not standard ones.
pub fn imports(gtk_bookmarks: &str) -> Vec<Favorite> {
    parse(gtk_bookmarks)
        .into_iter()
        .filter(|f| f.uri.starts_with("file:///") && !standard(&f.uri))
        .collect()
}

/// The saved list, or None when there is none (yet).
pub fn read() -> Option<Vec<Favorite>> {
    std::fs::read_to_string(path()).ok().map(|t| parse(&t))
}

/// The list; on the first run, the defaults and GTK's bookmarks, saved.
pub fn load() -> Vec<Favorite> {
    if let Some(list) = read() {
        return list;
    }
    let mut list = defaults();
    let gtk = std::fs::read_to_string(gtk_bookmarks_path()).unwrap_or_default();
    for f in imports(&gtk) {
        if !list.iter().any(|d| same_uri(&d.uri, &f.uri)) {
            list.push(f);
        }
    }
    let _ = save(&list);
    list
}

/// Write the list whole, by renaming over the old one, so a reader never
/// sees half of it.
pub fn save(list: &[Favorite]) -> std::io::Result<()> {
    let path = path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, serialize(list))?;
    std::fs::rename(&tmp, &path)
}

/// Put `fav` at `at`, moving it there if it is in the list already.
pub fn insert(list: &mut Vec<Favorite>, fav: Favorite, at: usize) {
    let mut at = at.min(list.len());
    if let Some(i) = list.iter().position(|f| same_uri(&f.uri, &fav.uri)) {
        let old = list.remove(i);
        if i < at {
            at -= 1;
        }
        list.insert(
            at,
            Favorite {
                name: fav.name.or(old.name),
                ..old
            },
        );
        return;
    }
    list.insert(at, fav);
}

pub fn remove(list: &mut Vec<Favorite>, uri: &str) -> bool {
    let before = list.len();
    list.retain(|f| !same_uri(&f.uri, uri));
    list.len() != before
}

/// The defaults back in their places, first, then the folders the person
/// added, in their order.
pub fn restore(list: &[Favorite]) -> Vec<Favorite> {
    let defaults = defaults();
    let mut out = defaults.clone();
    out.extend(
        list.iter()
            .filter(|f| !defaults.iter().any(|d| same_uri(&d.uri, &f.uri)))
            .cloned(),
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uris(list: &[Favorite]) -> Vec<&str> {
        list.iter().map(|f| f.uri.as_str()).collect()
    }

    #[test]
    fn parses_gtk_bookmarks() {
        let list = parse(
            "# note\nfile:///Applications Applications\n\nfile:///tmp/a%20b\n\
             file:///tmp/a%20b/ Again\nsftp://host/x Server\nnot-a-uri\n",
        );
        assert_eq!(
            uris(&list),
            ["file:///Applications", "file:///tmp/a%20b", "sftp://host/x"]
        );
        assert_eq!(list[0].name.as_deref(), Some("Applications"));
        assert_eq!(list[1].name, None);
        assert_eq!(parse(&serialize(&list)), list);
    }

    #[test]
    fn imports_only_other_local_folders() {
        let home = gio::File::for_path(glib::home_dir()).uri();
        let text = format!("{home}\nfile:///srv/projects Projects\nsmb://nas/share\n");
        let list = imports(&text);
        assert_eq!(uris(&list), ["file:///srv/projects"]);
        if let Some(pics) = glib::user_special_dir(glib::UserDirectory::Pictures) {
            if pics != glib::home_dir() {
                let uri = gio::File::for_path(pics).uri();
                assert!(imports(&format!("{uri} Pictures\n")).is_empty());
            }
        }
    }

    #[test]
    fn inserts_move_and_remove() {
        let mut list: Vec<Favorite> = ["file:///a", "file:///b", "file:///c"]
            .map(Favorite::new)
            .to_vec();
        insert(&mut list, Favorite::new("file:///d"), 1);
        assert_eq!(
            uris(&list),
            ["file:///a", "file:///d", "file:///b", "file:///c"]
        );
        // Down: the gap after c is 4, which is 3 once a is out.
        insert(&mut list, Favorite::new("file:///a/"), 4);
        assert_eq!(
            uris(&list),
            ["file:///d", "file:///b", "file:///c", "file:///a"]
        );
        // Up.
        insert(&mut list, Favorite::new("file:///c"), 0);
        assert_eq!(
            uris(&list),
            ["file:///c", "file:///d", "file:///b", "file:///a"]
        );
        // Onto its own place: no change.
        insert(&mut list, Favorite::new("file:///d"), 2);
        assert_eq!(
            uris(&list),
            ["file:///c", "file:///d", "file:///b", "file:///a"]
        );
        assert!(remove(&mut list, "file:///b/"));
        assert!(!remove(&mut list, "file:///b"));
        assert_eq!(uris(&list), ["file:///c", "file:///d", "file:///a"]);
    }

    #[test]
    fn restore_puts_defaults_first() {
        let defaults = defaults();
        let mut list = vec![Favorite::new("file:///x")];
        if let Some(d) = defaults.last() {
            list.push(d.clone());
        }
        let restored = restore(&list);
        assert_eq!(restored[..defaults.len()], defaults[..]);
        assert_eq!(restored.last().unwrap().uri, "file:///x");
        assert_eq!(restored.len(), defaults.len() + 1);
    }
}
