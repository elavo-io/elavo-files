//! Previews of a file, for the Columns view's preview column, the Gallery
//! view and the preview pane (Show Preview) beside Icons and List.
//!
//! - [`Stage`] is the picture: an image scaled to fit, the start of a text
//!   file, a thumbnail from the cache or a thumbnailer, or the file's icon.
//! - [`Preview`] is Finder's preview panel: optionally a stage, then the
//!   name, kind and size, the dates (and an image's dimensions), and the
//!   quick actions Rotate Left, Markup and More.
//!
//! Everything loads asynchronously; a newer request wins over one still
//! loading.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use elavo_files_core::{eapp, entry, format};
use gtk::prelude::*;
use gtk::{gio, glib};

use crate::i18n::tr;
use crate::thumbs;

/// What a preview reads about a file.
const ATTRIBUTES: &str = "standard::*,time::modified,time::created,time::access,\
thumbnail::path,thumbnail::failed,thumbnail::is-valid,access::can-write";

/// Plain text shows up to this much of the file.
const TEXT_BYTES: usize = 24 * 1024;
const TEXT_MAX_FILE: u64 = 64 * 1024 * 1024;

/// Image types Rotate Left can write back.
fn rotatable(content_type: &str) -> Option<&'static str> {
    match content_type {
        "image/png" => Some("png"),
        "image/jpeg" => Some("jpeg"),
        "image/bmp" => Some("bmp"),
        "image/tiff" => Some("tiff"),
        _ => None,
    }
}

/// Turn an image a quarter turn to the left and save it in place, as
/// Finder's Rotate Left does.
pub async fn rotate_left(file: &gio::File) -> Result<(), String> {
    let info = file
        .query_info_future(
            "standard::content-type",
            gio::FileQueryInfoFlags::NONE,
            glib::Priority::DEFAULT,
        )
        .await
        .map_err(|e| e.to_string())?;
    let ct = entry::content_type(&info);
    let format = rotatable(&ct).ok_or_else(|| tr("This kind of image can't be rotated."))?;
    let stream = file
        .read_future(glib::Priority::DEFAULT)
        .await
        .map_err(|e| e.to_string())?;
    let pixbuf = gtk::gdk_pixbuf::Pixbuf::from_stream_future(&stream)
        .await
        .map_err(|e| e.to_string())?;
    let pixbuf = pixbuf.apply_embedded_orientation().unwrap_or(pixbuf);
    let turned = pixbuf
        .rotate_simple(gtk::gdk_pixbuf::PixbufRotation::Counterclockwise)
        .ok_or_else(|| tr("The image couldn't be rotated."))?;
    let options: &[(&str, &str)] = if format == "jpeg" {
        &[("quality", "95")]
    } else {
        &[]
    };
    let bytes = turned
        .save_to_bufferv(format, options)
        .map_err(|e| e.to_string())?;
    file.replace_contents_future(bytes, None, false, gio::FileCreateFlags::NONE)
        .await
        .map(|_| ())
        .map_err(|(_, e)| e.to_string())
}

/// The picture of a file.
pub struct Stage {
    pub root: gtk::Stack,
    picture: gtk::Picture,
    text: gtk::TextView,
    icon: gtk::Image,
    /// The longest side images are decoded at.
    max: i32,
    generation: Cell<u32>,
}

impl Stage {
    pub fn new(max: i32) -> Rc<Self> {
        let picture = gtk::Picture::builder()
            .content_fit(gtk::ContentFit::Contain)
            .can_shrink(true)
            .hexpand(true)
            .vexpand(true)
            .css_classes(["files-stage-picture"])
            .build();
        let text = gtk::TextView::builder()
            .editable(false)
            .cursor_visible(false)
            .monospace(true)
            .wrap_mode(gtk::WrapMode::WordChar)
            .top_margin(10)
            .bottom_margin(10)
            .left_margin(12)
            .right_margin(12)
            .css_classes(["files-stage-text"])
            .build();
        let text_scroll = gtk::ScrolledWindow::builder()
            .child(&text)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .hexpand(true)
            .vexpand(true)
            .css_classes(["files-stage-page"])
            .build();
        let icon = gtk::Image::builder()
            .pixel_size((max / 2).clamp(64, 192))
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .build();
        let root = gtk::Stack::builder()
            .transition_type(gtk::StackTransitionType::Crossfade)
            .transition_duration(120)
            .hexpand(true)
            .vexpand(true)
            .css_classes(["files-stage"])
            .build();
        root.add_named(&icon, Some("icon"));
        root.add_named(&picture, Some("picture"));
        root.add_named(&text_scroll, Some("text"));
        Rc::new(Self {
            root,
            picture,
            text,
            icon,
            max,
            generation: Cell::new(0),
        })
    }

    pub fn clear(&self) {
        self.generation.set(self.generation.get().wrapping_add(1));
        self.icon.set_icon_name(None);
        self.picture.set_paintable(None::<&gtk::gdk::Paintable>);
        self.root.set_visible_child_name("icon");
    }

    /// Show `file`; `info` is what is known of it already.
    pub fn show(self: &Rc<Self>, file: &gio::File, info: &gio::FileInfo) {
        let generation = self.generation.get().wrapping_add(1);
        self.generation.set(generation);
        let icon = info
            .icon()
            .unwrap_or_else(|| gio::content_type_get_icon(&entry::content_type(info)));
        self.icon.set_from_gicon(&icon);
        self.root.set_visible_child_name("icon");
        if entry::is_dir(info) {
            return;
        }
        let ct = entry::content_type(info);
        let (file, info, this) = (file.clone(), info.clone(), Rc::downgrade(self));
        glib::spawn_future_local(async move {
            let current = |s: &Self| s.generation.get() == generation;
            let wanted = || this.upgrade().is_some_and(|s| current(&s));
            let size = entry::size(&info);
            if gio::content_type_is_a(&ct, "text/plain") && size <= TEXT_MAX_FILE {
                if let Some(text) = read_text(&file).await {
                    if let Some(s) = this.upgrade().filter(|s| current(s)) {
                        s.text.buffer().set_text(&text);
                        s.root.set_visible_child_name("text");
                    }
                    return;
                }
            }
            let max = match this.upgrade() {
                Some(s) => s.max,
                None => return,
            };
            let texture = if ct.starts_with("image/") && size <= thumbs::IMAGE_MAX_BYTES {
                thumbs::image(&file, max).await
            } else {
                thumbs::texture(&file, Some(&info), max.min(256), &wanted).await
            };
            if let (Some(t), Some(s)) = (texture, this.upgrade().filter(|s| current(s))) {
                s.picture.set_paintable(Some(&t));
                s.root.set_visible_child_name("picture");
            }
        });
    }
}

async fn read_text(file: &gio::File) -> Option<String> {
    let stream = file.read_future(glib::Priority::LOW).await.ok()?;
    let bytes = stream
        .read_bytes_future(TEXT_BYTES, glib::Priority::LOW)
        .await
        .ok()?;
    let text = String::from_utf8_lossy(&bytes);
    // Binary data that only claims to be text.
    if text.chars().take(2048).filter(|c| *c == '\u{fffd}').count() > 8 {
        return None;
    }
    Some(text.into_owned())
}

type WidgetFn = Box<dyn Fn(&gtk::Widget)>;
type TextFn = Box<dyn Fn(String)>;
type FileFn = Box<dyn Fn(&gio::File)>;

/// Finder's preview panel.
pub struct Preview {
    pub root: gtk::Box,
    stage: Option<Rc<Stage>>,
    name: gtk::Label,
    kind: gtk::Label,
    info_title: gtk::Label,
    grid: gtk::Grid,
    actions: gtk::Box,
    rotate: gtk::Button,
    /// Rotate Left and Markup, which an application has no use for.
    edits: [gtk::Box; 2],
    more: gtk::Button,
    file: RefCell<Option<gio::File>>,
    generation: Cell<u32>,
    on_more: RefCell<Option<WidgetFn>>,
    on_error: RefCell<Option<TextFn>>,
    on_rotated: RefCell<Option<FileFn>>,
}

fn action(icon: &str, label: &str) -> (gtk::Button, gtk::Box) {
    let button = gtk::Button::builder()
        .icon_name(icon)
        .halign(gtk::Align::Center)
        .css_classes(["files-quick-action", "circular"])
        .build();
    let caption = gtk::Label::builder()
        .label(label)
        .css_classes(["files-quick-caption"])
        .build();
    let column = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(4)
        .build();
    column.append(&button);
    column.append(&caption);
    (button, column)
}

impl Preview {
    /// `stage` is the size images are decoded at, or None for a panel
    /// without a picture (the Gallery's, whose picture is in the middle).
    pub fn new(stage: Option<i32>) -> Rc<Self> {
        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(8)
            .css_classes(["files-preview"])
            .build();
        let stage = stage.map(Stage::new);
        if let Some(s) = &stage {
            s.root.set_vexpand(false);
            s.root.set_size_request(-1, 200);
            root.append(&s.root);
        }
        let name = gtk::Label::builder()
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .lines(3)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .xalign(0.0)
            .css_classes(["files-preview-name"])
            .build();
        let kind = gtk::Label::builder()
            .xalign(0.0)
            .wrap(true)
            .css_classes(["files-dim"])
            .build();
        let info_title = gtk::Label::builder()
            .label(tr("Information"))
            .xalign(0.0)
            .css_classes(["files-preview-heading"])
            .build();
        let grid = gtk::Grid::builder()
            .column_spacing(12)
            .row_spacing(6)
            .css_classes(["files-preview-info"])
            .build();
        root.append(&name);
        root.append(&kind);
        root.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        root.append(&info_title);
        root.append(&grid);
        // The quick actions sit at the bottom.
        root.append(&gtk::Box::builder().vexpand(true).build());
        let (rotate, rotate_col) = action("object-rotate-left-symbolic", &tr("Rotate Left"));
        let (markup, markup_col) = action("document-edit-symbolic", &tr("Markup"));
        markup.set_sensitive(false);
        markup.set_tooltip_text(Some(&tr("Markup comes in a later version")));
        let (more, more_col) = action("view-more-horizontal-symbolic", &tr("More…"));
        let actions = gtk::Box::builder()
            .spacing(14)
            .halign(gtk::Align::Center)
            .homogeneous(true)
            .css_classes(["files-quick-actions"])
            .build();
        actions.append(&rotate_col);
        actions.append(&markup_col);
        actions.append(&more_col);
        root.append(&actions);
        let this = Rc::new(Self {
            root,
            stage,
            name,
            kind,
            info_title,
            grid,
            actions,
            rotate,
            edits: [rotate_col, markup_col],
            more,
            file: RefCell::default(),
            generation: Cell::new(0),
            on_more: RefCell::default(),
            on_error: RefCell::default(),
            on_rotated: RefCell::default(),
        });
        let weak = Rc::downgrade(&this);
        this.more.connect_clicked(move |b| {
            if let Some(p) = weak.upgrade() {
                if let Some(f) = p.on_more.borrow().as_ref() {
                    f(b.upcast_ref());
                }
            }
        });
        let weak = Rc::downgrade(&this);
        this.rotate.connect_clicked(move |b| {
            let Some(p) = weak.upgrade() else {
                return;
            };
            let Some(file) = p.file.borrow().clone() else {
                return;
            };
            b.set_sensitive(false);
            let weak = Rc::downgrade(&p);
            glib::spawn_future_local(async move {
                let result = rotate_left(&file).await;
                let Some(p) = weak.upgrade() else {
                    return;
                };
                match result {
                    Ok(()) => {
                        p.show(&file);
                        if let Some(f) = p.on_rotated.borrow().as_ref() {
                            f(&file);
                        }
                    }
                    Err(why) => {
                        p.rotate.set_sensitive(true);
                        if let Some(f) = p.on_error.borrow().as_ref() {
                            f(why);
                        }
                    }
                }
            });
        });
        this.clear();
        this
    }

    /// More… : the window shows the item's menu at the button.
    pub fn connect_more(&self, f: impl Fn(&gtk::Widget) + 'static) {
        self.on_more.replace(Some(Box::new(f)));
    }

    pub fn connect_error(&self, f: impl Fn(String) + 'static) {
        self.on_error.replace(Some(Box::new(f)));
    }

    /// An image was rotated, and so written anew.
    pub fn connect_rotated(&self, f: impl Fn(&gio::File) + 'static) {
        self.on_rotated.replace(Some(Box::new(f)));
    }

    fn rows(&self, rows: &[(String, String)]) {
        while let Some(child) = self.grid.first_child() {
            self.grid.remove(&child);
        }
        for (i, (key, value)) in rows.iter().enumerate() {
            let k = gtk::Label::builder()
                .label(key)
                .xalign(0.0)
                .valign(gtk::Align::Start)
                .css_classes(["files-dim"])
                .build();
            let v = gtk::Label::builder()
                .label(value)
                .xalign(1.0)
                .hexpand(true)
                .wrap(true)
                .selectable(false)
                .build();
            self.grid.attach(&k, 0, i as i32, 1, 1);
            self.grid.attach(&v, 1, i as i32, 1, 1);
        }
        self.info_title.set_visible(!rows.is_empty());
    }

    pub fn clear(&self) {
        self.generation.set(self.generation.get().wrapping_add(1));
        self.file.replace(None);
        if let Some(s) = &self.stage {
            s.clear();
        }
        self.name.set_label("");
        self.kind.set_label("");
        self.rows(&[]);
        self.actions.set_visible(false);
    }

    /// "3 items", for a selection of more than one.
    pub fn show_count(&self, n: usize) {
        self.clear();
        self.name.set_label(&format!("{n} {}", tr("items")));
    }

    pub fn show(self: &Rc<Self>, file: &gio::File) {
        let generation = self.generation.get().wrapping_add(1);
        self.generation.set(generation);
        self.file.replace(Some(file.clone()));
        let (file, weak) = (file.clone(), Rc::downgrade(self));
        glib::spawn_future_local(async move {
            let info = file
                .query_info_future(
                    ATTRIBUTES,
                    gio::FileQueryInfoFlags::NONE,
                    glib::Priority::DEFAULT,
                )
                .await;
            let Some(p) = weak.upgrade().filter(|p| p.generation.get() == generation) else {
                return;
            };
            match info {
                Ok(info) => {
                    // An app bundle shows as the app, as in the views.
                    info.set_attribute_object("standard::file", &file);
                    eapp::decorate(&info, &crate::views::themed(&p.root.display()));
                    p.fill(&file, &info, generation)
                }
                Err(err) => {
                    p.name.set_label(&file.basename().map_or_else(
                        || file.uri().to_string(),
                        |b| b.to_string_lossy().into_owned(),
                    ));
                    p.kind.set_label(err.message());
                    p.rows(&[]);
                    p.actions.set_visible(false);
                }
            }
        });
    }

    fn fill(self: &Rc<Self>, file: &gio::File, info: &gio::FileInfo, generation: u32) {
        if let Some(s) = &self.stage {
            s.show(file, info);
        }
        self.name.set_label(&entry::name(info));
        let package = eapp::is_package(info);
        let dir = entry::is_dir(info);
        let kind = entry::kind(info);
        self.kind.set_label(&if dir {
            kind
        } else {
            format!("{kind} \u{2014} {}", format::size(entry::size(info)))
        });
        let date = |attr: &str| {
            let secs = info.attribute_uint64(attr) as i64;
            (secs > 0).then(|| entry::date_text(secs))
        };
        let mut rows = Vec::new();
        if let Some(d) = date("time::created") {
            rows.push((tr("Created"), d));
        }
        if let Some(d) = date("time::modified") {
            rows.push((tr("Modified"), d));
        }
        if !dir {
            if let Some(d) = date("time::access") {
                rows.push((tr("Last opened"), d));
            }
        }
        self.rows(&rows);
        let ct = entry::content_type(info);
        let writable =
            info.boolean("access::can-write") || !info.has_attribute("access::can-write");
        self.rotate
            .set_sensitive(rotatable(&ct).is_some() && writable);
        self.actions.set_visible(!dir || package);
        for edit in &self.edits {
            edit.set_visible(!package);
        }
        self.more.set_sensitive(true);
        // An image's dimensions, from its header.
        if ct.starts_with("image/") {
            if let Some(path) = file.path() {
                let weak = Rc::downgrade(self);
                glib::spawn_future_local(async move {
                    let dims = gtk::gdk_pixbuf::Pixbuf::file_info_future(path).await;
                    let Some(p) = weak.upgrade().filter(|p| p.generation.get() == generation)
                    else {
                        return;
                    };
                    if let Ok(Some((_, w, h))) = dims {
                        let mut rows = rows;
                        rows.push((tr("Dimensions"), format!("{w} \u{d7} {h}")));
                        p.rows(&rows);
                    }
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotatable_types() {
        assert_eq!(rotatable("image/jpeg"), Some("jpeg"));
        assert_eq!(rotatable("image/png"), Some("png"));
        assert_eq!(rotatable("image/svg+xml"), None);
        assert_eq!(rotatable("text/plain"), None);
    }
}
