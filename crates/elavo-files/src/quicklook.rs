//! Quick Look: a large preview over the folder, toggled with Space. It
//! follows the selection, so the arrow keys step through items while it is
//! open; Space or Escape closes it. Images, text, PDFs (their first pages,
//! through `pdftoppm`), and audio or video (where GTK can play them) show
//! as themselves; anything else shows its thumbnail or icon.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use elavo_files_core::{eapp, entry, format};
use gtk::prelude::*;
use gtk::{gdk, gio, glib};

use crate::i18n::tr;
use crate::thumbs;

const TEXT_BYTES: usize = 512 * 1024;
const PDF_PAGES: u32 = 3;

type Callback = RefCell<Option<Box<dyn Fn()>>>;

pub struct QuickLook {
    pub root: gtk::Box,
    title: gtk::Label,
    open: gtk::Button,
    stack: gtk::Stack,
    picture: gtk::Picture,
    text: gtk::TextView,
    pages: gtk::Box,
    video: gtk::Video,
    icon: gtk::Image,
    meta: gtk::Label,
    uri: RefCell<Option<String>>,
    generation: Cell<u32>,
    on_open: Callback,
    on_close: Callback,
}

fn bar_button(icon: Option<&str>, label: Option<&str>) -> gtk::Button {
    let b = gtk::Button::builder()
        .focusable(false)
        .focus_on_click(false)
        .valign(gtk::Align::Center)
        .css_classes(["files-quicklook-button"])
        .build();
    if let Some(icon) = icon {
        b.set_icon_name(icon);
    }
    if let Some(label) = label {
        b.set_label(label);
    }
    b
}

/// Can GTK play media here (its GStreamer backend)?
fn media_backend(file: &gio::File) -> Option<gtk::MediaFile> {
    let media = gtk::MediaFile::for_file(file);
    (!media.type_().name().contains("NoMedia")).then_some(media)
}

impl QuickLook {
    pub fn new() -> Rc<Self> {
        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .visible(false)
            .margin_top(28)
            .margin_bottom(28)
            .margin_start(40)
            .margin_end(40)
            .css_classes(["files-quicklook"])
            .build();
        let close = bar_button(Some("window-close-symbolic"), None);
        close.set_tooltip_text(Some(&tr("Close (Space)")));
        let title = gtk::Label::builder()
            .hexpand(true)
            .ellipsize(gtk::pango::EllipsizeMode::Middle)
            .css_classes(["files-quicklook-title"])
            .build();
        let open = bar_button(None, Some(&tr("Open")));
        let bar = gtk::Box::builder()
            .spacing(8)
            .css_classes(["files-quicklook-bar"])
            .build();
        bar.append(&close);
        bar.append(&title);
        bar.append(&open);

        let picture = gtk::Picture::builder()
            .content_fit(gtk::ContentFit::Contain)
            .can_shrink(true)
            .hexpand(true)
            .vexpand(true)
            .build();
        let text = gtk::TextView::builder()
            .editable(false)
            .cursor_visible(false)
            .focusable(false)
            .monospace(true)
            .wrap_mode(gtk::WrapMode::WordChar)
            .top_margin(14)
            .bottom_margin(14)
            .left_margin(16)
            .right_margin(16)
            .css_classes(["files-quicklook-text"])
            .build();
        let text_scroll = gtk::ScrolledWindow::builder()
            .child(&text)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .build();
        let pages = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(14)
            .margin_top(14)
            .margin_bottom(14)
            .margin_start(14)
            .margin_end(14)
            .build();
        let pages_scroll = gtk::ScrolledWindow::builder()
            .child(&pages)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .build();
        let video = gtk::Video::builder()
            .autoplay(true)
            .hexpand(true)
            .vexpand(true)
            .build();
        let icon = gtk::Image::builder()
            .pixel_size(256)
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .build();
        let stack = gtk::Stack::builder()
            .hexpand(true)
            .vexpand(true)
            .css_classes(["files-quicklook-stage"])
            .build();
        stack.add_named(&icon, Some("icon"));
        stack.add_named(&picture, Some("picture"));
        stack.add_named(&text_scroll, Some("text"));
        stack.add_named(&pages_scroll, Some("pages"));
        stack.add_named(&video, Some("video"));
        let meta = gtk::Label::builder()
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .css_classes(["files-quicklook-meta", "files-dim"])
            .build();
        root.append(&bar);
        root.append(&stack);
        root.append(&meta);
        let this = Rc::new(Self {
            root,
            title,
            open,
            stack,
            picture,
            text,
            pages,
            video,
            icon,
            meta,
            uri: RefCell::default(),
            generation: Cell::new(0),
            on_open: RefCell::default(),
            on_close: RefCell::default(),
        });
        let weak = Rc::downgrade(&this);
        close.connect_clicked(move |_| {
            if let Some(q) = weak.upgrade() {
                if let Some(f) = q.on_close.borrow().as_ref() {
                    f();
                }
            }
        });
        let weak = Rc::downgrade(&this);
        this.open.connect_clicked(move |_| {
            if let Some(q) = weak.upgrade() {
                if let Some(f) = q.on_open.borrow().as_ref() {
                    f();
                }
            }
        });
        this
    }

    pub fn connect_open(&self, f: impl Fn() + 'static) {
        self.on_open.replace(Some(Box::new(f)));
    }

    pub fn connect_close(&self, f: impl Fn() + 'static) {
        self.on_close.replace(Some(Box::new(f)));
    }

    pub fn is_open(&self) -> bool {
        self.root.is_visible()
    }

    fn stop_media(&self) {
        if let Some(stream) = self.video.media_stream() {
            stream.set_playing(false);
        }
        self.video.set_media_stream(None::<&gtk::MediaStream>);
    }

    pub fn hide(&self) {
        self.generation.set(self.generation.get().wrapping_add(1));
        self.stop_media();
        self.uri.replace(None);
        self.root.set_visible(false);
    }

    /// Show `file`, unless it is what is shown already.
    pub fn show(self: &Rc<Self>, file: &gio::File, info: &gio::FileInfo) {
        let uri = file.uri().to_string();
        if self.is_open() && self.uri.borrow().as_deref() == Some(uri.as_str()) {
            return;
        }
        self.uri.replace(Some(uri));
        let generation = self.generation.get().wrapping_add(1);
        self.generation.set(generation);
        self.stop_media();
        self.root.set_visible(true);

        self.title.set_label(&entry::title(info));
        let dir = entry::is_dir(info);
        let kind = entry::kind(info);
        let mut meta = vec![kind];
        if !dir {
            meta.push(format::size(entry::size(info)));
        }
        let modified = entry::modified(info);
        if modified > 0 {
            meta.push(format!("{} {}", tr("Modified"), entry::date_text(modified)));
        }
        self.meta.set_label(&meta.join("  \u{b7}  "));

        let ct = entry::content_type(info);
        let opener = if eapp::is_marked(info) || dir {
            None
        } else {
            gio::AppInfo::default_for_type(&ct, false)
        };
        self.open.set_label(&match &opener {
            Some(app) => format!("{} {}", tr("Open with"), app.display_name()),
            None => tr("Open"),
        });

        let gicon = info
            .icon()
            .unwrap_or_else(|| gio::content_type_get_icon(&ct));
        self.icon.set_from_gicon(&gicon);
        self.stack.set_visible_child_name("icon");
        if dir {
            return;
        }
        let size = entry::size(info);
        if ct.starts_with("video/") || ct.starts_with("audio/") {
            if let Some(media) = media_backend(file) {
                self.video.set_media_stream(Some(&media));
                self.stack.set_visible_child_name("video");
                return;
            }
        }
        let (file, info, weak) = (file.clone(), info.clone(), Rc::downgrade(self));
        glib::spawn_future_local(async move {
            let current = |q: &Self| q.generation.get() == generation;
            let wanted = || weak.upgrade().is_some_and(|q| current(&q));
            if gio::content_type_is_a(&ct, "text/plain") {
                if let Some(text) = read_text(&file).await {
                    if let Some(q) = weak.upgrade().filter(|q| current(q)) {
                        q.text.buffer().set_text(&text);
                        q.stage_text_top();
                        q.stack.set_visible_child_name("text");
                    }
                    return;
                }
            }
            if ct == "application/pdf" {
                let pages = pdf_pages(&file, generation).await;
                if !pages.is_empty() {
                    if let Some(q) = weak.upgrade().filter(|q| current(q)) {
                        while let Some(c) = q.pages.first_child() {
                            q.pages.remove(&c);
                        }
                        for page in pages {
                            q.pages.append(
                                &gtk::Picture::builder()
                                    .paintable(&page)
                                    .can_shrink(true)
                                    .content_fit(gtk::ContentFit::Contain)
                                    .css_classes(["files-quicklook-page"])
                                    .build(),
                            );
                        }
                        q.stack.set_visible_child_name("pages");
                    }
                    return;
                }
            }
            let texture = if ct.starts_with("image/") && size <= thumbs::IMAGE_MAX_BYTES {
                thumbs::image(&file, 2048).await
            } else {
                thumbs::texture(&file, Some(&info), 512, &wanted).await
            };
            if let (Some(t), Some(q)) = (texture, weak.upgrade().filter(|q| current(q))) {
                q.picture.set_paintable(Some(&t));
                q.stack.set_visible_child_name("picture");
            }
        });
    }

    fn stage_text_top(&self) {
        let buffer = self.text.buffer();
        let mut start = buffer.start_iter();
        self.text.scroll_to_iter(&mut start, 0.0, false, 0.0, 0.0);
    }
}

async fn read_text(file: &gio::File) -> Option<String> {
    let stream = file.read_future(glib::Priority::DEFAULT).await.ok()?;
    let bytes = stream
        .read_bytes_future(TEXT_BYTES, glib::Priority::DEFAULT)
        .await
        .ok()?;
    let text = String::from_utf8_lossy(&bytes);
    if text.chars().take(4096).filter(|c| *c == '\u{fffd}').count() > 8 {
        return None;
    }
    Some(text.into_owned())
}

/// The first pages of a PDF, drawn by poppler's `pdftoppm` into a private
/// cache folder and read back; the image files are removed straight away.
async fn pdf_pages(file: &gio::File, generation: u32) -> Vec<gdk::Texture> {
    let Some(path) = file.path() else {
        return Vec::new();
    };
    if glib::find_program_in_path("pdftoppm").is_none() {
        return Vec::new();
    }
    let dir = glib::user_cache_dir()
        .join("elavo-files")
        .join(format!("quicklook-{}", std::process::id()));
    if std::fs::create_dir_all(&dir).is_err() {
        return Vec::new();
    }
    let prefix = dir.join(format!("p{generation}"));
    let last = PDF_PAGES.to_string();
    let args: Vec<&std::ffi::OsStr> = vec![
        "pdftoppm".as_ref(),
        "-png".as_ref(),
        "-f".as_ref(),
        "1".as_ref(),
        "-l".as_ref(),
        last.as_ref(),
        "-scale-to".as_ref(),
        "1400".as_ref(),
        "--".as_ref(),
        path.as_os_str(),
        prefix.as_os_str(),
    ];
    let Ok(sub) = gio::Subprocess::newv(
        &args,
        gio::SubprocessFlags::STDOUT_SILENCE | gio::SubprocessFlags::STDERR_SILENCE,
    ) else {
        return Vec::new();
    };
    let _ = sub.wait_future().await;
    let stem = format!("p{generation}-");
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .map(|d| {
            d.filter_map(Result::ok)
                .map(|e| e.path())
                .filter(|p| {
                    p.file_name()
                        .is_some_and(|n| n.to_string_lossy().starts_with(&stem))
                })
                .collect()
        })
        .unwrap_or_default();
    files.sort();
    let pages = files
        .iter()
        .filter_map(|p| gdk::Texture::from_filename(p).ok())
        .collect();
    for p in &files {
        let _ = std::fs::remove_file(p);
    }
    let _ = std::fs::remove_dir(&dir);
    pages
}
