//! Elavo Files, a Finder-style file manager on GIO.
//!
//! ```text
//! elavo-files [FOLDER|FILE|URI]...  open a window for each folder, or the
//!                                  home folder; a file shows selected in its folder
//! elavo-files --localsend-daemon    receive with LocalSend, with no window
//! elavo-files --gapplication-service  started by D-Bus (io.elavo.Files,
//!                                  org.freedesktop.FileManager1)
//! ```

mod app;
mod bundles;
mod clip;
mod columns;
mod connect;
mod dialogs;
mod filemanager1;
mod gallery;
mod gotofolder;
mod i18n;
mod info;
mod jobs;
mod localsend;
mod menubar;
mod menus;
mod mounts;
mod preview;
mod quicklook;
mod rubberband;
mod search;
mod settings;
mod shortcuts;
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
    // Dialogs that aren't the application's windows take their Wayland
    // app_id from the program name: Folio's panel then shows Files' menus
    // over them too, and the dock groups them with Files.
    glib::set_prgname(Some(APP_ID));
    gtk::gio::resources_register_include!("elavo-files.gresource").expect("compiled resources");
    app::run()
}
