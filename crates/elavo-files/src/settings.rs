//! Files Settings (Ctrl+comma, or Files Settings… in the More menu), as
//! Finder's: General, Sidebar and Advanced. Everything is saved to
//! `~/.config/elavo/files.conf` as it changes (see `prefs` in the core)
//! and every window follows at once.

use std::cell::RefCell;

use elavo_files_core::places;
use elavo_files_core::prefs::{self, Prefs};
use gtk::prelude::*;
use gtk::{gio, glib};

use crate::i18n::tr;
use crate::sidebar;

type Listener = Box<dyn Fn(&Prefs, &Prefs)>;

thread_local! {
    static CURRENT: RefCell<Option<Prefs>> = const { RefCell::new(None) };
    static LISTENERS: RefCell<Vec<Listener>> = const { RefCell::new(Vec::new()) };
    static WINDOW: RefCell<glib::WeakRef<gtk::Window>> = RefCell::new(glib::WeakRef::new());
}

/// The settings in force, read once.
pub fn get() -> Prefs {
    CURRENT.with(|c| {
        c.borrow_mut()
            .get_or_insert_with(|| {
                let p = prefs::read();
                prefs::apply(&p);
                p
            })
            .clone()
    })
}

/// Call `f(old, new)` after every change.
pub fn connect(f: impl Fn(&Prefs, &Prefs) + 'static) {
    LISTENERS.with(|l| l.borrow_mut().push(Box::new(f)));
}

/// Change a setting: saved, then applied everywhere.
pub fn update(f: impl FnOnce(&mut Prefs)) {
    let old = get();
    let mut new = old.clone();
    f(&mut new);
    if new == old {
        return;
    }
    prefs::apply(&new);
    if let Err(err) = prefs::write(&new) {
        eprintln!("elavo-files: saving settings: {err}");
    }
    CURRENT.with(|c| c.replace(Some(new.clone())));
    LISTENERS.with(|l| {
        for f in l.borrow().iter() {
            f(&old, &new);
        }
    });
}

/// The folder New Window opens.
pub fn new_window_folder() -> gio::File {
    let home = || gio::File::for_path(glib::home_dir());
    let special = |d| glib::user_special_dir(d).map(gio::File::for_path);
    match get().new_window_folder.as_str() {
        "recents" => gio::File::for_uri(&places::recents().uri),
        "applications" => gio::File::for_uri(&places::applications().uri),
        "desktop" => special(glib::UserDirectory::Desktop).unwrap_or_else(home),
        "documents" => special(glib::UserDirectory::Documents).unwrap_or_else(home),
        "downloads" => special(glib::UserDirectory::Downloads).unwrap_or_else(home),
        _ => home(),
    }
}

fn folder_label(id: &str) -> String {
    match id {
        "home" => glib::user_name().to_string_lossy().into_owned(),
        "recents" => tr("Recents"),
        "applications" => tr("Applications"),
        "desktop" => tr("Desktop"),
        "documents" => tr("Documents"),
        "downloads" => tr("Downloads"),
        other => other.to_owned(),
    }
}

fn check(label: &str, active: bool, on_toggle: impl Fn(bool) + 'static) -> gtk::CheckButton {
    let c = gtk::CheckButton::builder()
        .label(label)
        .active(active)
        .build();
    c.connect_toggled(move |c| on_toggle(c.is_active()));
    c
}

fn heading(text: &str) -> gtk::Label {
    gtk::Label::builder()
        .label(text)
        .xalign(0.0)
        .css_classes(["files-settings-heading"])
        .build()
}

fn group() -> gtk::Box {
    gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .css_classes(["files-settings-group"])
        .build()
}

fn page() -> gtk::Box {
    gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(10)
        .margin_top(16)
        .margin_bottom(18)
        .margin_start(18)
        .margin_end(18)
        .build()
}

fn general_page(on_hidden: std::rc::Rc<dyn Fn(bool)>) -> gtk::Box {
    let p = get();
    let page = page();
    let row = gtk::Box::builder().spacing(10).build();
    row.append(
        &gtk::Label::builder()
            .label(tr("New windows show:"))
            .xalign(0.0)
            .build(),
    );
    let labels: Vec<String> = prefs::NEW_WINDOW_FOLDERS
        .iter()
        .map(|id| folder_label(id))
        .collect();
    let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
    let folders = gtk::DropDown::from_strings(&refs);
    folders.set_hexpand(true);
    folders.set_selected(
        prefs::NEW_WINDOW_FOLDERS
            .iter()
            .position(|id| *id == p.new_window_folder)
            .unwrap_or(0) as u32,
    );
    folders.connect_selected_notify(|d| {
        if let Some(id) = prefs::NEW_WINDOW_FOLDERS.get(d.selected() as usize) {
            update(|p| p.new_window_folder = (*id).to_owned());
        }
    });
    row.append(&folders);
    let g = group();
    g.append(&row);
    page.append(&g);

    let g = group();
    g.append(&check(
        &tr("Show hidden files"),
        crate::state::read().show_hidden,
        move |on| on_hidden(on),
    ));
    g.append(&check(
        &tr("Keep folders on top when sorting by name"),
        p.folders_first,
        |on| update(|p| p.folders_first = on),
    ));
    page.append(&g);
    page
}

fn sidebar_page() -> gtk::Box {
    let p = get();
    let page = page();
    page.append(
        &gtk::Label::builder()
            .label(tr("Show these items in the sidebar:"))
            .xalign(0.0)
            .build(),
    );
    let g = group();
    g.append(&check(&tr("Recents"), p.show_recents, |on| {
        update(|p| p.show_recents = on)
    }));
    page.append(&g);

    let g = group();
    g.append(&heading(&tr("Favorites")));
    // Two columns, so the tab fits a 768-pixel-high screen.
    let favs = gtk::Grid::builder()
        .column_spacing(12)
        .row_spacing(6)
        .column_homogeneous(true)
        .build();
    for (i, place) in places::favorites().into_iter().enumerate() {
        let (name, _) = sidebar::label_of(place.id);
        let uri = place.uri.clone();
        let c = check(&name, sidebar::is_favorite(&place.uri), move |on| {
            sidebar::set_favorite(&uri, on)
        });
        favs.attach(&c, (i % 2) as i32, (i / 2) as i32, 1, 1);
    }
    g.append(&favs);
    page.append(&g);

    let g = group();
    g.append(&heading(&tr("Locations")));
    let locs = gtk::Grid::builder()
        .column_spacing(12)
        .row_spacing(6)
        .column_homogeneous(true)
        .build();
    let items = [
        check(&glib::user_name().to_string_lossy(), p.show_home, |on| {
            update(|p| p.show_home = on)
        }),
        check(&tr("Drives and volumes"), p.show_drives, |on| {
            update(|p| p.show_drives = on)
        }),
        check(&tr("Network"), p.show_network, |on| {
            update(|p| p.show_network = on)
        }),
        check(&tr("Trash"), p.show_trash, |on| {
            update(|p| p.show_trash = on)
        }),
    ];
    for (i, c) in items.iter().enumerate() {
        locs.attach(c, (i % 2) as i32, (i / 2) as i32, 1, 1);
    }
    g.append(&locs);
    page.append(&g);

    let g = group();
    g.append(&heading(&tr("Shared")));
    let localsend = check(&tr("LocalSend"), p.show_localsend, |on| {
        update(|p| p.show_localsend = on)
    });
    localsend.set_tooltip_text(Some(&tr("Sharing with LocalSend comes in a later version")));
    g.append(&localsend);
    page.append(&g);
    page
}

fn advanced_page() -> gtk::Box {
    let p = get();
    let page = page();
    let g = group();
    g.append(&check(
        &tr("Show all filename extensions"),
        p.show_extensions,
        |on| update(|p| p.show_extensions = on),
    ));
    g.append(&check(
        &tr("Show warning before emptying the Trash"),
        p.warn_empty_trash,
        |on| update(|p| p.warn_empty_trash = on),
    ));
    g.append(&check(
        &tr("Ask before deleting items immediately"),
        p.confirm_delete,
        |on| update(|p| p.confirm_delete = on),
    ));
    page.append(&g);
    page
}

fn tab(icon: &str, label: &str) -> (gtk::ToggleButton, gtk::Box) {
    let inner = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(2)
        .build();
    inner.append(&gtk::Image::from_icon_name(icon));
    inner.append(&gtk::Label::new(Some(label)));
    let button = gtk::ToggleButton::builder()
        .child(&inner)
        .css_classes(["files-settings-tab"])
        .build();
    (button, inner)
}

/// Show the Settings window, making it the first time.
pub fn present(parent: Option<&gtk::Window>) {
    if let Some(w) = WINDOW.with(|w| w.borrow().upgrade()) {
        w.present();
        return;
    }
    let window = gtk::Window::builder()
        .title(tr("Files Settings"))
        .default_width(440)
        .resizable(false)
        .css_classes(["background", "files-settings"])
        .build();
    if let Some(app) = parent.and_then(|p| p.application()) {
        window.set_application(Some(&app));
    }

    let stack = gtk::Stack::builder()
        .transition_type(gtk::StackTransitionType::Crossfade)
        .vhomogeneous(false)
        .interpolate_size(true)
        .build();
    let on_hidden: std::rc::Rc<dyn Fn(bool)> = std::rc::Rc::new(crate::app::set_show_hidden);
    stack.add_named(&general_page(on_hidden), Some("general"));
    stack.add_named(&sidebar_page(), Some("sidebar"));
    stack.add_named(&advanced_page(), Some("advanced"));

    let tabs = gtk::Box::builder()
        .spacing(2)
        .css_classes(["files-settings-tabs"])
        .build();
    let mut first: Option<gtk::ToggleButton> = None;
    for (id, icon, label) in [
        ("general", "preferences-system-symbolic", tr("General")),
        ("sidebar", "sidebar-show-symbolic", tr("Sidebar")),
        (
            "advanced",
            "applications-engineering-symbolic",
            tr("Advanced"),
        ),
    ] {
        let (button, _) = tab(icon, &label);
        if let Some(f) = &first {
            button.set_group(Some(f));
        } else {
            button.set_active(true);
            first = Some(button.clone());
        }
        let (stack, window, label) = (stack.clone(), window.clone(), label.clone());
        button.connect_toggled(move |b| {
            if b.is_active() {
                stack.set_visible_child_name(id);
                window.set_title(Some(&label));
            }
        });
        tabs.append(&button);
    }
    window.set_title(Some(&tr("General")));
    let header = gtk::HeaderBar::builder()
        .title_widget(&tabs)
        .css_classes(["files-settings-bar"])
        .build();
    window.set_titlebar(Some(&header));
    // Taller than the screen never: scroll inside instead.
    window.set_child(Some(
        &gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_height(true)
            .max_content_height(440)
            .child(&stack)
            .build(),
    ));
    let keys = gtk::ShortcutController::new();
    keys.add_shortcut(gtk::Shortcut::new(
        gtk::ShortcutTrigger::parse_string("<Control>w|Escape"),
        Some(gtk::CallbackAction::new(|w, _| {
            if let Some(w) = w.downcast_ref::<gtk::Window>() {
                w.close();
            }
            glib::Propagation::Stop
        })),
    ));
    window.add_controller(keys);
    WINDOW.with(|w| w.borrow().set(Some(&window)));
    window.present();
}
