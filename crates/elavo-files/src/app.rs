//! The application: one instance per session, a window per folder opened,
//! and the keyboard shortcuts. Folders and files given on the command line
//! or by another app (`gio open`, `xdg-open`, the FileManager1 interface)
//! come to the running instance: a folder opens in a window, a file is
//! shown selected in its folder.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, gio, glib};

use crate::window::Window;

thread_local! {
    static WINDOWS: RefCell<Vec<Rc<Window>>> = const { RefCell::new(Vec::new()) };
}

pub fn open_window(app: &gtk::Application, folder: Option<gio::File>) -> Rc<Window> {
    open_window_for(app, folder, None)
}

fn open_window_for(
    app: &gtk::Application,
    folder: Option<gio::File>,
    startup_id: Option<&str>,
) -> Rc<Window> {
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
    if let Some(id) = startup_id {
        w.window.set_startup_id(id);
    }
    w.window.present();
    WINDOWS.with(|all| all.borrow_mut().push(w.clone()));
    crate::menubar::sync(app);
    w
}

/// The Files window focused last, for the menu bar's application actions.
fn focused_window(app: &gtk::Application) -> Option<Rc<Window>> {
    let target = crate::menubar::target(app)?;
    WINDOWS.with(|all| all.borrow().iter().find(|w| w.window == target).cloned())
}

/// Folio's Empty Trash: asks over the Files window focused last, or over
/// a new Trash window when none is open.
fn empty_trash(app: &gtk::Application) {
    let w = focused_window(app).unwrap_or_else(|| {
        open_window(
            app,
            Some(gio::File::for_uri(&elavo_files_core::places::trash().uri)),
        )
    });
    w.empty_trash();
}

/// Actions for the menu bar that are about the application or its windows
/// as a whole.
fn add_menu_actions(app: &gtk::Application) {
    let add = |name: &str, f: fn(&gtk::Application)| {
        let action = gio::SimpleAction::new(name, None);
        let a = app.downgrade();
        action.connect_activate(move |_, _| {
            if let Some(app) = a.upgrade() {
                f(&app);
            }
        });
        app.add_action(&action);
    };
    add("empty-trash", empty_trash);
    add("shortcuts", |app| {
        crate::shortcuts::present(app, crate::menubar::target(app).as_ref())
    });
    add("minimize", |app| {
        if let Some(w) = crate::menubar::target(app) {
            w.minimize();
        }
    });
    add("zoom", |app| {
        if let Some(w) = crate::menubar::target(app) {
            if w.is_maximized() {
                w.unmaximize();
            } else {
                w.maximize();
            }
        }
    });
    add("cycle-windows", |app| {
        // Focus order: the focused window first, so the last one is the
        // one not seen for longest.
        let windows = crate::menubar::files_windows(app);
        if windows.len() >= 2 {
            if let Some(w) = windows.last() {
                w.present();
            }
        }
    });
    add("bring-all-to-front", |app| {
        let windows = crate::menubar::files_windows(app);
        // The focused one last, so it stays on top.
        for w in windows.iter().rev() {
            w.present();
        }
    });
}

/// Whether `file` opens as a folder: a directory, a place that has to be
/// mounted first, or anything not on this computer GIO can't tell yet.
/// Application bundles and files are shown in their folder instead.
fn is_folder(file: &gio::File) -> bool {
    if file.uri().ends_with(".eapp") || file.uri().ends_with(".eapp/") {
        return false;
    }
    match file.query_file_type(gio::FileQueryInfoFlags::NONE, gio::Cancellable::NONE) {
        gio::FileType::Directory | gio::FileType::Mountable => true,
        gio::FileType::Unknown => !file.is_native() || file.parent().is_none(),
        _ => false,
    }
}

/// Show folders, or items selected in their folders, or Get Info for
/// items: for the command line and FileManager1.
pub fn show(
    app: &gtk::Application,
    what: crate::filemanager1::Show,
    files: &[gio::File],
    startup_id: Option<&str>,
) {
    use crate::filemanager1::Show;
    if files.is_empty() {
        if what == Show::Folders {
            open_window_for(app, None, startup_id);
        }
        return;
    }
    if what == Show::Folders {
        for f in files {
            open_window_for(app, Some(f.clone()), startup_id);
        }
        return;
    }
    // One window per folder, the items in it selected together.
    let mut groups: Vec<(gio::File, Vec<gio::File>)> = Vec::new();
    for f in files {
        let Some(parent) = f.parent() else {
            open_window_for(app, Some(f.clone()), startup_id);
            continue;
        };
        match groups.iter_mut().find(|(p, _)| p.equal(&parent)) {
            Some((_, items)) => items.push(f.clone()),
            None => groups.push((parent, vec![f.clone()])),
        }
    }
    for (parent, items) in groups {
        let existing = WINDOWS.with(|all| all.borrow().iter().find(|w| w.shows(&parent)).cloned());
        let w = match existing {
            Some(w) => {
                if let Some(id) = startup_id {
                    w.window.set_startup_id(id);
                }
                w.window.present();
                w
            }
            None => open_window_for(app, Some(parent), startup_id),
        };
        w.show_items(&items);
        if what == Show::Properties {
            for item in &items {
                crate::info::present(&w.window, item);
            }
        }
    }
}

/// Windows showing a place under `root` go Home: it is being unmounted.
pub fn leave(root: &gio::File) {
    WINDOWS.with(|all| {
        for w in all.borrow().iter() {
            w.leave(root);
        }
    });
}

fn mounts_changed() {
    WINDOWS.with(|all| {
        for w in all.borrow().iter() {
            w.mounts_changed();
        }
    });
}

/// A drive or share about to go, by Eject here or elsewhere: leave it
/// first, so Files doesn't hold it busy.
fn follow_unmounts() {
    let monitor = gio::VolumeMonitor::get();
    monitor.connect_mount_pre_unmount(|_, m| leave(&m.root()));
    monitor.connect_mount_removed(|_, m| {
        leave(&m.root());
        mounts_changed();
    });
    // Eject (Ctrl+E) follows what is mounted, which GVfs may report after
    // a window has opened.
    monitor.connect_mount_added(|_, _| mounts_changed());
    monitor.connect_mount_changed(|_, _| mounts_changed());
    std::mem::forget(monitor);
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
            set_gtk_dark(&gtk_settings, dark);
        }
    };
    apply(&iface);
    iface.connect_changed(Some("color-scheme"), move |s, _| apply(s));
    // Keep the settings object alive for the life of the app.
    std::mem::forget(iface);
}

pub(crate) const ACCELS: &[(&str, &[&str])] = &[
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
    ("win.connect-server", &["<Control>k"]),
    ("win.eject", &["<Control>e"]),
    ("win.show-sidebar", &["<Control><Alt>s"]),
    ("win.go-to-folder", &["<Control><Shift>g"]),
    ("win.fullscreen", &["F11"]),
    ("app.shortcuts", &["<Control>question"]),
];

thread_local! {
    /// Started with --localsend-daemon: stay running with no window.
    static DAEMON: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static DAEMON_HOLD: RefCell<Option<gio::ApplicationHoldGuard>> = const { RefCell::new(None) };
}

mod imp {
    use gtk::subclass::prelude::*;
    use gtk::{gio, glib};

    #[derive(Default)]
    pub struct FilesApp;

    #[glib::object_subclass]
    impl ObjectSubclass for FilesApp {
        const NAME: &'static str = "ElavoFilesApplication";
        type Type = super::FilesApp;
        type ParentType = gtk::Application;
    }

    impl ObjectImpl for FilesApp {}

    impl ApplicationImpl for FilesApp {
        /// Files' own objects (`crate::service`) go up before Files owns
        /// its name, so a call D-Bus started Files for finds them.
        fn dbus_register(
            &self,
            connection: &gio::DBusConnection,
            object_path: &str,
        ) -> Result<(), glib::Error> {
            self.parent_dbus_register(connection, object_path)?;
            crate::service::register(connection);
            Ok(())
        }

        fn dbus_unregister(&self, connection: &gio::DBusConnection, object_path: &str) {
            crate::service::unregister(connection);
            self.parent_dbus_unregister(connection, object_path);
        }
    }

    impl GtkApplicationImpl for FilesApp {}
}

glib::wrapper! {
    /// `gtk::Application`, with Files' D-Bus objects registered with it.
    pub struct FilesApp(ObjectSubclass<imp::FilesApp>)
        @extends gtk::Application, gio::Application,
        @implements gio::ActionGroup, gio::ActionMap;
}

pub fn run() -> glib::ExitCode {
    let app: gtk::Application = glib::Object::builder::<FilesApp>()
        .property("application-id", crate::APP_ID)
        .property("flags", gio::ApplicationFlags::HANDLES_OPEN)
        .build()
        .upcast();
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
        // Other apps (Notes' Share menu) send files through this action over
        // D-Bus: `io.elavo.Files` `localsend-send` with the file URIs. It opens
        // the same device picker as Send with LocalSend… on a selection.
        let ls_send = gio::SimpleAction::new("localsend-send", Some(glib::VariantTy::STRING_ARRAY));
        let a = app.downgrade();
        ls_send.connect_activate(move |_, uris| {
            let files: Vec<gio::File> = uris
                .and_then(|v| v.get::<Vec<String>>())
                .unwrap_or_default()
                .iter()
                .map(|uri| gio::File::for_uri(uri))
                .collect();
            if files.is_empty() {
                return;
            }
            let parent = a.upgrade().and_then(|a| a.active_window());
            crate::localsend::pick_and_send(parent.as_ref(), files);
        });
        app.add_action(&ls_send);
        add_menu_actions(app);
        crate::menubar::install(app, |app, folder| open_window(app, folder).window.clone());
        app.set_menubar(Some(&crate::menubar::build()));
        crate::settings::get();
        follow_settings();
        follow_unmounts();
        crate::filemanager1::start(app);
        crate::localsend::start(app);
    });
    app.connect_activate(|app| {
        if DAEMON.with(|d| d.replace(false)) {
            // Receive until the session ends; windows come and go.
            DAEMON_HOLD.with(|h| h.replace(Some(app.hold())));
            return;
        }
        open_window(app, None);
    });
    app.connect_open(|app, files, _| {
        use crate::filemanager1::Show;
        let (folders, items): (Vec<gio::File>, Vec<gio::File>) =
            files.iter().cloned().partition(is_folder);
        if !folders.is_empty() {
            show(app, Show::Folders, &folders, None);
        }
        if !items.is_empty() {
            show(app, Show::Items, &items, None);
        }
    });
    app.run()
}

/// Load the light or dark sheet for `dark`. GTK 4.24 picks the sheet from
/// `gtk-interface-color-scheme`, and `gtk-application-prefer-dark-theme`
/// reloads the sheet before it updates that value, so one call alone loads
/// the sheet of the previous scheme. Set the scheme first where GTK has it,
/// then set the old property twice so the reload sees the new scheme. The
/// theme name is left alone so accent changes still reach the app.
fn set_gtk_dark(settings: &gtk::Settings, dark: bool) {
    let scheme = settings
        .find_property("gtk-interface-color-scheme")
        .and_then(|pspec| {
            let class = gtk::glib::EnumClass::with_type(pspec.value_type())?;
            class.to_value_by_nick(if dark { "dark" } else { "light" })
        });
    if let Some(value) = &scheme {
        settings.set_property_from_value("gtk-interface-color-scheme", value);
    }
    settings.set_gtk_application_prefer_dark_theme(dark);
    settings.set_gtk_application_prefer_dark_theme(dark);
}
