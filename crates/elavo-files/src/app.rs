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

/// Follow the system light/dark choice (org.gnome.desktop.interface
/// color-scheme), the same key System Settings writes, and keep following it.
fn follow_color_scheme() {
    let Some(source) = gio::SettingsSchemaSource::default() else {
        return;
    };
    let Some(schema) = source.lookup("org.gnome.desktop.interface", true) else {
        return;
    };
    if !schema.has_key("color-scheme") {
        return;
    }
    let iface = gio::Settings::new("org.gnome.desktop.interface");
    let apply = |iface: &gio::Settings| {
        if let Some(gtk_settings) = gtk::Settings::default() {
            let dark = iface.string("color-scheme") == "prefer-dark";
            gtk_settings.set_gtk_application_prefer_dark_theme(dark);
        }
    };
    apply(&iface);
    iface.connect_changed(Some("color-scheme"), move |s, _| apply(s));
    // Keep the settings object alive for the life of the app.
    std::mem::forget(iface);
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
    ("win.view::columns", &["<Control>3"]),
    ("win.view::gallery", &["<Control>4"]),
    ("win.show-preview", &["<Control><Shift>p"]),
    ("win.show-hidden", &["<Control>h", "<Control><Shift>period"]),
    ("win.new-folder", &["<Control><Shift>n"]),
    ("win.add-to-sidebar", &["<Control>t"]),
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
        follow_color_scheme();
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
