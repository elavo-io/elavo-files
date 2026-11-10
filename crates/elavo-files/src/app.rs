//! The application: one instance per session, a window per folder opened,
//! and the keyboard shortcuts.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, gio, glib};

use crate::window::Window;

thread_local! {
    static WINDOWS: RefCell<Vec<Rc<Window>>> = const { RefCell::new(Vec::new()) };
}

pub fn open_window(app: &gtk::Application, folder: Option<gio::File>) {
    let folder = folder.unwrap_or_else(|| gio::File::for_path(glib::home_dir()));
    let w = Window::new(app, folder);
    let weak = Rc::downgrade(&w);
    w.window.connect_close_request(move |_| {
        if let Some(w) = weak.upgrade() {
            w.remember();
            WINDOWS.with(|all| all.borrow_mut().retain(|x| !Rc::ptr_eq(x, &w)));
        }
        glib::Propagation::Proceed
    });
    w.window.present();
    WINDOWS.with(|all| all.borrow_mut().push(w));
}

fn load_css() {
    let Some(display) = gdk::Display::default() else {
        return;
    };
    let css = gtk::CssProvider::new();
    css.load_from_resource("/io/elavo/Files/style.css");
    gtk::style_context_add_provider_for_display(
        &display,
        &css,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
    gtk::IconTheme::for_display(&display).add_resource_path("/io/elavo/Files/icons");
}

const ACCELS: &[(&str, &[&str])] = &[
    ("app.new-window", &["<Control>n"]),
    ("win.close", &["<Control>w"]),
    ("win.back", &["<Alt>Left", "<Control>bracketleft"]),
    ("win.forward", &["<Alt>Right", "<Control>bracketright"]),
    ("win.up", &["<Alt>Up", "<Control>Up"]),
    ("win.home", &["<Alt>Home", "<Control><Shift>h"]),
    ("win.view::icons", &["<Control>1"]),
    ("win.view::list", &["<Control>2"]),
    ("win.show-hidden", &["<Control>h", "<Control><Shift>period"]),
    ("win.new-folder", &["<Control><Shift>n"]),
    (
        "win.zoom-in",
        &["<Control>plus", "<Control>equal", "<Control>KP_Add"],
    ),
    ("win.zoom-out", &["<Control>minus", "<Control>KP_Subtract"]),
    ("win.zoom-reset", &["<Control>0", "<Control>KP_0"]),
    ("win.search", &["<Control>f"]),
    ("win.reload", &["F5", "<Control>r"]),
];

pub fn run() -> glib::ExitCode {
    let app = gtk::Application::builder()
        .application_id(crate::APP_ID)
        .flags(gio::ApplicationFlags::HANDLES_OPEN)
        .build();
    app.connect_startup(|app| {
        load_css();
        for (action, accels) in ACCELS {
            app.set_accels_for_action(action, accels);
        }
        let new_window = gio::SimpleAction::new("new-window", None);
        let a = app.downgrade();
        new_window.connect_activate(move |_, _| {
            if let Some(app) = a.upgrade() {
                open_window(&app, None);
            }
        });
        app.add_action(&new_window);
    });
    app.connect_activate(|app| open_window(app, None));
    app.connect_open(|app, files, _| {
        for file in files {
            open_window(app, Some(file.clone()));
        }
    });
    app.run()
}
