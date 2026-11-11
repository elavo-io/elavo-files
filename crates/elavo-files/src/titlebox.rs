//! The folder name in the toolbar, as in Finder.
//!
//! - A left click slides a small folder icon in on the name's left (the
//!   proxy icon). It drags the folder like any file. When the pointer leaves
//!   without using it, the icon slides away again and the name moves back.
//! - A right click lists the folder and the ones it is in, then the drive
//!   and the computer, with a check on the current one. Picking one goes
//!   there.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use elavo_files_core::places;
use gtk::prelude::*;
use gtk::{gdk, gio, glib};

/// How long after the pointer leaves the proxy icon goes away.
const LINGER: Duration = Duration::from_millis(350);
const SLIDE_MS: u32 = 200;

pub struct TitleBox {
    pub root: gtk::Box,
    label: gtk::Label,
    revealer: gtk::Revealer,
    proxy: gtk::Image,
    location: RefCell<Option<gio::File>>,
    hide_later: RefCell<Option<glib::SourceId>>,
    inside: Cell<bool>,
    dragging: Cell<bool>,
    go: Box<dyn Fn(gio::File)>,
}

impl TitleBox {
    pub fn new(go: impl Fn(gio::File) + 'static) -> Rc<Self> {
        let proxy = gtk::Image::builder()
            .icon_name("folder")
            .pixel_size(18)
            .margin_end(6)
            .css_classes(["files-proxy-icon"])
            .tooltip_text(crate::i18n::tr("Drag to move or copy this folder"))
            .build();
        let revealer = gtk::Revealer::builder()
            .transition_type(gtk::RevealerTransitionType::SlideRight)
            .transition_duration(SLIDE_MS)
            .child(&proxy)
            .valign(gtk::Align::Center)
            .build();
        let label = gtk::Label::builder()
            .css_classes(["files-title"])
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        // The name and its icon take only their own width, so clicks on the
        // empty toolbar still drag the window.
        let clickable = gtk::Box::builder()
            .css_classes(["files-title-box"])
            .valign(gtk::Align::Center)
            .build();
        clickable.append(&revealer);
        clickable.append(&label);
        let root = gtk::Box::builder().hexpand(true).build();
        root.append(&clickable);

        let this = Rc::new(Self {
            root,
            label,
            revealer,
            proxy,
            location: RefCell::default(),
            hide_later: RefCell::default(),
            inside: Cell::new(false),
            dragging: Cell::new(false),
            go: Box::new(go),
        });
        this.connect(&clickable);
        this
    }

    fn connect(self: &Rc<Self>, clickable: &gtk::Box) {
        // On press, before the title bar shows its window menu.
        let menu_click = gtk::GestureClick::builder()
            .button(gdk::BUTTON_SECONDARY)
            .build();
        let weak = Rc::downgrade(self);
        menu_click.connect_pressed(move |g, _, _, _| {
            g.set_state(gtk::EventSequenceState::Claimed);
            if let Some(t) = weak.upgrade() {
                t.show_path_menu();
            }
        });
        clickable.add_controller(menu_click);
        // On release, so pressing and dragging still moves the window.
        let click = gtk::GestureClick::builder()
            .button(gdk::BUTTON_PRIMARY)
            .build();
        let weak = Rc::downgrade(self);
        click.connect_released(move |_, n, _, _| {
            if let Some(t) = weak.upgrade() {
                if n == 1 {
                    t.show_proxy();
                }
            }
        });
        clickable.add_controller(click);

        let motion = gtk::EventControllerMotion::new();
        let weak = Rc::downgrade(self);
        motion.connect_enter(move |_, _, _| {
            if let Some(t) = weak.upgrade() {
                t.inside.set(true);
                t.cancel_hide();
            }
        });
        let weak = Rc::downgrade(self);
        motion.connect_leave(move |_| {
            if let Some(t) = weak.upgrade() {
                t.inside.set(false);
                t.hide_soon();
            }
        });
        clickable.add_controller(motion);

        // The proxy icon drags the folder, the same way as a file.
        let drag = gtk::DragSource::builder()
            .actions(gdk::DragAction::COPY | gdk::DragAction::MOVE)
            .build();
        let weak = Rc::downgrade(self);
        drag.connect_prepare(move |_, _, _| {
            let t = weak.upgrade()?;
            let file = t.location.borrow().clone()?;
            file.path()?;
            let list = gdk::FileList::from_array(&[file]);
            Some(gdk::ContentProvider::for_value(&list.to_value()))
        });
        let weak = Rc::downgrade(self);
        drag.connect_drag_begin(move |source, _| {
            if let Some(t) = weak.upgrade() {
                t.dragging.set(true);
                t.cancel_hide();
                let icon = gtk::WidgetPaintable::new(Some(&t.proxy));
                source.set_icon(Some(&icon), 9, 9);
            }
        });
        let weak = Rc::downgrade(self);
        drag.connect_drag_end(move |_, _, _| {
            if let Some(t) = weak.upgrade() {
                t.dragging.set(false);
                t.hide_soon();
            }
        });
        // The title bar would take a press here for moving the window;
        // claiming it, together with the drag, keeps it for the folder.
        let hold = gtk::GestureClick::builder()
            .button(gdk::BUTTON_PRIMARY)
            .build();
        hold.connect_pressed(|g, _, _, _| {
            g.set_state(gtk::EventSequenceState::Claimed);
        });
        hold.group_with(&drag);
        self.proxy.add_controller(drag);
        self.proxy.add_controller(hold);
    }

    /// Show `title` for `location`; the proxy icon is the folder's icon.
    pub fn set(&self, title: &str, location: &gio::File, icon: Option<gio::Icon>) {
        self.label.set_label(title);
        self.location.replace(Some(location.clone()));
        match icon {
            Some(icon) => self.proxy.set_from_gicon(&icon),
            None => self.proxy.set_icon_name(Some("folder")),
        }
        // A new folder starts without it, as in Finder.
        self.cancel_hide();
        self.revealer.set_reveal_child(false);
        self.proxy.remove_css_class("shown");
    }

    fn show_proxy(&self) {
        let draggable = self
            .location
            .borrow()
            .as_ref()
            .is_some_and(|f| f.path().is_some());
        if !draggable {
            return;
        }
        self.cancel_hide();
        self.revealer.set_reveal_child(true);
        self.proxy.add_css_class("shown");
    }

    fn cancel_hide(&self) {
        if let Some(id) = self.hide_later.take() {
            id.remove();
        }
    }

    fn hide_soon(self: &Rc<Self>) {
        if !self.revealer.reveals_child() || self.dragging.get() {
            return;
        }
        self.cancel_hide();
        let weak = Rc::downgrade(self);
        let id = glib::timeout_add_local_once(LINGER, move || {
            if let Some(t) = weak.upgrade() {
                t.hide_later.take();
                if !t.inside.get() && !t.dragging.get() {
                    t.proxy.remove_css_class("shown");
                    t.revealer.set_reveal_child(false);
                }
            }
        });
        self.hide_later.replace(Some(id));
    }

    fn show_path_menu(self: &Rc<Self>) {
        let Some(current) = self.location.borrow().clone() else {
            return;
        };
        let chain = places::ancestry(&current);
        let home = gio::File::for_path(glib::home_dir());
        let rows = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let popover = gtk::Popover::builder()
            .child(&rows)
            .has_arrow(false)
            .position(gtk::PositionType::Bottom)
            .halign(gtk::Align::Start)
            .build();
        // Added, not set: a popover's own "background" class gives it the
        // theme's background.
        for class in ["menu", "files-menu", "files-path-menu"] {
            popover.add_css_class(class);
        }
        let add = |label: &str, icon: &str, target: gio::File, current: bool| {
            let check = gtk::Image::builder()
                .icon_name("object-select-symbolic")
                .pixel_size(12)
                .opacity(if current { 1.0 } else { 0.0 })
                .build();
            let line = gtk::Box::builder().spacing(7).build();
            line.append(&check);
            line.append(&gtk::Image::builder().icon_name(icon).pixel_size(16).build());
            line.append(&gtk::Label::builder().label(label).xalign(0.0).build());
            let button = gtk::Button::builder()
                .child(&line)
                .css_classes(["flat", "files-path-row"])
                .build();
            let (weak, pop) = (Rc::downgrade(self), popover.downgrade());
            button.connect_clicked(move |_| {
                if let Some(p) = pop.upgrade() {
                    p.popdown();
                }
                if let Some(t) = weak.upgrade() {
                    (t.go)(target.clone());
                }
            });
            rows.append(&button);
        };
        let root_name = places::root_drive_name();
        for (i, dir) in chain.iter().enumerate() {
            let is_root = dir.path().is_some_and(|p| p.parent().is_none());
            let (label, icon) = if is_root {
                (root_name.clone(), "drive-harddisk")
            } else if i == 0 {
                (
                    self.label.label().to_string(),
                    if dir.equal(&home) {
                        "user-home"
                    } else {
                        "folder"
                    },
                )
            } else if dir.equal(&home) {
                (display_name(dir), "user-home")
            } else {
                (display_name(dir), "folder")
            };
            add(&label, icon, dir.clone(), i == 0);
        }
        // The computer stands above the drives; Files has no view of it
        // yet, so it opens the root drive.
        add(
            &places::computer_name(),
            "computer",
            gio::File::for_path("/"),
            false,
        );
        popover.set_parent(&self.label);
        popover.set_offset(-28, 4);
        popover.connect_closed(|p| {
            let p = p.clone();
            glib::idle_add_local_once(move || p.unparent());
        });
        popover.popup();
    }
}

fn display_name(file: &gio::File) -> String {
    file.query_info(
        "standard::display-name",
        gio::FileQueryInfoFlags::NONE,
        gio::Cancellable::NONE,
    )
    .map(|i| i.display_name().to_string())
    .ok()
    .or_else(|| file.basename().map(|b| b.to_string_lossy().into_owned()))
    .unwrap_or_else(|| file.uri().to_string())
}
