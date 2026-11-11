//! The Gallery view: the selected file large in the middle, the folder as
//! a filmstrip along the bottom (names show on hover) and its details on
//! the right, with Finder's quick actions under them. Left and Right step
//! through the filmstrip.

use std::cell::RefCell;
use std::rc::Rc;

use elavo_files_core::entry;
use elavo_files_core::sort::Key;
use gtk::prelude::*;
use gtk::{gdk, gio, glib};

use crate::preview::{Preview, Stage};
use crate::views::{self, Cells, View};

const FILM_SIZE: i32 = 56;
const INFO_WIDTH: i32 = 280;

type InfoFn = Box<dyn Fn(&gio::FileInfo)>;

pub struct Gallery {
    pub root: gtk::Box,
    pub sel: gtk::MultiSelection,
    pub strip: gtk::ListView,
    stage: Rc<Stage>,
    pub info: Rc<Preview>,
    on_activate: RefCell<Option<InfoFn>>,
    /// The file shown, to find again when the folder lists it anew (a
    /// rotated image is replaced on disk).
    shown: RefCell<Option<String>>,
}

fn film_factory(cells: &Rc<Cells>) -> gtk::SignalListItemFactory {
    let factory = gtk::SignalListItemFactory::new();
    let c = cells.clone();
    factory.connect_setup(move |_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().expect("list item");
        let image = gtk::Image::builder()
            .pixel_size(FILM_SIZE)
            .css_classes([views::GRAB])
            .build();
        let cell = gtk::Box::builder()
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .css_classes(["files-film-item"])
            .build();
        cell.append(&image);
        views::wire(cell.upcast_ref(), item, &c, View::Gallery, Some(&image));
        item.set_child(Some(&cell));
    });
    let c = cells.clone();
    factory.connect_bind(move |_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().expect("list item");
        let (Some(info), Some(cell)) = (item.item().and_downcast::<gio::FileInfo>(), item.child())
        else {
            return;
        };
        let image = cell
            .first_child()
            .and_downcast::<gtk::Image>()
            .expect("image");
        views::set_icon(&image, &info, FILM_SIZE);
        cell.set_tooltip_text(Some(&entry::name(&info)));
        c.bind(&info, &cell);
    });
    let c = cells.clone();
    factory.connect_unbind(move |_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().expect("list item");
        if let (Some(info), Some(cell)) =
            (item.item().and_downcast::<gio::FileInfo>(), item.child())
        {
            c.unbind(&info, &cell);
        }
    });
    factory
}

impl Gallery {
    pub fn new(base: &gtk::FilterListModel, cells: &Rc<Cells>) -> Rc<Self> {
        let sorted = gtk::SortListModel::new(Some(base.clone()), Some(views::sorter(Key::Name)));
        let sel = gtk::MultiSelection::new(Some(sorted));
        let strip = gtk::ListView::builder()
            .model(&sel)
            .factory(&film_factory(cells))
            .orientation(gtk::Orientation::Horizontal)
            .css_classes(["files-filmstrip"])
            .build();
        let strip_scroll = gtk::ScrolledWindow::builder()
            .vscrollbar_policy(gtk::PolicyType::Never)
            .hscrollbar_policy(gtk::PolicyType::Automatic)
            .height_request(FILM_SIZE + 30)
            .child(&strip)
            .css_classes(["files-filmstrip-scroll"])
            .build();
        let stage = Stage::new(1600);
        stage.root.add_css_class("files-gallery-stage");
        let center = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .hexpand(true)
            .build();
        center.append(&stage.root);
        center.append(&strip_scroll);
        let info = Preview::new(None);
        let info_scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .width_request(INFO_WIDTH)
            // Its labels expand; the panel itself keeps its width.
            .hexpand(false)
            .child(&info.root)
            .css_classes(["files-gallery-info"])
            .build();
        let root = gtk::Box::builder().css_classes(["files-gallery"]).build();
        root.append(&center);
        root.append(&gtk::Separator::new(gtk::Orientation::Vertical));
        root.append(&info_scroll);
        let this = Rc::new(Self {
            root,
            sel,
            strip,
            stage,
            info,
            on_activate: RefCell::default(),
            shown: RefCell::default(),
        });
        this.connect();
        this
    }

    pub fn connect_activate(&self, f: impl Fn(&gio::FileInfo) + 'static) {
        self.on_activate.replace(Some(Box::new(f)));
    }

    fn activate(&self, pos: u32) {
        if let (Some(info), Some(f)) = (
            self.sel.item(pos).and_downcast::<gio::FileInfo>(),
            self.on_activate.borrow().as_ref(),
        ) {
            f(&info);
        }
    }

    fn connect(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.sel.connect_selection_changed(move |_, _, _| {
            if let Some(g) = weak.upgrade() {
                g.show_selection();
            }
        });
        let weak = Rc::downgrade(self);
        self.sel.connect_items_changed(move |_, _, _, _| {
            let weak = weak.clone();
            glib::idle_add_local_once(move || {
                if let Some(g) = weak.upgrade() {
                    if g.root.is_mapped() {
                        g.ensure_selection();
                    }
                }
            });
        });
        let weak = Rc::downgrade(self);
        self.strip.connect_activate(move |_, pos| {
            if let Some(g) = weak.upgrade() {
                g.activate(pos);
            }
        });
        // A double click on the picture opens the file.
        let click = gtk::GestureClick::new();
        let weak = Rc::downgrade(self);
        click.connect_pressed(move |_, n, _, _| {
            let Some(g) = weak.upgrade() else {
                return;
            };
            g.strip.grab_focus();
            if n == 2 {
                if let Some(pos) = g.first_selected() {
                    g.activate(pos);
                }
            }
        });
        self.stage.root.add_controller(click);
        // Left and Right step through the filmstrip wherever the focus is.
        let keys = gtk::EventControllerKey::builder()
            .propagation_phase(gtk::PropagationPhase::Capture)
            .build();
        let weak = Rc::downgrade(self);
        keys.connect_key_pressed(move |_, key, _, state| {
            if state.intersects(gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::ALT_MASK) {
                return glib::Propagation::Proceed;
            }
            let step = match key {
                gdk::Key::Left | gdk::Key::KP_Left => -1,
                gdk::Key::Right | gdk::Key::KP_Right => 1,
                _ => return glib::Propagation::Proceed,
            };
            if let Some(g) = weak.upgrade() {
                g.step(step);
            }
            glib::Propagation::Stop
        });
        self.root.add_controller(keys);
    }

    fn first_selected(&self) -> Option<u32> {
        let set = self.sel.selection();
        (!set.is_empty()).then(|| set.minimum())
    }

    fn step(&self, by: i32) {
        let n = self.sel.n_items();
        if n == 0 {
            return;
        }
        let pos = match self.first_selected() {
            Some(p) => (p as i64 + by as i64).clamp(0, n as i64 - 1) as u32,
            None => 0,
        };
        self.strip.scroll_to(
            pos,
            gtk::ListScrollFlags::FOCUS | gtk::ListScrollFlags::SELECT,
            None,
        );
    }

    /// With nothing selected, the file shown before if it is still there,
    /// or the first item.
    pub fn ensure_selection(&self) {
        if self.first_selected().is_some() || self.sel.n_items() == 0 {
            return;
        }
        let shown = self.shown.borrow().clone();
        let pos = shown
            .and_then(|uri| {
                (0..self.sel.n_items()).find(|&i| {
                    self.sel
                        .item(i)
                        .and_downcast::<gio::FileInfo>()
                        .and_then(|info| entry::target(&info))
                        .is_some_and(|f| f.uri() == uri)
                })
            })
            .unwrap_or(0);
        self.strip.scroll_to(
            pos,
            gtk::ListScrollFlags::FOCUS | gtk::ListScrollFlags::SELECT,
            None,
        );
    }

    /// A new folder: start from its first item.
    pub fn reset(&self) {
        self.shown.replace(None);
    }

    pub fn scroll_to(&self, pos: u32) {
        self.strip.scroll_to(pos, gtk::ListScrollFlags::FOCUS, None);
    }

    fn show_selection(&self) {
        let set = self.sel.selection();
        let count = set.size();
        let first = self
            .first_selected()
            .and_then(|p| self.sel.item(p))
            .and_downcast::<gio::FileInfo>();
        let file = first.as_ref().and_then(entry::target);
        if let Some(f) = &file {
            self.shown.replace(Some(f.uri().to_string()));
        }
        match (count, first, file) {
            (1, Some(info), Some(file)) => {
                self.stage.show(&file, &info);
                self.info.show(&file);
            }
            (n, Some(info), Some(file)) if n > 1 => {
                self.stage.show(&file, &info);
                self.info.show_count(n as usize);
            }
            _ => {
                self.stage.clear();
                self.info.clear();
            }
        }
    }
}
