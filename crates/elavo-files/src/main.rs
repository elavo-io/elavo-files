//! Elavo Files, a Finder-style file manager on GIO.
//!
//! ```text
//! elavo-files [FOLDER|URI]...   open a window for each, or the home folder
//! elavo-files --localsend-daemon receive with LocalSend, with no window
//! ```

mod app;
mod bundles;
mod clip;
mod columns;
mod dialogs;
mod gallery;
mod i18n;
mod info;
mod jobs;
mod localsend;
mod menus;
mod preview;
mod quicklook;
mod rubberband;
mod search;
mod settings;
mod sidebar;
mod state;
mod terminal;
mod thumbs;
mod titlebox;
mod views;
mod window;

use gtk::glib;

pub const APP_ID: &str = "io.elavo.Files";

fn main() -> glib::ExitCode {
    i18n::init();
    gtk::gio::resources_register_include!("elavo-files.gresource").expect("compiled resources");
    app::run()
}
