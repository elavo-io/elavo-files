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

/// Applications, Desktop, Documents, Downloads, then the hidden ones.
pub fn favorites() -> Vec<Place> {
    use glib::UserDirectory as D;
    let mut out = vec![Place {
        id: "applications",
        uri: gio::File::for_path(glib::home_dir().join("Applications"))
            .uri()
            .to_string(),
        shown: true,
    }];
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

#[cfg(test)]
mod tests {
    use super::*;

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
