//! The sidebar: Recents, Favorites, and Locations (home, mounted drives,
//! Network, Trash). Shared and LocalSend come in later phases. Mounted
//! drives follow GIO's volume monitor. Folders, drives and the Trash take
//! dropped files.
//!
//! Favorites are the person's, as in Finder (`favorites` in the core keeps
//! them): a folder dropped between two favorites is added there, with a
//! line showing where; dropped on a favorite it goes into that folder. A
//! favorite drags to a new place, and dragged out of the window and let go
//! it is removed, like an icon from the dock. Every window shows the same
//! list, and a change in one shows in all of them.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use elavo_files_core::eapp;
use elavo_files_core::favorites::{self, Favorite};
use elavo_files_core::places::{self, Place};
use gtk::prelude::*;
use gtk::{gdk, gio, glib};

use crate::i18n::tr;
use crate::views::{self, Cells};

type OpenFn = Rc<dyn Fn(gio::File)>;
type SidebarFn = Box<dyn Fn(&Rc<Sidebar>)>;

/// What a favorite being dragged offers: only Files knows it, so another
/// app takes nothing and letting go over it removes the favorite.
const FAVORITE_MIME: &str = "application/x-elavo-favorite";
/// How long the removal takes to fade and close up.
const POOF_MS: u32 = 220;

pub struct Sidebar {
    pub widget: gtk::ScrolledWindow,
    list: gtk::ListBox,
    monitor: gio::VolumeMonitor,
    cells: Rc<Cells>,
    open: OpenFn,
    favs: RefCell<Vec<Favorite>>,
    /// The Favorites rows, in order, and the headers around them.
    rows: RefCell<Vec<gtk::ListBoxRow>>,
    header: RefCell<Option<gtk::ListBoxRow>>,
    after: RefCell<Option<gtk::ListBoxRow>>,
    /// The row showing the insertion line or the drop-into highlight.
    marked: RefCell<Option<(gtk::ListBoxRow, &'static str)>>,
    /// Which drag the cached folder check is for, and whether it had one.
    folders_in: RefCell<(Vec<String>, bool)>,
    file_monitor: RefCell<Option<gio::FileMonitor>>,
    menu: RefCell<Option<gtk::PopoverMenu>>,
    /// The favorite the menu was opened on.
    menu_uri: RefCell<String>,
    watching_window: Cell<bool>,
    actions: gio::SimpleActionGroup,
    me: RefCell<Weak<Sidebar>>,
}

/// A favorite being dragged, in whichever window.
struct Dragged {
    uri: String,
    row: gtk::ListBoxRow,
    pill: gtk::Box,
    sidebar: Weak<Sidebar>,
}

thread_local! {
    static SIDEBARS: RefCell<Vec<Weak<Sidebar>>> = const { RefCell::new(Vec::new()) };
    static DRAGGED: RefCell<Option<Dragged>> = const { RefCell::new(None) };
    /// Whether the pointer is over one of Files' windows during that drag.
    static INSIDE: Cell<bool> = const { Cell::new(true) };
    static LEAVING: RefCell<Option<glib::SourceId>> = const { RefCell::new(None) };
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Zone {
    /// Between favorites: add or move one to this index.
    Insert(usize),
    /// On the favorite at this index: into the folder.
    Into(usize),
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

/// The places a favorite can be, for their names and icons.
fn known_places() -> Vec<Place> {
    let mut all = vec![
        places::recents(),
        places::home(),
        places::network(),
        places::trash(),
    ];
    all.extend(places::favorites());
    all
}

/// The title the toolbar shows for a place in the sidebar, if `uri` is one.
pub fn place_title(uri: &str) -> Option<String> {
    known_places()
        .iter()
        .find(|p| same_uri(&p.uri, uri))
        .map(|p| label(p.id).0)
}

pub fn same_uri(a: &str, b: &str) -> bool {
    favorites::same_uri(a, b)
}

/// A favorite's name and icon: a standard place's own, else the bookmark's
/// name or the folder's.
fn favorite_label(fav: &Favorite) -> (String, &'static str) {
    if let Some(p) = known_places().iter().find(|p| same_uri(&p.uri, &fav.uri)) {
        let (name, icon) = label(p.id);
        return (fav.name.clone().unwrap_or(name), icon);
    }
    let name = fav.name.clone().unwrap_or_else(|| {
        let file = gio::File::for_uri(&fav.uri);
        file.basename()
            .map(|b| glib::filename_display_name(b).to_string())
            .filter(|b| !b.is_empty() && b != "/")
            .unwrap_or_else(|| file.parse_name().to_string())
    });
    (name, "folder-symbolic")
}

/// A folder the sidebar can keep: not an application bundle.
fn is_folder(file: &gio::File) -> bool {
    if let Some(path) = file.path() {
        return path.is_dir() && !eapp::is_bundle(&path) && !eapp::is_mac_app(&path);
    }
    file.query_file_type(gio::FileQueryInfoFlags::NONE, gio::Cancellable::NONE)
        == gio::FileType::Directory
}

/// Save the list and show it in every window.
fn store(list: Vec<Favorite>) {
    if let Err(err) = favorites::save(&list) {
        eprintln!("elavo-files: saving favorites: {err}");
    }
    SIDEBARS.with(|all| {
        all.borrow_mut().retain(|w| w.strong_count() > 0);
        for s in all.borrow().iter().filter_map(Weak::upgrade) {
            s.show_favorites(list.clone());
        }
    });
}

fn current() -> Vec<Favorite> {
    favorites::read().unwrap_or_else(favorites::load)
}

/// Add to Sidebar: the folders that aren't favorites yet, at the end.
pub fn add_folders(files: &[gio::File]) {
    let mut list = current();
    let mut added = 0;
    for f in files.iter().filter(|f| is_folder(f)) {
        let uri = f.uri();
        if !list.iter().any(|x| same_uri(&x.uri, &uri)) {
            let at = list.len();
            favorites::insert(&mut list, Favorite::new(&uri), at);
            added += 1;
        }
    }
    if added > 0 {
        store(list);
    }
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
        gdk::FileList::static_type(),
        gdk::DragAction::COPY | gdk::DragAction::MOVE,
    );
    drop.set_preload(true);
    let dest = gio::File::for_uri(uri);
    let (c, d) = (cells.clone(), dest.clone());
    drop.connect_motion(move |t, _, _| {
        let Some(h) = c.handler() else {
            return gdk::DragAction::empty();
        };
        let files = t.value().map(|v| views::files_of(&v)).unwrap_or_default();
        let offered = views::offered(t);
        h.drop_action(&d, &files, views::force_copy(t), offered)
    });
    let c = cells.clone();
    drop.connect_drop(move |t, value, _, _| drop_into(&c, t, value, &dest));
    row.add_controller(drop);
}

fn drop_into(
    cells: &Rc<Cells>,
    t: &gtk::DropTarget,
    value: &glib::Value,
    dest: &gio::File,
) -> bool {
    let Some(h) = cells.handler() else {
        return false;
    };
    let files: Vec<gio::File> = views::files_of(value)
        .into_iter()
        .filter(|f| !f.equal(dest))
        .collect();
    if files.is_empty() {
        return false;
    }
    let offered = views::offered(t);
    let action = h.drop_action(dest, &files, views::force_copy(t), offered);
    if action.is_empty() {
        return false;
    }
    h.drop_files(dest, files, action);
    true
}

fn content(name: &str, icon: &gio::Icon) -> gtk::Box {
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
    content
}

fn item(name: &str, icon: &gio::Icon, uri: &str) -> gtk::ListBoxRow {
    let row = gtk::ListBoxRow::builder()
        .child(&content(name, icon))
        .build();
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

/// What dragging a favorite shows: its icon and name, and the removal
/// hint once outside the window.
fn drag_pill(name: &str, icon: &str) -> gtk::Box {
    let pill = content(name, &gio::ThemedIcon::new(icon).upcast());
    pill.add_css_class("files-fav-drag");
    let poof = gtk::Box::builder()
        .spacing(4)
        .css_classes(["files-fav-poof-hint"])
        .visible(false)
        .build();
    poof.append(&gtk::Image::from_icon_name("list-remove-symbolic"));
    poof.append(&gtk::Label::new(Some(&tr("Remove"))));
    pill.append(&poof);
    pill
}

/// Show or hide the removal hint for the favorite being dragged.
fn hint(outside: bool) {
    DRAGGED.with(|d| {
        let Some(d) = d.borrow().as_ref().map(|d| (d.pill.clone(), d.row.clone())) else {
            return;
        };
        let (pill, row) = d;
        if let Some(poof) = pill.last_child() {
            poof.set_visible(outside);
        }
        if outside {
            pill.add_css_class("files-fav-removing");
            row.add_css_class("files-fav-leaving");
        } else {
            pill.remove_css_class("files-fav-removing");
            row.remove_css_class("files-fav-leaving");
        }
    });
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
        let o = open.clone();
        list.connect_row_activated(move |_, row| {
            let uri = row.widget_name();
            if !uri.is_empty() {
                o(gio::File::for_uri(&uri));
            }
        });
        let this = Rc::new(Self {
            widget,
            list,
            monitor: gio::VolumeMonitor::get(),
            cells,
            open,
            favs: RefCell::new(favorites::load()),
            rows: RefCell::default(),
            header: RefCell::default(),
            after: RefCell::default(),
            marked: RefCell::default(),
            folders_in: RefCell::default(),
            file_monitor: RefCell::default(),
            menu: RefCell::default(),
            menu_uri: RefCell::default(),
            watching_window: Cell::new(false),
            actions: gio::SimpleActionGroup::new(),
            me: RefCell::default(),
        });
        this.me.replace(Rc::downgrade(&this));
        SIDEBARS.with(|all| all.borrow_mut().push(Rc::downgrade(&this)));
        this.rebuild();
        let weak = Rc::downgrade(&this);
        let refresh = move || {
            if let Some(s) = weak.upgrade() {
                s.refresh();
            }
        };
        let r = refresh.clone();
        this.monitor.connect_mount_added(move |_, _| r());
        let r = refresh.clone();
        this.monitor.connect_mount_removed(move |_, _| r());
        this.monitor.connect_mount_changed(move |_, _| refresh());
        this.add_actions();
        this.take_favorite_drops();
        this.watch_file();
        let weak = Rc::downgrade(&this);
        this.widget.connect_realize(move |_| {
            if let Some(s) = weak.upgrade() {
                s.watch_window();
            }
        });
        this
    }

    /// Rebuild, keeping the highlighted row.
    fn refresh(&self) {
        let selected = self
            .list
            .selected_row()
            .map(|r| r.widget_name().to_string());
        self.rebuild();
        match selected {
            Some(uri) => self.select_uri(&uri),
            // A rebuilt list may pick its first row on focus.
            None => self.list.unselect_all(),
        }
    }

    fn show_favorites(&self, list: Vec<Favorite>) {
        if *self.favs.borrow() == list {
            return;
        }
        self.favs.replace(list);
        self.refresh();
    }

    /// Another Files (another process) changed the list.
    fn watch_file(self: &Rc<Self>) {
        let file = gio::File::for_path(favorites::path());
        let Ok(monitor) =
            file.monitor_file(gio::FileMonitorFlags::WATCH_MOVES, gio::Cancellable::NONE)
        else {
            return;
        };
        let weak = Rc::downgrade(self);
        monitor.connect_changed(move |_, _, _, _| {
            if let (Some(s), Some(list)) = (weak.upgrade(), favorites::read()) {
                s.show_favorites(list);
            }
        });
        self.file_monitor.replace(Some(monitor));
    }

    /// Whether a favorite being dragged is over a Files window, from every
    /// drag passing over this one. Leaving is noted a moment later: GTK
    /// may report a leave as the drop happens inside.
    fn watch_window(&self) {
        if self.watching_window.replace(true) {
            return;
        }
        let Some(root) = self.widget.root() else {
            self.watching_window.set(false);
            return;
        };
        let motion = gtk::DropControllerMotion::new();
        motion.connect_enter(|_, _, _| {
            if let Some(id) = LEAVING.with(|l| l.borrow_mut().take()) {
                id.remove();
            }
            INSIDE.with(|i| i.set(true));
            hint(false);
        });
        motion.connect_leave(|_| {
            let id = glib::timeout_add_local_once(std::time::Duration::from_millis(80), || {
                LEAVING.with(|l| l.borrow_mut().take());
                if DRAGGED.with(|d| d.borrow().is_some()) {
                    INSIDE.with(|i| i.set(false));
                    hint(true);
                }
            });
            if let Some(old) = LEAVING.with(|l| l.borrow_mut().replace(id)) {
                old.remove();
            }
        });
        root.add_controller(motion);
        // Without a file monitor (inotify can run out), catch up with another
        // Files' changes when this window comes to the front.
        if let Some(window) = root.downcast_ref::<gtk::Window>() {
            let weak = self.me.borrow().clone();
            window.connect_is_active_notify(move |w| {
                if let (true, Some(s), Some(list)) =
                    (w.is_active(), weak.upgrade(), favorites::read())
                {
                    s.show_favorites(list);
                }
            });
        }
    }

    fn rebuild(&self) {
        self.unmark();
        while let Some(child) = self.list.first_child() {
            self.list.remove(&child);
        }
        self.list
            .append(&place_row(&places::recents(), &self.cells));

        let header = section(&tr("Favorites"));
        self.menu_on(&header, None);
        self.list.append(&header);
        let rows: Vec<gtk::ListBoxRow> = self
            .favs
            .borrow()
            .iter()
            .map(|f| self.favorite_row(f))
            .collect();
        for row in &rows {
            self.list.append(row);
        }
        self.rows.replace(rows);
        self.header.replace(Some(header));

        let after = section(&tr("Locations"));
        self.list.append(&after);
        self.after.replace(Some(after));
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

    /// A favorite: inside a revealer, so removing it can close up.
    fn favorite_row(&self, fav: &Favorite) -> gtk::ListBoxRow {
        let (name, icon) = favorite_label(fav);
        let revealer = gtk::Revealer::builder()
            .child(&content(&name, &gio::ThemedIcon::new(icon).upcast()))
            .reveal_child(true)
            .transition_type(gtk::RevealerTransitionType::SlideUp)
            .transition_duration(POOF_MS)
            .build();
        let row = gtk::ListBoxRow::builder()
            .child(&revealer)
            .css_classes(["files-favorite"])
            .build();
        row.set_widget_name(&fav.uri);
        row.set_tooltip_text(Some(&gio::File::for_uri(&fav.uri).parse_name()));
        self.menu_on(&row, Some(&fav.uri));

        let drag = gtk::DragSource::builder()
            .actions(gdk::DragAction::MOVE)
            .build();
        let uri = fav.uri.clone();
        drag.connect_prepare(move |_, _, _| {
            Some(gdk::ContentProvider::for_bytes(
                FAVORITE_MIME,
                &glib::Bytes::from(uri.as_bytes()),
            ))
        });
        let (uri, weak_row, me) = (fav.uri.clone(), row.downgrade(), self.me.borrow().clone());
        drag.connect_drag_begin(move |_, drag| {
            let Some(row) = weak_row.upgrade() else {
                return;
            };
            let pill = drag_pill(&name, icon);
            if let Ok(icon) = gtk::DragIcon::for_drag(drag).downcast::<gtk::DragIcon>() {
                icon.set_child(Some(&pill));
            }
            drag.set_hotspot(12, 12);
            row.add_css_class("files-fav-dragging");
            INSIDE.with(|i| i.set(true));
            DRAGGED.with(|d| {
                d.replace(Some(Dragged {
                    uri: uri.clone(),
                    row,
                    pill,
                    sidebar: me.clone(),
                }))
            });
        });
        drag.connect_drag_cancel(|_, _, reason| {
            let outside = !INSIDE.with(Cell::get);
            let done = DRAGGED.with(|d| d.borrow_mut().take());
            let Some(d) = done else {
                return false;
            };
            d.row.remove_css_class("files-fav-dragging");
            d.row.remove_css_class("files-fav-leaving");
            // Let go where nothing takes it (Wayland reports that as an
            // error); Escape keeps it.
            if outside && reason != gdk::DragCancelReason::UserCancelled {
                if let Some(s) = d.sidebar.upgrade() {
                    s.poof(&d.uri);
                }
                // No sliding back: it is gone.
                return true;
            }
            false
        });
        drag.connect_drag_end(|_, _, _| {
            if let Some(d) = DRAGGED.with(|d| d.borrow_mut().take()) {
                d.row.remove_css_class("files-fav-dragging");
                d.row.remove_css_class("files-fav-leaving");
            }
            INSIDE.with(|i| i.set(true));
        });
        row.add_controller(drag);
        row
    }

    /// Remove a favorite with a short fade, the folder untouched.
    fn poof(&self, uri: &str) {
        let row = self
            .rows
            .borrow()
            .iter()
            .find(|r| same_uri(&r.widget_name(), uri))
            .cloned();
        let uri = uri.to_owned();
        let forget = move || {
            let mut list = current();
            if favorites::remove(&mut list, &uri) {
                store(list);
            }
        };
        let Some(row) = row else {
            forget();
            return;
        };
        row.add_css_class("files-fav-poof");
        row.set_sensitive(false);
        match row.child().and_downcast::<gtk::Revealer>() {
            Some(revealer) => {
                let forget = RefCell::new(Some(forget));
                revealer.connect_child_revealed_notify(move |r| {
                    if !r.is_child_revealed() {
                        if let Some(f) = forget.borrow_mut().take() {
                            f();
                        }
                    }
                });
                revealer.set_reveal_child(false);
            }
            None => forget(),
        }
    }

    // Dropping on Favorites

    /// Where a drop at `y` (in the list) lands. Over a favorite, the top and
    /// bottom quarters are the gaps around it and the middle the folder
    /// itself, when `into` allows it.
    fn zone(&self, y: f64, into: bool) -> Option<Zone> {
        let row = self.list.row_at_y(y as i32)?;
        let rows = self.rows.borrow();
        if self.header.borrow().as_ref() == Some(&row) {
            return Some(Zone::Insert(0));
        }
        if self.after.borrow().as_ref() == Some(&row) {
            return Some(Zone::Insert(rows.len()));
        }
        let i = rows.iter().position(|r| r == &row)?;
        let b = row.compute_bounds(&self.list)?;
        let at = ((y - b.y() as f64) / (b.height() as f64).max(1.0)).clamp(0.0, 1.0);
        Some(match (into, at) {
            (true, a) if a < 0.25 => Zone::Insert(i),
            (true, a) if a > 0.75 => Zone::Insert(i + 1),
            (true, _) => Zone::Into(i),
            (false, a) if a < 0.5 => Zone::Insert(i),
            (false, _) => Zone::Insert(i + 1),
        })
    }

    fn mark(&self, zone: Option<Zone>) {
        let rows = self.rows.borrow();
        let want = match zone {
            Some(Zone::Insert(i)) if i < rows.len() => {
                Some((rows[i].clone(), "files-fav-line-before"))
            }
            Some(Zone::Insert(_)) => match rows.last() {
                Some(last) => Some((last.clone(), "files-fav-line-after")),
                None => self
                    .header
                    .borrow()
                    .clone()
                    .map(|h| (h, "files-fav-line-after")),
            },
            Some(Zone::Into(i)) => rows.get(i).map(|r| (r.clone(), "files-fav-drop-into")),
            None => None,
        };
        drop(rows);
        if *self.marked.borrow() == want {
            return;
        }
        self.unmark();
        if let Some((row, class)) = &want {
            row.add_css_class(class);
        }
        self.marked.replace(want);
    }

    fn unmark(&self) {
        if let Some((row, class)) = self.marked.take() {
            row.remove_css_class(class);
        }
    }

    /// Whether the files being dragged include a folder, checked once per
    /// drag.
    fn has_folder(&self, files: &[gio::File]) -> bool {
        let uris: Vec<String> = files.iter().map(|f| f.uri().to_string()).collect();
        let mut cache = self.folders_in.borrow_mut();
        if cache.0 != uris {
            *cache = (uris, files.iter().any(is_folder));
        }
        cache.1
    }

    /// The action for adding: nothing happens to the files, so the
    /// gentlest one offered.
    fn add_action(offered: gdk::DragAction) -> gdk::DragAction {
        [
            gdk::DragAction::LINK,
            gdk::DragAction::COPY,
            gdk::DragAction::MOVE,
        ]
        .into_iter()
        .find(|a| offered.contains(*a))
        .unwrap_or(gdk::DragAction::empty())
    }

    fn take_favorite_drops(self: &Rc<Self>) {
        // Files and folders: between favorites adds the folders, on one
        // moves or copies into it.
        let files = gtk::DropTarget::new(
            gdk::FileList::static_type(),
            gdk::DragAction::COPY | gdk::DragAction::MOVE | gdk::DragAction::LINK,
        );
        files.set_preload(true);
        let weak = Rc::downgrade(self);
        files.connect_motion(move |t, _, y| {
            let Some(s) = weak.upgrade() else {
                return gdk::DragAction::empty();
            };
            let files = t.value().map(|v| views::files_of(&v)).unwrap_or_default();
            let folders = s.has_folder(&files);
            let zone = s.zone(y, true);
            let action = match zone {
                Some(Zone::Insert(_)) if folders => Self::add_action(views::offered(t)),
                Some(Zone::Into(i)) => {
                    let dest = s
                        .rows
                        .borrow()
                        .get(i)
                        .map(|r| gio::File::for_uri(&r.widget_name()));
                    match (dest, s.cells.handler()) {
                        (Some(d), Some(h)) => {
                            h.drop_action(&d, &files, views::force_copy(t), views::offered(t))
                        }
                        _ => gdk::DragAction::empty(),
                    }
                }
                _ => gdk::DragAction::empty(),
            };
            s.mark(zone.filter(|_| !action.is_empty()));
            action
        });
        let weak = Rc::downgrade(self);
        files.connect_leave(move |_| {
            if let Some(s) = weak.upgrade() {
                s.unmark();
            }
        });
        let weak = Rc::downgrade(self);
        files.connect_drop(move |t, value, _, y| {
            let Some(s) = weak.upgrade() else {
                return false;
            };
            s.unmark();
            match s.zone(y, true) {
                Some(Zone::Insert(at)) => {
                    let mut list = current();
                    let mut at = at.min(list.len());
                    let mut any = false;
                    for f in views::files_of(value).iter().filter(|f| is_folder(f)) {
                        let uri = f.uri();
                        let from = list.iter().position(|x| same_uri(&x.uri, &uri));
                        favorites::insert(&mut list, Favorite::new(&uri), at);
                        // The next one goes after it.
                        at = match from {
                            Some(i) if i < at => at,
                            _ => at + 1,
                        };
                        any = true;
                    }
                    if any {
                        store(list);
                    }
                    any
                }
                Some(Zone::Into(i)) => {
                    let Some(dest) = s
                        .rows
                        .borrow()
                        .get(i)
                        .map(|r| gio::File::for_uri(&r.widget_name()))
                    else {
                        return false;
                    };
                    drop_into(&s.cells, t, value, &dest)
                }
                None => false,
            }
        });
        self.list.add_controller(files);

        // A favorite: to a new place.
        let favorite = gtk::DropTargetAsync::new(
            Some(gdk::ContentFormats::new(&[FAVORITE_MIME])),
            gdk::DragAction::MOVE,
        );
        favorite.connect_accept(|_, _| DRAGGED.with(|d| d.borrow().is_some()));
        let weak = Rc::downgrade(self);
        let motion = move |_: &gtk::DropTargetAsync, _: &gdk::Drop, _: f64, y: f64| {
            let Some(s) = weak.upgrade() else {
                return gdk::DragAction::empty();
            };
            let zone = s.zone(y, false);
            s.mark(zone);
            if zone.is_some() {
                gdk::DragAction::MOVE
            } else {
                gdk::DragAction::empty()
            }
        };
        favorite.connect_drag_enter(motion.clone());
        favorite.connect_drag_motion(motion);
        let weak = Rc::downgrade(self);
        favorite.connect_drag_leave(move |_, _| {
            if let Some(s) = weak.upgrade() {
                s.unmark();
            }
        });
        let weak = Rc::downgrade(self);
        favorite.connect_drop(move |_, drop, _, y| {
            let Some(s) = weak.upgrade() else {
                return false;
            };
            s.unmark();
            let uri = DRAGGED.with(|d| d.borrow().as_ref().map(|d| d.uri.clone()));
            let (Some(uri), Some(Zone::Insert(at))) = (uri, s.zone(y, false)) else {
                drop.finish(gdk::DragAction::empty());
                return false;
            };
            let mut list = current();
            favorites::insert(&mut list, Favorite::new(&uri), at);
            store(list);
            drop.finish(gdk::DragAction::MOVE);
            true
        });
        self.list.add_controller(favorite);
    }

    // The menu

    fn add_actions(self: &Rc<Self>) {
        let group = &self.actions;
        let add = |name: &str, f: SidebarFn| {
            let action = gio::SimpleAction::new(name, None);
            let weak = Rc::downgrade(self);
            action.connect_activate(move |_, _| {
                if let Some(s) = weak.upgrade() {
                    f(&s);
                }
            });
            group.add_action(&action);
        };
        add(
            "open",
            Box::new(|s| {
                let uri = s.menu_uri.borrow().clone();
                if !uri.is_empty() {
                    (s.open)(gio::File::for_uri(&uri));
                }
            }),
        );
        add(
            "remove",
            Box::new(|s| {
                let uri = s.menu_uri.borrow().clone();
                if !uri.is_empty() {
                    s.poof(&uri);
                }
            }),
        );
        add(
            "restore",
            Box::new(|_| store(favorites::restore(&current()))),
        );
    }

    /// Right click: Open and Remove from Sidebar on a favorite, Restore
    /// Default Favorites on both it and the Favorites heading.
    fn menu_on(&self, row: &gtk::ListBoxRow, uri: Option<&str>) {
        let click = gtk::GestureClick::builder()
            .button(gdk::BUTTON_SECONDARY)
            .build();
        let (weak, uri) = (self.me.borrow().clone(), uri.map(str::to_owned));
        click.connect_pressed(move |g, _, x, y| {
            g.set_state(gtk::EventSequenceState::Claimed);
            let (Some(s), Some(w)) = (weak.upgrade(), g.widget()) else {
                return;
            };
            s.menu_uri.replace(uri.clone().unwrap_or_default());
            let menu = gio::Menu::new();
            if uri.is_some() {
                let first = gio::Menu::new();
                first.append(Some(&tr("Open")), Some("sidebar.open"));
                menu.append_section(None, &first);
                let second = gio::Menu::new();
                second.append(Some(&tr("Remove from Sidebar")), Some("sidebar.remove"));
                menu.append_section(None, &second);
            }
            let last = gio::Menu::new();
            last.append(
                Some(&tr("Restore Default Favorites")),
                Some("sidebar.restore"),
            );
            menu.append_section(None, &last);
            s.show_menu(&menu, &w, x, y);
        });
        row.add_controller(click);
    }

    fn show_menu(&self, model: &gio::Menu, from: &gtk::Widget, x: f64, y: f64) {
        if let Some(old) = self.menu.take() {
            old.popdown();
        }
        // Parented outside the sidebar's scrolled window, which would size
        // the menu by its own height.
        let parent = self
            .widget
            .parent()
            .unwrap_or_else(|| self.list.clone().upcast());
        let Some(point) =
            from.compute_point(&parent, &gtk::graphene::Point::new(x as f32, y as f32))
        else {
            return;
        };
        let popover = gtk::PopoverMenu::from_model_full(model, gtk::PopoverMenuFlags::NESTED);
        popover.add_css_class("files-menu");
        popover.set_has_arrow(false);
        popover.set_halign(gtk::Align::Start);
        popover.set_position(gtk::PositionType::Bottom);
        popover.insert_action_group("sidebar", Some(&self.actions));
        popover.set_parent(&parent);
        popover.set_pointing_to(Some(&gdk::Rectangle::new(
            point.x() as i32,
            point.y() as i32,
            1,
            1,
        )));
        popover.connect_closed(|p| {
            let p = p.clone();
            glib::idle_add_local_once(move || p.unparent());
        });
        popover.popup();
        self.menu.replace(Some(popover));
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
