//! `~/.config/elavo/files.conf`: what Files remembers between runs.
//!
//! ```ini
//! [View]
//! Mode=icons        # icons or list
//! IconSize=64       # 48, 64, 96 or 128 (Ctrl+plus, Ctrl+minus, Ctrl+0)
//! ListIconSize=16   # 16, 24 or 32
//! ShowHidden=false
//! [Window]
//! Width=1100
//! Height=680
//! ```

use elavo_files_core::zoom;
use gtk::glib;

const FILE: &str = "files.conf";

fn path() -> std::path::PathBuf {
    glib::user_config_dir().join("elavo").join(FILE)
}

fn load() -> glib::KeyFile {
    let file = glib::KeyFile::new();
    let _ = file.load_from_file(path(), glib::KeyFileFlags::KEEP_COMMENTS);
    file
}

fn save(file: &glib::KeyFile) {
    let path = path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = file.save_to_file(&path);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Icons,
    List,
}

impl Mode {
    pub fn id(self) -> &'static str {
        match self {
            Mode::Icons => "icons",
            Mode::List => "list",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct State {
    pub mode: Mode,
    pub icon_size: i32,
    pub list_icon_size: i32,
    pub show_hidden: bool,
    pub width: i32,
    pub height: i32,
}

pub fn read() -> State {
    let f = load();
    let int = |g: &str, k: &str| f.integer(g, k).ok();
    State {
        mode: match f.string("View", "Mode").ok().as_deref() {
            Some("list") => Mode::List,
            _ => Mode::Icons,
        },
        icon_size: zoom::snap(
            &zoom::ICON_SIZES,
            int("View", "IconSize").unwrap_or(zoom::DEFAULT_ICON),
        ),
        list_icon_size: zoom::snap(
            &zoom::LIST_SIZES,
            int("View", "ListIconSize").unwrap_or(zoom::DEFAULT_LIST),
        ),
        show_hidden: f.boolean("View", "ShowHidden").unwrap_or(false),
        width: int("Window", "Width").unwrap_or(1100).clamp(640, 4000),
        height: int("Window", "Height").unwrap_or(680).clamp(420, 4000),
    }
}

pub fn write(state: &State) {
    let f = load();
    f.set_string("View", "Mode", state.mode.id());
    f.set_integer("View", "IconSize", state.icon_size);
    f.set_integer("View", "ListIconSize", state.list_icon_size);
    f.set_boolean("View", "ShowHidden", state.show_hidden);
    f.set_integer("Window", "Width", state.width);
    f.set_integer("Window", "Height", state.height);
    save(&f);
}
