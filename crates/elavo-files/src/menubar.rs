//! The menu bar for Folio's global menu: File, Edit, View, Go, Window and
//! Help, in Finder's order. Files draws no menu bar in its windows; the
//! panel reads this one over D-Bus (`/io/elavo/Files/menus/menubar`) and
//! puts Folio's own menu in front of it, since Files is part of Folio.
//!
//! The panel can't tell which Files window has the keyboard, so every item
//! names an `app.*` action. Those for a window are `app.win-<name>`: they
//! forward to the window's `win.<name>` in the Files window focused last,
//! and follow that window's enabled flags and check marks. Folders and
//! places open a window when none is open.

use std::cell::RefCell;

use gtk::prelude::*;
use gtk::{gio, glib};

use crate::i18n::tr;

/// Window actions with no parameter and no state.
const PLAIN: &[&str] = &[
    "close",
    "new-folder",
    "open",
    "get-info",
    "rename",
    "compress",
    "duplicate",
    "make-alias",
    "quick-look",
    "add-to-sidebar",
    "trash",
    "eject",
    "share",
    "open-terminal",
    "search",
    "undo",
    "redo",
    "cut",
    "copy",
    "paste",
    "paste-move",
    "select-all",
    "zoom-in",
    "zoom-out",
    "zoom-reset",
    "fullscreen",
    "back",
    "forward",
    "up",
    "home",
    "go-to-folder",
    "connect-server",
    "show-progress",
];

/// Window actions with a check mark.
const TOGGLES: &[&str] = &["show-sidebar", "show-preview", "show-hidden"];

/// Forwarded, but open a window first when there is none.
const OPEN_A_WINDOW: &[&str] = &["home", "go-uri", "connect-server", "go-to-folder", "search"];

/// Gets the window to act on, opening one at `folder` (Home when None).
type Opener = Box<dyn Fn(&gtk::Application, Option<gio::File>) -> gtk::ApplicationWindow>;

thread_local! {
    static TARGET: RefCell<Option<(glib::WeakRef<gtk::ApplicationWindow>, Vec<glib::SignalHandlerId>)>> =
        const { RefCell::new(None) };
    static OPENER: RefCell<Option<Opener>> = const { RefCell::new(None) };
}

fn item(label: &str, action: &str, accel: Option<&str>) -> gio::MenuItem {
    let item = gio::MenuItem::new(Some(label), Some(action));
    if let Some(accel) = accel {
        item.set_attribute_value("accel", Some(&accel.to_variant()));
    }
    item
}

fn target_item(label: &str, action: &str, target: &str, accel: Option<&str>) -> gio::MenuItem {
    let item = gio::MenuItem::new(Some(label), None);
    item.set_action_and_target_value(Some(action), Some(&target.to_variant()));
    if let Some(accel) = accel {
        item.set_attribute_value("accel", Some(&accel.to_variant()));
    }
    item
}

fn section(items: &[gio::MenuItem]) -> gio::Menu {
    let menu = gio::Menu::new();
    for i in items {
        menu.append_item(i);
    }
    menu
}

fn menu(sections: &[gio::Menu]) -> gio::Menu {
    let menu = gio::Menu::new();
    for s in sections {
        menu.append_section(None, s);
    }
    menu
}

/// A place in Go, by URI, or None when this computer has no such folder.
fn special(dir: glib::UserDirectory) -> Option<String> {
    glib::user_special_dir(dir).map(|p| gio::File::for_path(p).uri().to_string())
}

pub fn build() -> gio::Menu {
    let bar = gio::Menu::new();

    let file = menu(&[
        section(&[
            item(
                &tr("New Folio Window"),
                "app.new-window",
                Some("<Control>n"),
            ),
            item(
                &tr("New Folder"),
                "app.win-new-folder",
                Some("<Control><Shift>n"),
            ),
            item(&tr("Open"), "app.win-open", Some("<Control>o")),
            item(&tr("Close Window"), "app.win-close", Some("<Control>w")),
        ]),
        section(&[
            item(&tr("Get Info"), "app.win-get-info", Some("<Control>i")),
            item(&tr("Rename\u{2026}"), "app.win-rename", Some("F2")),
            item(&tr("Compress"), "app.win-compress", None),
            item(&tr("Duplicate"), "app.win-duplicate", Some("<Control>d")),
            item(&tr("Make Alias"), "app.win-make-alias", None),
            item(&tr("Quick Look"), "app.win-quick-look", Some("space")),
            item(
                &tr("Add to Sidebar"),
                "app.win-add-to-sidebar",
                Some("<Control>t"),
            ),
        ]),
        section(&[
            item(&tr("Move to Trash"), "app.win-trash", Some("Delete")),
            item(&tr("Eject"), "app.win-eject", Some("<Control>e")),
        ]),
        section(&[
            item(&tr("Send with LocalSend\u{2026}"), "app.win-share", None),
            item(
                &tr("Open in Terminal"),
                "app.win-open-terminal",
                Some("<Control><Alt>t"),
            ),
        ]),
        section(&[item(&tr("Find"), "app.win-search", Some("<Control>f"))]),
    ]);
    bar.append_submenu(Some(&tr("File")), &file);

    let edit = menu(&[
        section(&[
            item(&tr("Undo"), "app.win-undo", Some("<Control>z")),
            item(&tr("Redo"), "app.win-redo", Some("<Control><Shift>z")),
        ]),
        section(&[
            item(&tr("Cut"), "app.win-cut", Some("<Control>x")),
            item(&tr("Copy"), "app.win-copy", Some("<Control>c")),
            item(&tr("Paste"), "app.win-paste", Some("<Control>v")),
            item(
                &tr("Move Item Here"),
                "app.win-paste-move",
                Some("<Control><Shift>v"),
            ),
            item(&tr("Select All"), "app.win-select-all", Some("<Control>a")),
        ]),
    ]);
    bar.append_submenu(Some(&tr("Edit")), &edit);

    let view = menu(&[
        section(&[
            target_item(&tr("as Icons"), "app.win-view", "icons", Some("<Control>1")),
            target_item(&tr("as List"), "app.win-view", "list", Some("<Control>2")),
            target_item(
                &tr("as Columns"),
                "app.win-view",
                "columns",
                Some("<Control>3"),
            ),
            target_item(
                &tr("as Gallery"),
                "app.win-view",
                "gallery",
                Some("<Control>4"),
            ),
        ]),
        section(&[
            item(
                &tr("Show Sidebar"),
                "app.win-show-sidebar",
                Some("<Control><Alt>s"),
            ),
            item(
                &tr("Show Preview"),
                "app.win-show-preview",
                Some("<Control><Shift>p"),
            ),
            item(
                &tr("Show Hidden Files"),
                "app.win-show-hidden",
                Some("<Control>h"),
            ),
        ]),
        section(&[
            item(
                &tr("Make Icons Larger"),
                "app.win-zoom-in",
                Some("<Control>plus"),
            ),
            item(
                &tr("Make Icons Smaller"),
                "app.win-zoom-out",
                Some("<Control>minus"),
            ),
            item(
                &tr("Default Icon Size"),
                "app.win-zoom-reset",
                Some("<Control>0"),
            ),
        ]),
        section(&[item(
            &tr("Enter Full Screen"),
            "app.win-fullscreen",
            Some("F11"),
        )]),
    ]);
    bar.append_submenu(Some(&tr("View")), &view);

    let home = gio::File::for_path(glib::home_dir()).uri().to_string();
    let mut places = vec![target_item(
        &tr("Recents"),
        "app.win-go-uri",
        "recent:///",
        None,
    )];
    for (label, dir) in [
        (tr("Documents"), glib::UserDirectory::Documents),
        (tr("Desktop"), glib::UserDirectory::Desktop),
        (tr("Downloads"), glib::UserDirectory::Downloads),
    ] {
        if let Some(uri) = special(dir) {
            places.push(target_item(&label, "app.win-go-uri", &uri, None));
        }
    }
    places.push(item(&tr("Home"), "app.win-home", Some("<Control><Shift>h")));
    // Files has no Computer view: the sidebar lists the drives under
    // Locations beside Home.
    places.push(target_item(&tr("Computer"), "app.win-go-uri", &home, None));
    places.push(target_item(
        &tr("Network"),
        "app.win-go-uri",
        "network:///",
        None,
    ));
    let apps = elavo_files_core::places::applications();
    if gio::File::for_uri(&apps.uri).query_exists(gio::Cancellable::NONE) {
        places.push(target_item(
            &tr("Applications"),
            "app.win-go-uri",
            &apps.uri,
            None,
        ));
    }
    let go = menu(&[
        section(&[
            item(&tr("Back"), "app.win-back", Some("<Control>bracketleft")),
            item(
                &tr("Forward"),
                "app.win-forward",
                Some("<Control>bracketright"),
            ),
            item(&tr("Enclosing Folder"), "app.win-up", Some("<Control>Up")),
        ]),
        section(&places),
        section(&[
            item(
                &tr("Go to Folder\u{2026}"),
                "app.win-go-to-folder",
                Some("<Control><Shift>g"),
            ),
            item(
                &tr("Connect to Server\u{2026}"),
                "app.win-connect-server",
                Some("<Control>k"),
            ),
        ]),
    ]);
    bar.append_submenu(Some(&tr("Go")), &go);

    let window = menu(&[
        section(&[
            item(&tr("Minimize"), "app.minimize", None),
            item(&tr("Zoom"), "app.zoom", None),
        ]),
        section(&[
            item(&tr("Cycle Through Windows"), "app.cycle-windows", None),
            item(&tr("Show Progress Window"), "app.win-show-progress", None),
        ]),
        section(&[item(
            &tr("Bring All to Front"),
            "app.bring-all-to-front",
            None,
        )]),
    ]);
    bar.append_submenu(Some(&tr("Window")), &window);

    // Folio Help names no action yet, so it shows dimmed.
    let help = menu(&[section(&[
        item(&tr("Folio Help"), "app.help", None),
        item(
            &tr("Keyboard Shortcuts"),
            "app.shortcuts",
            Some("<Control>question"),
        ),
    ])]);
    bar.append_submenu(Some(&tr("Help")), &help);
    bar
}

/// The Files window focused last: GTK keeps the application's windows
/// in focus order. Dialogs and Settings are skipped.
pub fn target(app: &gtk::Application) -> Option<gtk::ApplicationWindow> {
    app.windows()
        .into_iter()
        .filter(is_files_window)
        .find_map(|w| w.downcast::<gtk::ApplicationWindow>().ok())
}

/// Add the `app.win-*` actions. `open` opens a window at a folder, for the
/// places when no window is open.
pub fn install(
    app: &gtk::Application,
    open: impl Fn(&gtk::Application, Option<gio::File>) -> gtk::ApplicationWindow + 'static,
) {
    OPENER.with(|o| o.replace(Some(Box::new(open))));
    for name in PLAIN {
        add(app, name, None, None);
    }
    for name in TOGGLES {
        add(app, name, None, Some(true.to_variant()));
    }
    add(
        app,
        "view",
        Some(glib::VariantTy::STRING),
        Some("icons".to_variant()),
    );
    add(app, "go-uri", Some(glib::VariantTy::STRING), None);
    app.connect_active_window_notify(sync);
    app.connect_window_removed(|app, _| sync(app));
    app.connect_window_added(|app, _| sync(app));
    sync(app);
}

fn add(
    app: &gtk::Application,
    name: &'static str,
    param: Option<&glib::VariantTy>,
    state: Option<glib::Variant>,
) {
    let action = match state {
        Some(state) => gio::SimpleAction::new_stateful(&format!("win-{name}"), param, &state),
        None => gio::SimpleAction::new(&format!("win-{name}"), param),
    };
    let a = app.downgrade();
    action.connect_activate(move |_, param| {
        let Some(app) = a.upgrade() else {
            return;
        };
        let window = match target(&app) {
            Some(w) => w,
            None => {
                if !OPEN_A_WINDOW.contains(&name) {
                    return;
                }
                let folder = match name {
                    "go-uri" => param.and_then(|p| p.str()).map(gio::File::for_uri),
                    "home" => Some(gio::File::for_path(glib::home_dir())),
                    _ => None,
                };
                let Some(w) = OPENER.with(|o| o.borrow().as_ref().map(|open| open(&app, folder)))
                else {
                    return;
                };
                if matches!(name, "go-uri" | "home") {
                    // The new window shows the place already.
                    return;
                }
                w
            }
        };
        gio::prelude::ActionGroupExt::activate_action(&window, name, param);
    });
    app.add_action(&action);
}

fn forwarded() -> impl Iterator<Item = &'static str> {
    PLAIN
        .iter()
        .chain(TOGGLES)
        .copied()
        .chain(["view", "go-uri"])
}

/// Follow the window focused last: its enabled flags and check marks.
pub fn sync(app: &gtk::Application) {
    let target = target(app);
    let same = TARGET.with(|t| {
        t.borrow()
            .as_ref()
            .and_then(|(w, _)| w.upgrade())
            .zip(target.as_ref())
            .is_some_and(|(old, new)| old == *new)
    });
    if !same {
        TARGET.with(|t| {
            if let Some((old, handlers)) = t.borrow_mut().take() {
                if let Some(old) = old.upgrade() {
                    for h in handlers {
                        old.disconnect(h);
                    }
                }
            }
        });
        if let Some(w) = &target {
            let a = app.downgrade();
            let enabled = w.connect_action_enabled_changed(None, move |_, name, _| {
                if let Some(app) = a.upgrade() {
                    copy_one(&app, name);
                }
            });
            let a = app.downgrade();
            let state = w.connect_action_state_changed(None, move |_, name, _| {
                if let Some(app) = a.upgrade() {
                    copy_one(&app, name);
                }
            });
            TARGET.with(|t| t.replace(Some((w.downgrade(), vec![enabled, state]))));
        }
    }
    for name in forwarded() {
        copy_one(app, name);
    }
    let windows = files_windows(app).len();
    for (name, on) in [
        ("minimize", target.is_some()),
        ("zoom", target.is_some()),
        ("cycle-windows", windows >= 2),
        ("bring-all-to-front", windows > 0),
    ] {
        if let Some(a) = app.lookup_action(name).and_downcast::<gio::SimpleAction>() {
            a.set_enabled(on);
        }
    }
}

/// A Files window rather than a dialog or Settings.
fn is_files_window(w: &gtk::Window) -> bool {
    w.downcast_ref::<gtk::ApplicationWindow>()
        .is_some_and(|w| w.lookup_action("go-uri").is_some())
}

/// Copy one window action's enabled flag and state to `app.win-<name>`.
fn copy_one(app: &gtk::Application, name: &str) {
    if !forwarded().any(|n| n == name) {
        return;
    }
    let Some(ours) = app
        .lookup_action(&format!("win-{name}"))
        .and_downcast::<gio::SimpleAction>()
    else {
        return;
    };
    match target(app) {
        Some(w) => {
            ours.set_enabled(gio::prelude::ActionGroupExt::is_action_enabled(&w, name));
            if let Some(state) = gio::prelude::ActionGroupExt::action_state(&w, name) {
                if ours.state().is_some() {
                    ours.set_state(&state);
                }
            }
        }
        None => ours.set_enabled(OPEN_A_WINDOW.contains(&name)),
    }
}

/// Every Files window, for Cycle Through Windows and Bring All to Front.
pub fn files_windows(app: &gtk::Application) -> Vec<gtk::ApplicationWindow> {
    app.windows()
        .into_iter()
        .filter(is_files_window)
        .filter_map(|w| w.downcast::<gtk::ApplicationWindow>().ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forwarded_names_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for name in forwarded() {
            assert!(seen.insert(name), "{name} twice");
        }
    }

    #[test]
    fn places_that_open_a_window_are_forwarded() {
        for name in OPEN_A_WINDOW {
            assert!(forwarded().any(|n| n == *name), "{name}");
        }
    }
}
