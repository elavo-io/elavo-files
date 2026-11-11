//! The sidebar's fixed places. Mounted drives come from GIO's volume
//! monitor in the app.

use gio::glib;
use gio::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Place {
    /// Stable id: `recents`, `applications`, `desktop`, `home`, `trash`...
    pub id: &'static str,
    pub uri: String,
    /// Music, Pictures, Videos, Public and Templates exist but stay out of
    /// the sidebar until the person adds them (Files Settings, later).
    pub shown: bool,
}

fn special(id: &'static str, dir: glib::UserDirectory, shown: bool) -> Option<Place> {
    let path = glib::user_special_dir(dir)?;
    // An unset XDG directory falls back to home; that is not a favorite.
    if path == glib::home_dir() {
        return None;
    }
    Some(Place {
        id,
        uri: gio::File::for_path(path).uri().to_string(),
        shown,
    })
}

pub fn recents() -> Place {
    Place {
        id: "recents",
        uri: "recent:///".to_owned(),
        shown: true,
    }
}

/// The system-wide Applications folder Elavo installs into (`/Applications`,
/// see `eapp::shared_dir`). `~/Applications` only when there is none.
pub fn applications() -> Place {
    let shared = crate::eapp::shared_dir();
    let dir = if shared.is_dir() {
        shared
    } else {
        crate::eapp::user_dir()
    };
    Place {
        id: "applications",
        uri: gio::File::for_path(dir).uri().to_string(),
        shown: true,
    }
}

/// Applications, Desktop, Documents, Downloads, then the hidden ones.
pub fn favorites() -> Vec<Place> {
    use glib::UserDirectory as D;
    let mut out = vec![applications()];
    out.extend(
        [
            special("desktop", D::Desktop, true),
            special("documents", D::Documents, true),
            special("downloads", D::Downloads, true),
            special("music", D::Music, false),
            special("pictures", D::Pictures, false),
            special("videos", D::Videos, false),
            special("public", D::PublicShare, false),
            special("templates", D::Templates, false),
        ]
        .into_iter()
        .flatten(),
    );
    out
}

pub fn home() -> Place {
    Place {
        id: "home",
        uri: gio::File::for_path(glib::home_dir()).uri().to_string(),
        shown: true,
    }
}

pub fn network() -> Place {
    Place {
        id: "network",
        uri: "network:///".to_owned(),
        shown: true,
    }
}

pub fn trash() -> Place {
    Place {
        id: "trash",
        uri: "trash:///".to_owned(),
        shown: true,
    }
}

/// The folder and the ones it is in, nearest first, up to the root. A place
/// that is not on disk (Trash, Recents, Network) is only itself.
pub fn ancestry(file: &gio::File) -> Vec<gio::File> {
    let mut out = vec![file.clone()];
    if file.path().is_none() {
        return out;
    }
    let mut at = file.clone();
    while let Some(parent) = at.parent() {
        out.push(parent.clone());
        at = parent;
    }
    out
}

/// A system file, the host's when running in a toolbox.
fn system_file(path: &str) -> Option<String> {
    std::fs::read_to_string(format!("/run/host{path}"))
        .or_else(|_| std::fs::read_to_string(path))
        .ok()
}

/// What the root drive is called, as Finder's "Macintosh HD": the system's
/// name from os-release with "HD", or "File System".
pub fn root_drive_name() -> String {
    system_file("/etc/os-release")
        .or_else(|| system_file("/usr/lib/os-release"))
        .and_then(|t| {
            t.lines()
                .find_map(|l| l.strip_prefix("NAME="))
                .map(|v| v.trim().trim_matches('"').to_owned())
        })
        .filter(|n| !n.is_empty())
        .map(|n| format!("{n} HD"))
        .unwrap_or_else(|| "File System".to_owned())
}

/// The computer's name: the pretty hostname, else the hostname.
pub fn computer_name() -> String {
    system_file("/etc/machine-info")
        .and_then(|t| {
            t.lines()
                .find_map(|l| l.strip_prefix("PRETTY_HOSTNAME="))
                .map(|v| v.trim().trim_matches('"').to_owned())
        })
        .filter(|n| !n.is_empty())
        .or_else(|| {
            system_file("/etc/hostname")
                .map(|h| h.trim().to_owned())
                .filter(|h| !h.is_empty())
        })
        .unwrap_or_else(|| glib::host_name().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ancestry_runs_to_the_root() {
        let chain = ancestry(&gio::File::for_path("/home/kevin/Desktop"));
        let paths: Vec<_> = chain.iter().map(|f| f.path().unwrap()).collect();
        assert_eq!(
            paths,
            ["/home/kevin/Desktop", "/home/kevin", "/home", "/"].map(std::path::PathBuf::from)
        );
        assert_eq!(ancestry(&gio::File::for_uri("trash:///")).len(), 1);
        assert!(!computer_name().is_empty());
        assert!(root_drive_name().ends_with("HD") || root_drive_name() == "File System");
    }

    #[test]
    fn favorites_start_with_applications() {
        let favs = favorites();
        assert_eq!(favs[0].id, "applications");
        assert!(favs[0].uri.ends_with("/Applications"));
        assert!(favs.iter().filter(|p| !p.shown).all(|p| matches!(
            p.id,
            "music" | "pictures" | "videos" | "public" | "templates"
        )));
    }
}
