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
    let folder = folder.unwrap_or_else(crate::settings::new_window_folder);
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

/// Show Hidden Files in every window (Files Settings), and for windows
/// opened later.
pub fn set_show_hidden(on: bool) {
    let mut any = false;
    WINDOWS.with(|all| {
        for w in all.borrow().iter() {
            w.set_show_hidden(on);
            any = true;
        }
    });
    if !any {
        let mut st = crate::state::read();
        st.show_hidden = on;
        crate::state::write(&st);
    }
}

/// Files Settings changed: every window shows its folder again.
fn follow_settings() {
    crate::settings::connect(|old, new| {
        WINDOWS.with(|all| {
            for w in all.borrow().iter() {
                w.settings_changed(old, new);
            }
        });
    });
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
    ("win.get-info", &["<Control>i"]),
    ("win.open-terminal", &["<Control><Alt>t"]),
    ("app.settings", &["<Control>comma"]),
];

thread_local! {
    /// Started with --localsend-daemon: stay running with no window.
    static DAEMON: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static DAEMON_HOLD: RefCell<Option<gio::ApplicationHoldGuard>> = const { RefCell::new(None) };
}

pub fn run() -> glib::ExitCode {
    let app = gtk::Application::builder()
        .application_id(crate::APP_ID)
        .flags(gio::ApplicationFlags::HANDLES_OPEN)
        .build();
    app.add_main_option(
        "localsend-daemon",
        glib::Char::from(0),
        glib::OptionFlags::NONE,
        glib::OptionArg::None,
        "Receive with LocalSend in the background, with no window",
        None,
    );
    app.connect_handle_local_options(|app, options| {
        if options.contains("localsend-daemon") {
            if let Err(e) = app.register(gio::Cancellable::NONE) {
                eprintln!("elavo-files: {e}");
                return std::ops::ControlFlow::Break(glib::ExitCode::FAILURE);
            }
            if app.is_remote() {
                // Files is running already, and receiving with it.
                return std::ops::ControlFlow::Break(glib::ExitCode::SUCCESS);
            }
            DAEMON.with(|d| d.set(true));
        }
        std::ops::ControlFlow::Continue(())
    });
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
        let settings = gio::SimpleAction::new("settings", None);
        let a = app.downgrade();
        settings.connect_activate(move |_, _| {
            let parent = a.upgrade().and_then(|a| a.active_window());
            crate::settings::present(parent.as_ref());
        });
        app.add_action(&settings);
        let ls_settings = gio::SimpleAction::new("localsend-settings", None);
        let a = app.downgrade();
        ls_settings.connect_activate(move |_, _| {
            let parent = a.upgrade().and_then(|a| a.active_window());
            crate::settings::present_tab(parent.as_ref(), "localsend");
        });
        app.add_action(&ls_settings);
        crate::settings::get();
        follow_settings();
        crate::localsend::start(app);
    });
    app.connect_activate(|app| {
        if DAEMON.with(|d| d.replace(false)) {
            // Receive until the session ends; windows come and go.
            DAEMON_HOLD.with(|h| h.replace(Some(app.hold())));
            return;
        }
        open_window(app, None)
    });
    app.connect_open(|app, files, _| {
        for file in files {
            open_window(app, Some(file.clone()));
        }
    });
    app.run()
}
