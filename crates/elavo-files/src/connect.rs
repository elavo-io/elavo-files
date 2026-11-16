//! Connect to Server (Ctrl+K), as in Finder: an address, the recent
//! servers under it, and Connect. A server that wants a name and password
//! asks through GTK's mount operation, which offers to remember them in
//! the keyring; Files itself never stores or logs a password.

use std::rc::Rc;

use elavo_files_core::servers;
use gtk::prelude::*;
use gtk::{gio, glib};

use crate::i18n::tr;
use crate::mounts;

type OpenFn = Rc<dyn Fn(gio::File)>;

struct Dialog {
    window: gtk::Window,
    entry: gtk::Entry,
    error: gtk::Label,
    status: gtk::Label,
    spinner: gtk::Spinner,
    connect: gtk::Button,
    recent: gtk::ListBox,
    recent_box: gtk::Box,
    open: OpenFn,
}

/// Show Connect to Server over `parent`; `open` shows what it connects to.
pub fn present(parent: &gtk::ApplicationWindow, open: impl Fn(gio::File) + 'static) {
    let window = gtk::Window::builder()
        .modal(true)
        .resizable(false)
        .title(tr("Connect to Server"))
        .default_width(480)
        .css_classes(["background", "files-dialog", "files-connect"])
        .build();
    window.set_transient_for(Some(parent));
    window.set_titlebar(Some(&gtk::Box::new(gtk::Orientation::Horizontal, 0)));

    let body = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(10)
        .margin_top(20)
        .margin_bottom(16)
        .margin_start(20)
        .margin_end(20)
        .build();
    body.append(
        &gtk::Label::builder()
            .label(tr("Connect to Server"))
            .xalign(0.0)
            .css_classes(["files-dialog-title"])
            .build(),
    );
    let entry = gtk::Entry::builder()
        .placeholder_text("smb://server/share")
        .activates_default(true)
        .hexpand(true)
        .input_purpose(gtk::InputPurpose::Url)
        .build();
    entry.update_property(&[gtk::accessible::Property::Label(&tr("Server Address"))]);
    let row = gtk::Box::builder().spacing(8).build();
    row.append(
        &gtk::Label::builder()
            .label(tr("Server Address:"))
            .css_classes(["files-dim"])
            .build(),
    );
    row.append(&entry);
    body.append(&row);
    body.append(
        &gtk::Label::builder()
            .label(tr(
                "SMB, SFTP, FTP, WebDAV (dav://, davs://) or NFS. A name on its own connects with SMB.",
            ))
            .xalign(0.0)
            .wrap(true)
            .max_width_chars(56)
            .css_classes(["files-dim", "files-connect-hint"])
            .build(),
    );
    let error = gtk::Label::builder()
        .xalign(0.0)
        .wrap(true)
        .max_width_chars(56)
        .visible(false)
        .selectable(true)
        .css_classes(["error", "files-connect-error"])
        .build();
    body.append(&error);

    let recent_box = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .margin_top(4)
        .build();
    recent_box.append(
        &gtk::Label::builder()
            .label(tr("Recent Servers"))
            .xalign(0.0)
            .css_classes(["files-section-title"])
            .build(),
    );
    let recent = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::Single)
        .activate_on_single_click(false)
        .css_classes(["files-connect-recent"])
        .build();
    recent_box.append(
        &gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_height(true)
            .max_content_height(196)
            .child(&recent)
            .css_classes(["files-connect-recent-scroll"])
            .build(),
    );
    body.append(&recent_box);

    let buttons = gtk::Box::builder().spacing(8).margin_top(6).build();
    let spinner = gtk::Spinner::builder().visible(false).build();
    let status = gtk::Label::builder()
        .xalign(0.0)
        .hexpand(true)
        .ellipsize(gtk::pango::EllipsizeMode::Middle)
        .css_classes(["files-dim"])
        .build();
    let cancel = gtk::Button::with_label(&tr("Cancel"));
    let connect = gtk::Button::builder()
        .label(tr("Connect"))
        .css_classes(["suggested-action"])
        .build();
    buttons.append(&spinner);
    buttons.append(&status);
    buttons.append(&cancel);
    buttons.append(&connect);
    body.append(&buttons);
    window.set_child(Some(&body));
    window.set_default_widget(Some(&connect));

    let d = Rc::new(Dialog {
        window: window.clone(),
        entry: entry.clone(),
        error,
        status,
        spinner,
        connect: connect.clone(),
        recent: recent.clone(),
        recent_box,
        open: Rc::new(open),
    });
    d.fill_recent();
    if let Some(first) = servers::recent().first() {
        entry.set_text(first);
        entry.select_region(0, -1);
    }

    let w = window.downgrade();
    cancel.connect_clicked(move |_| {
        if let Some(w) = w.upgrade() {
            w.close();
        }
    });
    let dd = d.clone();
    connect.connect_clicked(move |_| dd.connect());
    let dd = d.clone();
    entry.connect_changed(move |_| dd.error.set_visible(false));
    let dd = d.clone();
    recent.connect_row_selected(move |_, row| {
        if let Some(row) = row {
            dd.entry.set_text(&row.widget_name());
        }
    });
    let dd = d.clone();
    recent.connect_row_activated(move |_, row| {
        dd.entry.set_text(&row.widget_name());
        dd.connect();
    });

    let keys = gtk::ShortcutController::new();
    keys.add_shortcut(gtk::Shortcut::new(
        gtk::ShortcutTrigger::parse_string("Escape"),
        Some(gtk::CallbackAction::new(|w, _| {
            if let Some(w) = w.downcast_ref::<gtk::Window>() {
                w.close();
            }
            glib::Propagation::Stop
        })),
    ));
    window.add_controller(keys);
    window.present();
    entry.grab_focus();
}

impl Dialog {
    fn fill_recent(self: &Rc<Self>) {
        while let Some(child) = self.recent.first_child() {
            self.recent.remove(&child);
        }
        let list = servers::recent();
        self.recent_box.set_visible(!list.is_empty());
        for uri in list {
            let row = gtk::ListBoxRow::new();
            row.set_widget_name(&uri);
            let b = gtk::Box::builder().spacing(8).build();
            b.append(
                &gtk::Image::builder()
                    .icon_name("folder-remote-symbolic")
                    .pixel_size(16)
                    .css_classes(["files-place-icon"])
                    .build(),
            );
            let text = gtk::Box::builder()
                .orientation(gtk::Orientation::Vertical)
                .hexpand(true)
                .build();
            text.append(
                &gtk::Label::builder()
                    .label(&uri)
                    .xalign(0.0)
                    .ellipsize(gtk::pango::EllipsizeMode::Middle)
                    .build(),
            );
            text.append(
                &gtk::Label::builder()
                    .label(tr(servers::kind(&uri)))
                    .xalign(0.0)
                    .css_classes(["files-dim", "files-connect-kind"])
                    .build(),
            );
            b.append(&text);
            let remove = gtk::Button::builder()
                .icon_name("window-close-symbolic")
                .tooltip_text(tr("Remove from Recent Servers"))
                .valign(gtk::Align::Center)
                .css_classes(["flat", "circular", "files-connect-remove"])
                .build();
            let (weak, u) = (Rc::downgrade(self), uri.clone());
            remove.connect_clicked(move |_| {
                let _ = servers::forget(&u);
                if let Some(d) = weak.upgrade() {
                    d.fill_recent();
                }
            });
            b.append(&remove);
            row.set_child(Some(&b));
            self.recent.append(&row);
        }
    }

    fn busy(&self, on: bool, status: &str) {
        self.spinner.set_visible(on);
        self.spinner.set_spinning(on);
        self.status.set_label(status);
        self.connect.set_sensitive(!on);
        self.entry.set_sensitive(!on);
        self.recent.set_sensitive(!on);
    }

    fn connect(self: &Rc<Self>) {
        if !self.connect.is_sensitive() {
            return;
        }
        let uri = match servers::normalize(&self.entry.text()) {
            Ok(uri) => uri,
            Err(e) => {
                self.error.set_label(&tr(&e.to_string()));
                self.error.set_visible(true);
                return;
            }
        };
        let file = gio::File::for_uri(&uri);
        let host = servers::without_password(&uri);
        self.error.set_visible(false);
        self.busy(true, &format!("{} {host}\u{2026}", tr("Connecting to")));
        let d = self.clone();
        let f = file.clone();
        mounts::mount(Some(&self.window), &file, move |r| match r {
            Ok(()) => {
                let _ = servers::remember(&uri);
                d.busy(false, "");
                d.window.close();
                (d.open)(f);
            }
            Err(e) if mounts::quietly_cancelled(&e) => d.busy(false, ""),
            Err(e) => {
                d.busy(false, "");
                d.error.set_label(&format!(
                    "{} {host}.\n{}",
                    tr("Couldn\u{2019}t connect to"),
                    e.message()
                ));
                d.error.set_visible(true);
                d.entry.grab_focus();
            }
        });
    }
}
