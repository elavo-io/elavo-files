//! Elavo Files, a Finder-style file manager on GIO.
//!
//! ```text
//! elavo-files [FOLDER|URI]...   open a window for each, or the home folder
//! ```

mod app;
mod bundles;
mod clip;
mod dialogs;
mod i18n;
mod jobs;
mod menus;
mod rubberband;
mod sidebar;
mod state;
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
