//! The sidebar: Recents, Favorites, and Locations (home, mounted drives,
//! Network, Trash). Shared and LocalSend come in later phases. Mounted
//! drives follow GIO's volume monitor. Folders, drives and the Trash take
//! dropped files.

use std::rc::Rc;

use elavo_files_core::places::{self, Place};
use gtk::prelude::*;
use gtk::{gio, glib};

use crate::i18n::tr;
use crate::views::{self, Cells};

type OpenFn = Rc<dyn Fn(gio::File)>;

pub struct Sidebar {
    pub widget: gtk::ScrolledWindow,
    list: gtk::ListBox,
    monitor: gio::VolumeMonitor,
    cells: Rc<Cells>,
}

/// The sidebar's name and symbolic icon for a fixed place.
fn label(id: &str) -> (String, &'static str) {
    match id {
        "recents" => (tr("Recents"), "document-open-recent-symbolic"),
        "applications" => (tr("Applications"), "view-app-grid-symbolic"),
        "desktop" => (tr("Desktop"), "user-desktop-symbolic"),
        "documents" => (tr("Documents"), "folder-documents-symbolic"),
        "downloads" => (tr("Downloads"), "folder-download-symbolic"),
        "music" => (tr("Music"), "folder-music-symbolic"),
        "pictures" => (tr("Pictures"), "folder-pictures-symbolic"),
        "videos" => (tr("Videos"), "folder-videos-symbolic"),
        "public" => (tr("Public"), "folder-publicshare-symbolic"),
        "templates" => (tr("Templates"), "folder-templates-symbolic"),
        "home" => (
            glib::user_name().to_string_lossy().into_owned(),
            "user-home-symbolic",
        ),
        "network" => (tr("Network"), "network-workgroup-symbolic"),
        "trash" => (tr("Trash"), "user-trash-symbolic"),
        _ => (id.to_owned(), "folder-symbolic"),
    }
}

/// The title the toolbar shows for a place in the sidebar, if `uri` is one.
pub fn place_title(uri: &str) -> Option<String> {
    let mut all = vec![
        places::recents(),
        places::home(),
        places::network(),
        places::trash(),
    ];
    all.extend(places::favorites());
    all.iter()
        .find(|p| same_uri(&p.uri, uri))
        .map(|p| label(p.id).0)
}

pub fn same_uri(a: &str, b: &str) -> bool {
    a.trim_end_matches('/') == b.trim_end_matches('/')
}

fn section(title: &str) -> gtk::ListBoxRow {
    let row = gtk::ListBoxRow::builder()
        .activatable(false)
        .selectable(false)
        .css_classes(["files-section"])
        .child(
            &gtk::Label::builder()
                .label(title)
                .xalign(0.0)
                .css_classes(["files-section-title"])
                .build(),
        )
        .build();
    row.set_focusable(false);
    row
}

/// Files dropped on a row go into it; Recents and Network take none.
fn take_drops(row: &gtk::ListBoxRow, uri: &str, cells: &Rc<Cells>) {
    if uri.starts_with("recent:") || uri.starts_with("network:") {
        return;
    }
    let drop = gtk::DropTarget::new(
        gtk::gdk::FileList::static_type(),
        gtk::gdk::DragAction::COPY | gtk::gdk::DragAction::MOVE,
    );
    drop.set_preload(true);
    let dest = gio::File::for_uri(uri);
    let (c, d) = (cells.clone(), dest.clone());
    drop.connect_motion(move |t, _, _| {
        let Some(h) = c.handler() else {
            return gtk::gdk::DragAction::empty();
        };
        let files = t.value().map(|v| views::files_of(&v)).unwrap_or_default();
        let offered = views::offered(t);
        h.drop_action(&d, &files, views::force_copy(t), offered)
    });
    let c = cells.clone();
    drop.connect_drop(move |t, value, _, _| {
        let Some(h) = c.handler() else {
            return false;
        };
        let files: Vec<gio::File> = views::files_of(value)
            .into_iter()
            .filter(|f| !f.equal(&dest))
            .collect();
        if files.is_empty() {
            return false;
        }
        let offered = views::offered(t);
        let action = h.drop_action(&dest, &files, views::force_copy(t), offered);
        if action.is_empty() {
            return false;
        }
        h.drop_files(&dest, files, action);
        true
    });
    row.add_controller(drop);
}

fn item(name: &str, icon: &gio::Icon, uri: &str) -> gtk::ListBoxRow {
    let content = gtk::Box::builder().spacing(8).build();
    content.append(
        &gtk::Image::builder()
            .gicon(icon)
            .pixel_size(16)
            .css_classes(["files-place-icon"])
            .build(),
    );
    content.append(
        &gtk::Label::builder()
            .label(name)
            .xalign(0.0)
            .hexpand(true)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build(),
    );
    let row = gtk::ListBoxRow::builder().child(&content).build();
    row.set_widget_name(uri);
    row.set_tooltip_text(Some(name));
    row
}

fn place_row(place: &Place, cells: &Rc<Cells>) -> gtk::ListBoxRow {
    let (name, icon) = label(place.id);
    let row = item(&name, &gio::ThemedIcon::new(icon).upcast(), &place.uri);
    take_drops(&row, &place.uri, cells);
    row
}

impl Sidebar {
    pub fn new(open: OpenFn, cells: Rc<Cells>) -> Rc<Self> {
        let list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::Single)
            .css_classes(["navigation-sidebar"])
            .build();
        let widget = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&list)
            .css_classes(["files-sidebar"])
            .build();
        list.connect_row_activated(move |_, row| {
            let uri = row.widget_name();
            if !uri.is_empty() {
                open(gio::File::for_uri(&uri));
            }
        });
        let this = Rc::new(Self {
            widget,
            list,
            monitor: gio::VolumeMonitor::get(),
            cells,
        });
        this.rebuild();
        let weak = Rc::downgrade(&this);
        let refresh = move || {
            if let Some(s) = weak.upgrade() {
                let selected = s.list.selected_row().map(|r| r.widget_name().to_string());
                s.rebuild();
                if let Some(uri) = selected {
                    s.select_uri(&uri);
                }
            }
        };
        let r = refresh.clone();
        this.monitor.connect_mount_added(move |_, _| r());
        let r = refresh.clone();
        this.monitor.connect_mount_removed(move |_, _| r());
        this.monitor.connect_mount_changed(move |_, _| refresh());
        this
    }

    fn rebuild(&self) {
        while let Some(child) = self.list.first_child() {
            self.list.remove(&child);
        }
        self.list
            .append(&place_row(&places::recents(), &self.cells));

        self.list.append(&section(&tr("Favorites")));
        for place in places::favorites().iter().filter(|p| p.shown) {
            self.list.append(&place_row(place, &self.cells));
        }

        self.list.append(&section(&tr("Locations")));
        self.list.append(&place_row(&places::home(), &self.cells));
        for mount in self.monitor.mounts() {
            if mount.is_shadowed() {
                continue;
            }
            let uri = mount.root().uri().to_string();
            let row = item(&mount.name(), &mount.symbolic_icon(), &uri);
            take_drops(&row, &uri, &self.cells);
            self.list.append(&row);
        }
        self.list
            .append(&place_row(&places::network(), &self.cells));
        self.list.append(&place_row(&places::trash(), &self.cells));
    }

    /// Highlight the row for `uri`, or none when it is not in the sidebar.
    pub fn select_uri(&self, uri: &str) {
        let mut child = self.list.first_child();
        while let Some(widget) = child {
            if let Ok(row) = widget.clone().downcast::<gtk::ListBoxRow>() {
                if row.is_selectable() && same_uri(&row.widget_name(), uri) {
                    self.list.select_row(Some(&row));
                    return;
                }
            }
            child = widget.next_sibling();
        }
        self.list.unselect_all();
    }

    /// The name of the mount `uri` is the root of, for the toolbar title.
    pub fn mount_title(&self, uri: &str) -> Option<String> {
        self.monitor
            .mounts()
            .into_iter()
            .find(|m| same_uri(&m.root().uri(), uri))
            .map(|m| m.name().to_string())
    }
}
