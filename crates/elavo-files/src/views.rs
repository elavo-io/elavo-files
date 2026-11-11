//! Cell factories and sorters for the Icons view (a `GridView`) and the
//! List view (a `ColumnView` over a `TreeListModel`). Every cell carries
//! the same controllers: right-click (or a long press on the touchscreen)
//! for the context menu, a drag source for the item and, on folders, a
//! drop target. What they do is up to the window, through [`CellHandler`].

use std::cell::RefCell;
use std::cmp::Ordering;
use std::collections::HashMap;
use std::rc::{Rc, Weak};

use elavo_files_core::entry;
use elavo_files_core::sort::{self, Key};
use gtk::prelude::*;
use gtk::{gdk, gio, glib};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Icons,
    List,
    Columns,
    Gallery,
}

/// What the window does for the cells.
pub trait CellHandler {
    /// The context menu for the item at `position`, at `x, y` in `cell`.
    fn item_menu(&self, view: View, position: u32, cell: &gtk::Widget, x: f64, y: f64);
    /// The files a drag from the item at `position` carries.
    fn drag_files(&self, view: View, position: u32) -> Vec<gio::File>;
    /// Copy or move, for files dragged over `dest`.
    fn drop_action(
        &self,
        dest: &gio::File,
        files: &[gio::File],
        force_copy: bool,
        offered: gdk::DragAction,
    ) -> gdk::DragAction;
    fn drop_files(&self, dest: &gio::File, files: Vec<gio::File>, action: gdk::DragAction);
}

/// The cells on screen by file URI, for Rename to find its item, and the
/// window's handler.
#[derive(Default)]
pub struct Cells {
    // Both views bind the same files, so a uri can have a cell in each.
    widgets: RefCell<HashMap<String, Vec<glib::WeakRef<gtk::Widget>>>>,
    handler: RefCell<Option<Weak<dyn CellHandler>>>,
}

impl Cells {
    pub fn set_handler(&self, handler: Weak<dyn CellHandler>) {
        self.handler.replace(Some(handler));
    }

    pub fn handler(&self) -> Option<Rc<dyn CellHandler>> {
        self.handler.borrow().as_ref().and_then(Weak::upgrade)
    }

    /// The bound cell showing `uri`, if it is on screen.
    pub fn widget(&self, uri: &str) -> Option<gtk::Widget> {
        self.widgets
            .borrow()
            .get(uri)?
            .iter()
            .filter_map(|w| w.upgrade())
            .find(|w| w.is_mapped())
    }

    /// Every bound cell on screen, with its file's uri.
    pub fn mapped(&self) -> Vec<(String, gtk::Widget)> {
        self.widgets
            .borrow()
            .iter()
            .flat_map(|(uri, list)| {
                list.iter()
                    .filter_map(|w| w.upgrade())
                    .filter(|w| w.is_mapped())
                    .map(move |w| (uri.clone(), w))
            })
            .collect()
    }

    pub fn bind(&self, info: &gio::FileInfo, widget: &gtk::Widget) {
        if let Some(file) = entry::file(info) {
            let mut map = self.widgets.borrow_mut();
            let list = map.entry(file.uri().to_string()).or_default();
            list.retain(|w| w.upgrade().is_some_and(|w| &w != widget));
            list.push(widget.downgrade());
        }
    }

    pub fn unbind(&self, info: &gio::FileInfo, widget: &gtk::Widget) {
        if let Some(file) = entry::file(info) {
            let uri = file.uri().to_string();
            let mut map = self.widgets.borrow_mut();
            if let Some(list) = map.get_mut(&uri) {
                list.retain(|w| w.upgrade().is_some_and(|w| &w != widget));
                if list.is_empty() {
                    map.remove(&uri);
                }
            }
        }
    }
}

pub fn files_of(value: &glib::Value) -> Vec<gio::File> {
    value
        .get::<gdk::FileList>()
        .map(|l| l.files())
        .unwrap_or_default()
}

/// The class of the parts of a list row that pick its file up.
pub const GRAB: &str = "files-grab";

/// Whether `(x, y)` in `cell` is on its icon or name.
pub fn on_grab(cell: &gtk::Widget, x: f64, y: f64) -> bool {
    let mut w = cell.pick(x, y, gtk::PickFlags::DEFAULT);
    while let Some(x) = w {
        if x.has_css_class(GRAB) {
            return true;
        }
        if &x == cell {
            return false;
        }
        w = x.parent();
    }
    false
}

/// What the drag's source allows. On Wayland a drop's own actions are only
/// the one the compositor settled on for the last place the pointer was, so
/// passing over a row that wanted Copy would rule out Move everywhere after.
/// A drag from Files itself says what it allows.
pub fn offered(target: &gtk::DropTarget) -> gdk::DragAction {
    let Some(drop) = target.current_drop() else {
        return gdk::DragAction::COPY;
    };
    match drop.drag() {
        Some(drag) => drag.actions(),
        None => drop.actions(),
    }
}

/// Ctrl or Alt (Finder's Option) held: copy instead of move. Wayland drag
/// events carry no modifiers, so the keyboard's own state counts too.
pub fn force_copy(target: &gtk::DropTarget) -> bool {
    let keyboard = target
        .widget()
        .map(|w| w.display())
        .and_then(|d| d.default_seat())
        .and_then(|s| s.keyboard())
        .map_or(gdk::ModifierType::empty(), |k| k.modifier_state());
    (target.current_event_state() | keyboard)
        .intersects(gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::ALT_MASK)
}

fn item_info(item: &glib::WeakRef<gtk::ListItem>) -> Option<gio::FileInfo> {
    item.upgrade()?.item().and_then(|o| info_of(&o))
}

/// The controllers every cell gets. `image` is the drag icon.
pub fn wire(
    cell: &gtk::Widget,
    item: &gtk::ListItem,
    cells: &Rc<Cells>,
    view: View,
    image: Option<&gtk::Image>,
) {
    cell.add_css_class("files-cell");
    let weak_item = item.downgrade();

    let menu = {
        let (cells, item, cell) = (cells.clone(), weak_item.clone(), cell.downgrade());
        move |x: f64, y: f64| {
            let (Some(h), Some(item), Some(cell)) =
                (cells.handler(), item.upgrade(), cell.upgrade())
            else {
                return;
            };
            h.item_menu(view, item.position(), &cell, x, y);
        }
    };
    let click = gtk::GestureClick::builder()
        .button(gdk::BUTTON_SECONDARY)
        .build();
    let m = menu.clone();
    let w = cell.downgrade();
    click.connect_pressed(move |g, _, x, y| {
        // In Icons only the picture and the name are the item; around
        // them is the folder's background.
        if view == View::Icons && !w.upgrade().is_some_and(|w| on_grab(&w, x, y)) {
            return;
        }
        g.set_state(gtk::EventSequenceState::Claimed);
        m(x, y);
    });
    cell.add_controller(click);
    let press = gtk::GestureLongPress::builder().touch_only(true).build();
    press.connect_pressed(move |g, x, y| {
        g.set_state(gtk::EventSequenceState::Claimed);
        menu(x, y);
    });
    cell.add_controller(press);

    let drag = gtk::DragSource::builder()
        .actions(gdk::DragAction::COPY | gdk::DragAction::MOVE)
        .build();
    let (c, it, w) = (cells.clone(), weak_item.clone(), cell.downgrade());
    drag.connect_prepare(move |_, x, y| {
        if matches!(view, View::Icons | View::List)
            && !w.upgrade().is_some_and(|w| on_grab(&w, x, y))
        {
            return None;
        }
        let (h, item) = (c.handler()?, it.upgrade()?);
        let files = h.drag_files(view, item.position());
        if files.is_empty() {
            return None;
        }
        Some(gdk::ContentProvider::for_value(
            &gdk::FileList::from_array(&files).to_value(),
        ))
    });
    if let Some(image) = image {
        let image = image.downgrade();
        drag.connect_drag_begin(move |source, _| {
            if let Some(image) = image.upgrade() {
                let icon = gtk::WidgetPaintable::new(Some(&image));
                source.set_icon(Some(&icon), image.width() / 2, image.height() / 2);
            }
        });
    }
    cell.add_controller(drag);

    // Folders take drops.
    let drop = gtk::DropTarget::new(
        gdk::FileList::static_type(),
        gdk::DragAction::COPY | gdk::DragAction::MOVE,
    );
    drop.set_preload(true);
    let it = weak_item.clone();
    drop.connect_accept(move |_, d| {
        item_info(&it).is_some_and(|i| entry::is_dir(&i) && !elavo_files_core::eapp::is_package(&i))
            && d.formats().contains_type(gdk::FileList::static_type())
    });
    let (c, it) = (cells.clone(), weak_item.clone());
    drop.connect_motion(move |t, _, _| {
        let (Some(h), Some(info)) = (c.handler(), item_info(&it)) else {
            return gdk::DragAction::empty();
        };
        let Some(dest) = entry::target(&info) else {
            return gdk::DragAction::empty();
        };
        let files = t.value().map(|v| files_of(&v)).unwrap_or_default();
        if files.iter().any(|f| f.equal(&dest)) {
            return gdk::DragAction::empty();
        }
        let offered = offered(t);
        h.drop_action(&dest, &files, force_copy(t), offered)
    });
    let (c, it) = (cells.clone(), weak_item);
    drop.connect_drop(move |t, value, _, _| {
        let (Some(h), Some(info)) = (c.handler(), item_info(&it)) else {
            return false;
        };
        let Some(dest) = entry::target(&info) else {
            return false;
        };
        let files: Vec<gio::File> = files_of(value)
            .into_iter()
            .filter(|f| !f.equal(&dest))
            .collect();
        if files.is_empty() {
            return false;
        }
        let offered = offered(t);
        let action = h.drop_action(&dest, &files, force_copy(t), offered);
        h.drop_files(&dest, files, action);
        true
    });
    cell.add_controller(drop);
}

/// Where a loaded thumbnail texture is kept on its `FileInfo`, and the
/// size it was loaded for.
const TEXTURE: &str = "elavo-files::texture";
const TEXTURE_SIZE: &str = "elavo-files::texture-size";

/// The pixels a picture shown at `size` is loaded at: enough for the
/// screen's scale and one zoom step, and no more, so a big folder of
/// photos doesn't keep hundreds of large textures around.
fn load_size(image: &gtk::Image, size: i32) -> i32 {
    (size * image.scale_factor().max(1) * 3 / 2).clamp(32, 256)
}

/// The `FileInfo` behind a row: the item itself in Icons, or the item of a
/// `TreeListRow` in List.
pub fn info_of(object: &glib::Object) -> Option<gio::FileInfo> {
    if let Some(row) = object.downcast_ref::<gtk::TreeListRow>() {
        return row.item().and_then(|i| i.downcast::<gio::FileInfo>().ok());
    }
    object.clone().downcast::<gio::FileInfo>().ok()
}

/// Load the picture of a file (a cached thumbnail, a scaled-down image or
/// one a thumbnailer makes) and show it once it is ready, if `image` still
/// shows the same file. Loads queue up, a few at a time, so a big folder
/// lists at once and fills in its pictures afterwards.
fn load_thumbnail(image: &gtk::Image, info: &gio::FileInfo, size: i32) {
    let Some(file) = entry::target(info) else {
        return;
    };
    let uri = file.uri().to_string();
    image.set_widget_name(&uri);
    let want = load_size(image, size);
    let weak = image.downgrade();
    let info = info.clone();
    glib::spawn_future_local(async move {
        let wanted = || weak.upgrade().is_some_and(|i| i.widget_name() == uri);
        let Some(texture) = crate::thumbs::texture(&file, Some(&info), want, &wanted).await else {
            return;
        };
        info.set_attribute_object(TEXTURE, &texture);
        info.set_attribute_int32(TEXTURE_SIZE, want);
        if let Some(image) = weak.upgrade().filter(|i| i.widget_name() == uri) {
            image.set_paintable(Some(&texture));
            image.set_pixel_size(size);
        }
    });
}

/// Whether the icon theme really has `name`, as bundles need. A name counts
/// only if its file is there: the icon cache can list Flatpak exports whose
/// files are missing.
pub fn themed(display: &gdk::Display) -> impl Fn(&str) -> bool {
    let theme = gtk::IconTheme::for_display(display);
    move |name: &str| {
        theme.has_icon(name)
            && theme
                .lookup_icon(
                    name,
                    &[],
                    64,
                    1,
                    gtk::TextDirection::Ltr,
                    gtk::IconLookupFlags::empty(),
                )
                .file()
                .and_then(|f| f.path())
                .is_some_and(|p| p.exists())
    }
}

/// The file's picture when it is big enough to show one, otherwise the
/// themed icon for the file (folders get the theme's folder icon).
pub fn set_icon(image: &gtk::Image, info: &gio::FileInfo, size: i32) {
    image.set_pixel_size(size);
    image.set_widget_name("");
    if size >= 24 && info.attribute_int32(TEXTURE_SIZE) >= load_size(image, size) {
        if let Some(texture) = info
            .attribute_object(TEXTURE)
            .and_then(|o| o.downcast::<gdk::Texture>().ok())
        {
            image.set_paintable(Some(&texture));
            return;
        }
    }
    let icon = info
        .icon()
        .unwrap_or_else(|| gio::content_type_get_icon(&entry::content_type(info)));
    image.set_from_gicon(&icon);
    if size >= 24 && !entry::is_dir(info) {
        load_thumbnail(image, info, size);
    }
}

/// Icons view cells: the icon over a name of up to two lines.
pub fn icons_factory(size: i32, cells: &Rc<Cells>) -> gtk::SignalListItemFactory {
    let factory = gtk::SignalListItemFactory::new();
    let c = cells.clone();
    factory.connect_setup(move |_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().expect("list item");
        let image = gtk::Image::builder()
            .pixel_size(size)
            .halign(gtk::Align::Center)
            .css_classes(["files-icon-picture", GRAB])
            .build();
        let name = gtk::Label::builder()
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .lines(2)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .justify(gtk::Justification::Center)
            .max_width_chars(((size + 40) / 7).max(11))
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Start)
            .css_classes(["files-icon-name", GRAB])
            .build();
        // Break long names between characters without adding hyphens.
        let attrs = gtk::pango::AttrList::new();
        attrs.insert(gtk::pango::AttrInt::new_insert_hyphens(false));
        name.set_attributes(Some(&attrs));
        let cell = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .width_request(size + 44)
            .valign(gtk::Align::Start)
            .build();
        cell.append(&image);
        cell.append(&name);
        wire(cell.upcast_ref(), item, &c, View::Icons, Some(&image));
        item.set_child(Some(&cell));
    });
    let c = cells.clone();
    factory.connect_bind(move |_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().expect("list item");
        let (Some(info), Some(cell)) = (item.item().and_then(|o| info_of(&o)), item.child()) else {
            return;
        };
        let image = cell
            .first_child()
            .and_downcast::<gtk::Image>()
            .expect("image");
        let name = image
            .next_sibling()
            .and_downcast::<gtk::Label>()
            .expect("label");
        set_icon(&image, &info, size);
        let text = entry::name(&info);
        name.set_label(&text);
        cell.set_tooltip_text(Some(&text));
        c.bind(&info, &cell);
    });
    let c = cells.clone();
    factory.connect_unbind(move |_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().expect("list item");
        if let (Some(info), Some(cell)) = (item.item().and_then(|o| info_of(&o)), item.child()) {
            c.unbind(&info, &cell);
        }
    });
    factory
}

/// Load a folder's children only when its row is first expanded, so a
/// list of many folders does not read them all.
fn watch_expansion(row: &gtk::TreeListRow) {
    // SAFETY: the key holds a plain bool and is only read with that type.
    unsafe {
        if row.data::<bool>("elavo-files-watched").is_some() {
            return;
        }
        row.set_data("elavo-files-watched", true);
    }
    row.connect_expanded_notify(|row| {
        if !row.is_expanded() {
            return;
        }
        let Some(children) = row.children() else {
            return;
        };
        let dir = children
            .downcast_ref::<gtk::FilterListModel>()
            .and_then(|f| f.model())
            .and_downcast::<gtk::DirectoryList>();
        let info = row.item().and_downcast::<gio::FileInfo>();
        if let (Some(dir), Some(info)) = (dir, info) {
            if dir.file().is_none() {
                dir.set_file(entry::target(&info).as_ref());
            }
        }
    });
}

/// List view Name cells: a disclosure triangle for folders, the icon and
/// the name.
pub fn name_factory(size: i32, cells: &Rc<Cells>) -> gtk::SignalListItemFactory {
    let factory = gtk::SignalListItemFactory::new();
    let c = cells.clone();
    factory.connect_setup(move |_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().expect("list item");
        let content = gtk::Box::builder().spacing(6).build();
        // Only the icon and the name pick the file up; a drag anywhere else
        // in the row draws a selection rectangle, as in Finder.
        let image = gtk::Image::builder()
            .pixel_size(size)
            .css_classes([GRAB])
            .build();
        content.append(&image);
        content.append(
            &gtk::Label::builder()
                .xalign(0.0)
                .ellipsize(gtk::pango::EllipsizeMode::Middle)
                .css_classes([GRAB])
                .build(),
        );
        let expander = gtk::TreeExpander::builder()
            .child(&content)
            .indent_for_icon(true)
            .build();
        wire(expander.upcast_ref(), item, &c, View::List, Some(&image));
        item.set_child(Some(&expander));
    });
    let c = cells.clone();
    factory.connect_bind(move |_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().expect("list item");
        let Some(row) = item.item().and_downcast::<gtk::TreeListRow>() else {
            return;
        };
        let Some(expander) = item.child().and_downcast::<gtk::TreeExpander>() else {
            return;
        };
        expander.set_list_row(Some(&row));
        watch_expansion(&row);
        let Some(info) = row.item().and_downcast::<gio::FileInfo>() else {
            return;
        };
        let content = expander.child().expect("content");
        let image = content
            .first_child()
            .and_downcast::<gtk::Image>()
            .expect("image");
        let label = image
            .next_sibling()
            .and_downcast::<gtk::Label>()
            .expect("label");
        set_icon(&image, &info, size);
        label.set_label(&entry::name(&info));
        c.bind(&info, expander.upcast_ref());
    });
    let c = cells.clone();
    factory.connect_unbind(move |_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().expect("list item");
        if let Some(expander) = item.child().and_downcast::<gtk::TreeExpander>() {
            if let Some(info) = item.item().and_then(|o| info_of(&o)) {
                c.unbind(&info, expander.upcast_ref());
            }
            expander.set_list_row(None);
        }
    });
    factory
}

/// A one-label column: Date Modified, Size or Kind.
pub fn text_factory(
    text: fn(&gio::FileInfo) -> String,
    xalign: f32,
    cells: &Rc<Cells>,
) -> gtk::SignalListItemFactory {
    let factory = gtk::SignalListItemFactory::new();
    let c = cells.clone();
    factory.connect_setup(move |_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().expect("list item");
        let label = gtk::Label::builder()
            .xalign(xalign)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .css_classes(["files-dim"])
            .build();
        wire(label.upcast_ref(), item, &c, View::List, None);
        item.set_child(Some(&label));
    });
    factory.connect_bind(move |_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().expect("list item");
        let (Some(info), Some(label)) = (
            item.item().and_then(|o| info_of(&o)),
            item.child().and_downcast::<gtk::Label>(),
        ) else {
            return;
        };
        label.set_label(&text(&info));
    });
    factory
}

fn ordering(o: Ordering) -> gtk::Ordering {
    match o {
        Ordering::Less => gtk::Ordering::Smaller,
        Ordering::Equal => gtk::Ordering::Equal,
        Ordering::Greater => gtk::Ordering::Larger,
    }
}

/// Ascending order by `key` over `FileInfo` items.
pub fn sorter(key: Key) -> gtk::CustomSorter {
    gtk::CustomSorter::new(move |a, b| {
        let (Some(a), Some(b)) = (info_of(a), info_of(b)) else {
            return gtk::Ordering::Equal;
        };
        let mut sa = Default::default();
        let mut sb = Default::default();
        let fa = entry::fields(&a, &mut sa);
        let fb = entry::fields(&b, &mut sb);
        ordering(sort::compare(key, &fa, &fb))
    })
}
