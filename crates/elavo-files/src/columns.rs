//! The Columns view: Finder's Miller columns. The first column is the
//! folder the window shows; selecting a folder opens it in a column to the
//! right, selecting a file shows its preview there instead. The columns
//! scroll sideways, each can be made wider or narrower by its right edge,
//! and Left and Right move between them.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use elavo_files_core::sort::Key;
use elavo_files_core::{eapp, entry};
use gtk::prelude::*;
use gtk::{gdk, gio, glib};

use crate::preview::Preview;
use crate::views::{self, Cells, View, GRAB};

const DEFAULT_WIDTH: i32 = 210;
const MIN_WIDTH: i32 = 140;
const MAX_WIDTH: i32 = 600;
const PREVIEW_WIDTH: i32 = 300;

pub struct Column {
    folder: RefCell<gio::File>,
    pub sel: gtk::MultiSelection,
    pub list: gtk::ListView,
    frame: gtk::Box,
    scroll: gtk::ScrolledWindow,
    /// What the column after this one shows, a folder or a file's preview.
    next: RefCell<Option<String>>,
}

type InfoFn = Box<dyn Fn(&gio::FileInfo)>;
/// Something to do with a column, its place and the view.
type Visit<'a> = dyn Fn(&Rc<Columns>, &Rc<Column>, usize) + 'a;
type WidgetFn = Box<dyn Fn(&gtk::Widget, f64, f64)>;

pub struct Columns {
    pub root: gtk::ScrolledWindow,
    strip: gtk::Box,
    columns: RefCell<Vec<Rc<Column>>>,
    pub preview: Rc<Preview>,
    preview_frame: gtk::Box,
    active: Cell<usize>,
    cells: Rc<Cells>,
    filter: gtk::CustomFilter,
    /// The width the last resized column was given; new columns take it.
    width: Cell<i32>,
    on_change: RefCell<Option<Box<dyn Fn()>>>,
    on_items: RefCell<Option<Box<dyn Fn()>>>,
    on_activate: RefCell<Option<InfoFn>>,
    on_background: RefCell<Option<WidgetFn>>,
}

fn is_folder(info: &gio::FileInfo) -> bool {
    entry::is_dir(info) && !eapp::is_package(info)
}

/// Whether `(x, y)` in `list` is on one of its rows.
fn on_row(list: &gtk::Widget, x: f64, y: f64) -> bool {
    let mut w = list.pick(x, y, gtk::PickFlags::DEFAULT);
    while let Some(x) = w {
        if &x == list {
            return false;
        }
        if x.css_name() == "row" || x.has_css_class(GRAB) {
            return true;
        }
        w = x.parent();
    }
    false
}

fn row_factory(cells: &Rc<Cells>) -> gtk::SignalListItemFactory {
    let factory = gtk::SignalListItemFactory::new();
    let c = cells.clone();
    factory.connect_setup(move |_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().expect("list item");
        let image = gtk::Image::builder()
            .pixel_size(16)
            .css_classes([GRAB])
            .build();
        let name = gtk::Label::builder()
            .xalign(0.0)
            .hexpand(true)
            .ellipsize(gtk::pango::EllipsizeMode::Middle)
            .css_classes([GRAB])
            .build();
        let chevron = gtk::Image::builder()
            .icon_name("go-next-symbolic")
            .pixel_size(12)
            .css_classes(["files-column-chevron"])
            .build();
        let row = gtk::Box::builder()
            .spacing(6)
            .css_classes(["files-column-row"])
            .build();
        row.append(&image);
        row.append(&name);
        row.append(&chevron);
        views::wire(row.upcast_ref(), item, &c, View::Columns, Some(&image));
        item.set_child(Some(&row));
    });
    let c = cells.clone();
    factory.connect_bind(move |_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().expect("list item");
        let (Some(info), Some(row)) = (item.item().and_downcast::<gio::FileInfo>(), item.child())
        else {
            return;
        };
        let image = row
            .first_child()
            .and_downcast::<gtk::Image>()
            .expect("image");
        let name = image
            .next_sibling()
            .and_downcast::<gtk::Label>()
            .expect("name");
        let chevron = name.next_sibling().expect("chevron");
        views::set_icon(&image, &info, 16);
        name.set_label(&entry::name(&info));
        chevron.set_opacity(if is_folder(&info) { 1.0 } else { 0.0 });
        c.bind(&info, &row);
    });
    let c = cells.clone();
    factory.connect_unbind(move |_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().expect("list item");
        if let (Some(info), Some(row)) = (item.item().and_downcast::<gio::FileInfo>(), item.child())
        {
            c.unbind(&info, &row);
        }
    });
    factory
}

fn selected_infos(sel: &gtk::MultiSelection) -> Vec<(u32, gio::FileInfo)> {
    let set = sel.selection();
    (0..set.size())
        .filter_map(|i| {
            let pos = set.nth(i as u32);
            sel.item(pos)
                .and_downcast::<gio::FileInfo>()
                .map(|info| (pos, info))
        })
        .collect()
}

impl Columns {
    /// `base` is the window's folder, filtered; `filter` is the same filter
    /// (hidden files, search) for the folders opened to the right.
    pub fn new(
        base: &gtk::FilterListModel,
        filter: &gtk::CustomFilter,
        folder: &gio::File,
        cells: &Rc<Cells>,
    ) -> Rc<Self> {
        let strip = gtk::Box::builder().css_classes(["files-columns"]).build();
        let preview = Preview::new(Some(240));
        let preview_frame = gtk::Box::builder()
            .width_request(PREVIEW_WIDTH)
            .hexpand(true)
            .visible(false)
            .css_classes(["files-columns-preview"])
            .build();
        let preview_scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .hexpand(true)
            .child(&preview.root)
            .build();
        preview_frame.append(&preview_scroll);
        strip.append(&preview_frame);
        let root = gtk::ScrolledWindow::builder()
            .vscrollbar_policy(gtk::PolicyType::Never)
            .hexpand(true)
            .vexpand(true)
            .child(&strip)
            .css_classes(["files-scroll"])
            .build();
        let this = Rc::new(Self {
            root,
            strip,
            columns: RefCell::default(),
            preview,
            preview_frame,
            active: Cell::new(0),
            cells: cells.clone(),
            filter: filter.clone(),
            width: Cell::new(DEFAULT_WIDTH),
            on_change: RefCell::default(),
            on_items: RefCell::default(),
            on_activate: RefCell::default(),
            on_background: RefCell::default(),
        });
        let first = this.make_column(folder, base);
        this.columns.borrow_mut().push(first);
        this
    }

    pub fn connect_change(&self, f: impl Fn() + 'static) {
        self.on_change.replace(Some(Box::new(f)));
    }

    pub fn connect_items(&self, f: impl Fn() + 'static) {
        self.on_items.replace(Some(Box::new(f)));
    }

    pub fn connect_activate(&self, f: impl Fn(&gio::FileInfo) + 'static) {
        self.on_activate.replace(Some(Box::new(f)));
    }

    pub fn connect_background(&self, f: impl Fn(&gtk::Widget, f64, f64) + 'static) {
        self.on_background.replace(Some(Box::new(f)));
    }

    fn changed(&self) {
        if let Some(f) = self.on_change.borrow().as_ref() {
            f();
        }
    }

    fn active_column(&self) -> Rc<Column> {
        let columns = self.columns.borrow();
        columns
            .get(self.active.get())
            .or_else(|| columns.last())
            .cloned()
            .expect("the first column is always there")
    }

    /// The selection of the column in use.
    pub fn sel(&self) -> gtk::MultiSelection {
        self.active_column().sel.clone()
    }

    pub fn list(&self) -> gtk::ListView {
        self.active_column().list.clone()
    }

    /// The folder of the column in use: where new and pasted files go.
    pub fn folder(&self) -> gio::File {
        self.active_column().folder.borrow().clone()
    }

    pub fn grab_focus(&self) {
        self.active_column().list.grab_focus();
    }

    /// The window went to `folder`: start over with it in the first column.
    pub fn set_root(&self, folder: &gio::File) {
        self.truncate(0);
        if let Some(first) = self.columns.borrow().first() {
            first.folder.replace(folder.clone());
            first.next.replace(None);
        }
        self.active.set(0);
        self.root.hadjustment().set_value(0.0);
    }

    fn index_of(&self, column: &Rc<Column>) -> Option<usize> {
        self.columns
            .borrow()
            .iter()
            .position(|c| Rc::ptr_eq(c, column))
    }

    fn make_column(
        self: &Rc<Self>,
        folder: &gio::File,
        model: &impl IsA<gio::ListModel>,
    ) -> Rc<Column> {
        let sorted = gtk::SortListModel::new(Some(model.clone()), Some(views::sorter(Key::Name)));
        let sel = gtk::MultiSelection::new(Some(sorted));
        let list = gtk::ListView::builder()
            .model(&sel)
            .factory(&row_factory(&self.cells))
            .css_classes(["files-column-list"])
            .build();
        let scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .width_request(self.width.get())
            .child(&list)
            .build();
        let handle = gtk::Box::builder()
            .width_request(7)
            .cursor(&gdk::Cursor::from_name("col-resize", None).expect("cursor"))
            .css_classes(["files-column-handle"])
            .build();
        let frame = gtk::Box::builder().css_classes(["files-column"]).build();
        frame.append(&scroll);
        frame.append(&handle);
        let column = Rc::new(Column {
            folder: RefCell::new(folder.clone()),
            sel,
            list,
            frame,
            scroll,
            next: RefCell::default(),
        });
        self.wire_column(&column, &handle);
        let last = self
            .columns
            .borrow()
            .last()
            .map(|c| c.frame.clone().upcast::<gtk::Widget>());
        self.strip.insert_child_after(&column.frame, last.as_ref());
        column
    }

    fn wire_column(self: &Rc<Self>, column: &Rc<Column>, handle: &gtk::Box) {
        let (weak, col) = (Rc::downgrade(self), Rc::downgrade(column));
        let with = move |f: &Visit<'_>| {
            let (Some(this), Some(col)) = (weak.upgrade(), col.upgrade()) else {
                return;
            };
            if let Some(i) = this.index_of(&col) {
                f(&this, &col, i);
            }
        };
        let with: Rc<dyn Fn(&Visit<'_>)> = Rc::new(with);

        let w = with.clone();
        column.sel.connect_selection_changed(move |_, _, _| {
            w(&|this, _, i| this.selection_changed(i));
        });
        let w = with.clone();
        column.sel.connect_items_changed(move |_, _, _, added| {
            if added > 0 {
                w(&|this, _, _| {
                    if let Some(f) = this.on_items.borrow().as_ref() {
                        f();
                    }
                });
            }
        });

        // Any press makes the column the one in use, before the row's own
        // gestures act on it; on empty space it clears the selection.
        let press = gtk::GestureClick::builder()
            .button(0)
            .propagation_phase(gtk::PropagationPhase::Capture)
            .build();
        let w = with.clone();
        press.connect_pressed(move |g, _, x, y| {
            w(&|this, col, i| {
                this.active.set(i);
                let list = col.list.upcast_ref::<gtk::Widget>();
                if on_row(list, x, y) {
                    return;
                }
                g.set_state(gtk::EventSequenceState::Claimed);
                col.list.grab_focus();
                let state = g.current_event_state();
                if !state
                    .intersects(gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::SHIFT_MASK)
                {
                    col.sel.unselect_all();
                    this.selection_changed(i);
                }
                if g.current_button() == gdk::BUTTON_SECONDARY {
                    if let Some(f) = this.on_background.borrow().as_ref() {
                        f(list, x, y);
                    }
                }
            });
        });
        column.list.add_controller(press);

        let keys = gtk::EventControllerKey::builder()
            .propagation_phase(gtk::PropagationPhase::Capture)
            .build();
        let w = with.clone();
        keys.connect_key_pressed(move |_, key, _, state| {
            if state.intersects(
                gdk::ModifierType::CONTROL_MASK
                    | gdk::ModifierType::ALT_MASK
                    | gdk::ModifierType::SHIFT_MASK,
            ) {
                return glib::Propagation::Proceed;
            }
            let done = Cell::new(false);
            match key {
                gdk::Key::Right | gdk::Key::KP_Right => {
                    w(&|this, _, i| done.set(this.step_right(i)));
                }
                gdk::Key::Left | gdk::Key::KP_Left => {
                    w(&|this, _, i| done.set(this.step_left(i)));
                }
                _ => {}
            }
            if done.get() {
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        });
        column.list.add_controller(keys);

        let w = with.clone();
        column.list.connect_activate(move |list, pos| {
            let Some(info) = list
                .model()
                .and_then(|m| m.item(pos))
                .and_downcast::<gio::FileInfo>()
            else {
                return;
            };
            w(&|this, _, i| {
                if is_folder(&info) {
                    this.step_right(i);
                } else if let Some(f) = this.on_activate.borrow().as_ref() {
                    f(&info);
                }
            });
        });

        // Files dropped on a column's empty space go into its folder.
        let drop = gtk::DropTarget::new(
            gdk::FileList::static_type(),
            gdk::DragAction::COPY | gdk::DragAction::MOVE,
        );
        drop.set_preload(true);
        let (cells, col) = (self.cells.clone(), Rc::downgrade(column));
        drop.connect_motion(move |t, _, _| {
            let (Some(h), Some(col)) = (cells.handler(), col.upgrade()) else {
                return gdk::DragAction::empty();
            };
            let dest = col.folder.borrow().clone();
            let files = t.value().map(|v| views::files_of(&v)).unwrap_or_default();
            if files
                .iter()
                .any(|f| f.parent().is_some_and(|p| p.equal(&dest)))
            {
                return gdk::DragAction::empty();
            }
            h.drop_action(&dest, &files, views::force_copy(t), views::offered(t))
        });
        let (cells, col) = (self.cells.clone(), Rc::downgrade(column));
        drop.connect_drop(move |t, value, _, _| {
            let (Some(h), Some(col)) = (cells.handler(), col.upgrade()) else {
                return false;
            };
            let dest = col.folder.borrow().clone();
            let files: Vec<gio::File> = views::files_of(value)
                .into_iter()
                .filter(|f| !f.parent().is_some_and(|p| p.equal(&dest)) && !f.equal(&dest))
                .collect();
            if files.is_empty() {
                return false;
            }
            let action = h.drop_action(&dest, &files, views::force_copy(t), views::offered(t));
            if action.is_empty() {
                return false;
            }
            h.drop_files(&dest, files, action);
            true
        });
        column.scroll.add_controller(drop);

        // The right edge resizes the column.
        let drag = gtk::GestureDrag::new();
        let start = Rc::new(Cell::new(DEFAULT_WIDTH));
        let (s, col) = (start.clone(), Rc::downgrade(column));
        drag.connect_drag_begin(move |g, _, _| {
            g.set_state(gtk::EventSequenceState::Claimed);
            if let Some(col) = col.upgrade() {
                s.set(col.scroll.width());
            }
        });
        let (weak, col) = (Rc::downgrade(self), Rc::downgrade(column));
        drag.connect_drag_update(move |_, dx, _| {
            let (Some(this), Some(col)) = (weak.upgrade(), col.upgrade()) else {
                return;
            };
            let width = (start.get() + dx as i32).clamp(MIN_WIDTH, MAX_WIDTH);
            col.scroll.set_size_request(width, -1);
            this.width.set(width);
        });
        handle.add_controller(drag);
    }

    /// Show what the selection in column `i` opens: a folder's column or a
    /// file's preview, or nothing for none or several.
    fn selection_changed(self: &Rc<Self>, i: usize) {
        let Some(column) = self.columns.borrow().get(i).cloned() else {
            return;
        };
        let selected = selected_infos(&column.sel);
        if !selected.is_empty() {
            self.active.set(i);
        }
        let one = match selected.as_slice() {
            [(_, info)] => entry::target(info).map(|f| (f, info.clone())),
            _ => None,
        };
        let next = one.as_ref().map(|(f, _)| f.uri().to_string());
        if *column.next.borrow() == next {
            self.changed();
            return;
        }
        self.truncate(i);
        column.next.replace(next);
        if let Some((file, info)) = one {
            if is_folder(&info) {
                let dir = gtk::DirectoryList::new(Some(entry::ATTRIBUTES), Some(&file));
                dir.set_monitored(true);
                let filtered = gtk::FilterListModel::new(Some(dir), Some(self.filter.clone()));
                let column = self.make_column(&file, &filtered);
                self.columns.borrow_mut().push(column);
            } else {
                self.preview.show(&file);
                self.preview_frame.set_visible(true);
            }
            self.keep_in_sight();
        }
        self.changed();
    }

    /// Scroll to the newest column, for the next few frames too, while it
    /// gets its width.
    fn keep_in_sight(&self) {
        let frames = Rc::new(Cell::new(0u32));
        self.root.add_tick_callback(move |root, _| {
            let adj = root.hadjustment();
            adj.set_value(adj.upper() - adj.page_size());
            frames.set(frames.get() + 1);
            if frames.get() < 12 {
                glib::ControlFlow::Continue
            } else {
                glib::ControlFlow::Break
            }
        });
    }

    /// Remove the columns after `i` and the preview.
    fn truncate(&self, i: usize) {
        let removed: Vec<Rc<Column>> = {
            let mut columns = self.columns.borrow_mut();
            if columns.len() > i + 1 {
                columns.split_off(i + 1)
            } else {
                Vec::new()
            }
        };
        for column in removed {
            self.strip.remove(&column.frame);
        }
        if let Some(column) = self.columns.borrow().get(i) {
            column.next.replace(None);
        }
        self.preview_frame.set_visible(false);
        self.preview.clear();
        if self.active.get() > i {
            self.active.set(i);
        }
    }

    /// Right: into the folder's column, selecting its first item.
    fn step_right(&self, i: usize) -> bool {
        let Some(next) = self.columns.borrow().get(i + 1).cloned() else {
            return false;
        };
        self.active.set(i + 1);
        next.list.grab_focus();
        let selected = selected_infos(&next.sel);
        match selected.first() {
            Some((pos, _)) => next.list.scroll_to(*pos, gtk::ListScrollFlags::FOCUS, None),
            None if next.sel.n_items() > 0 => next.list.scroll_to(
                0,
                gtk::ListScrollFlags::FOCUS | gtk::ListScrollFlags::SELECT,
                None,
            ),
            None => {
                next.list.grab_focus();
            }
        }
        true
    }

    /// Left: back to the folder in the column before, leaving this one
    /// open but with nothing selected.
    fn step_left(self: &Rc<Self>, i: usize) -> bool {
        if i == 0 {
            return false;
        }
        let Some((column, prev)) = ({
            let columns = self.columns.borrow();
            columns.get(i).cloned().zip(columns.get(i - 1).cloned())
        }) else {
            return false;
        };
        column.sel.unselect_all();
        self.selection_changed(i);
        self.active.set(i - 1);
        prev.list.grab_focus();
        match selected_infos(&prev.sel).first() {
            Some((pos, _)) => prev.list.scroll_to(*pos, gtk::ListScrollFlags::FOCUS, None),
            None => {
                prev.list.grab_focus();
            }
        }
        self.changed();
        true
    }

    /// Select and show the item at `pos` in the column in use.
    pub fn scroll_to(&self, pos: u32) {
        self.active_column()
            .list
            .scroll_to(pos, gtk::ListScrollFlags::FOCUS, None);
    }
}
