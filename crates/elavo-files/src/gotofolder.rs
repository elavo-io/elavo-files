//! Go to Folder (Ctrl+Shift+G), as in Finder: type a path and go there.
//! `~` is Home, a path without `/` in front is inside the folder shown,
//! and a URI (`smb://`, `trash:///`) works too. A file opens its folder
//! with the file selected.

use std::cell::RefCell;

use gtk::prelude::*;
use gtk::{gio, glib};

use crate::i18n::tr;

thread_local! {
    /// What was typed last time, offered again like Finder does.
    static LAST: RefCell<String> = const { RefCell::new(String::new()) };
}

/// Where `text` points, seen from `here`.
pub fn resolve(text: &str, here: &gio::File) -> Option<gio::File> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    if text == "~" {
        return Some(gio::File::for_path(glib::home_dir()));
    }
    if let Some(rest) = text.strip_prefix("~/") {
        return Some(gio::File::for_path(glib::home_dir().join(rest)));
    }
    if text.starts_with('/') {
        return Some(gio::File::for_path(text));
    }
    if let Some((scheme, _)) = text.split_once(':') {
        let is_scheme = scheme
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic())
            && scheme
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
        if is_scheme {
            return Some(gio::File::for_uri(text));
        }
    }
    Some(here.resolve_relative_path(text))
}

/// Show Go to Folder over `parent`. `open` gets the folder to show and the
/// items to select in it.
pub fn present(
    parent: &gtk::ApplicationWindow,
    here: gio::File,
    open: impl Fn(gio::File, Vec<gio::File>) + 'static,
) {
    let window = gtk::Window::builder()
        .modal(true)
        .resizable(false)
        .title(tr("Go to Folder"))
        .default_width(440)
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
            .label(tr("Go to the folder:"))
            .xalign(0.0)
            .css_classes(["files-dialog-title"])
            .build(),
    );
    let entry = gtk::Entry::builder()
        .placeholder_text("~/Documents")
        .activates_default(true)
        .hexpand(true)
        .build();
    entry.update_property(&[gtk::accessible::Property::Label(&tr("Folder"))]);
    body.append(&entry);
    let error = gtk::Label::builder()
        .xalign(0.0)
        .wrap(true)
        .max_width_chars(52)
        .visible(false)
        .css_classes(["error", "files-connect-error"])
        .build();
    body.append(&error);

    let buttons = gtk::Box::builder()
        .spacing(8)
        .margin_top(6)
        .halign(gtk::Align::End)
        .build();
    let cancel = gtk::Button::with_label(&tr("Cancel"));
    let go = gtk::Button::builder()
        .label(tr("Go"))
        .css_classes(["suggested-action"])
        .build();
    buttons.append(&cancel);
    buttons.append(&go);
    body.append(&buttons);
    window.set_child(Some(&body));
    window.set_default_widget(Some(&go));

    let last = LAST.with(|l| l.borrow().clone());
    if !last.is_empty() {
        entry.set_text(&last);
        entry.select_region(0, -1);
    }

    let w = window.downgrade();
    cancel.connect_clicked(move |_| {
        if let Some(w) = w.upgrade() {
            w.close();
        }
    });
    let err = error.clone();
    entry.connect_changed(move |_| err.set_visible(false));
    let (w, e) = (window.downgrade(), entry.downgrade());
    go.connect_clicked(move |_| {
        let (Some(w), Some(e)) = (w.upgrade(), e.upgrade()) else {
            return;
        };
        let text = e.text().to_string();
        let Some(file) = resolve(&text, &here) else {
            return;
        };
        LAST.with(|l| l.replace(text.trim().to_owned()));
        match file.query_file_type(gio::FileQueryInfoFlags::NONE, gio::Cancellable::NONE) {
            gio::FileType::Directory | gio::FileType::Mountable => open(file, Vec::new()),
            gio::FileType::Unknown if file.is_native() => {
                error.set_label(&tr("The folder can\u{2019}t be found."));
                error.set_visible(true);
                return;
            }
            // Not mounted yet: the window mounts it on the way.
            gio::FileType::Unknown => open(file, Vec::new()),
            _ => match file.parent() {
                Some(parent) => open(parent, vec![file]),
                None => open(file, Vec::new()),
            },
        }
        w.close();
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_resolve_like_finder() {
        let here = gio::File::for_path("/tmp/here");
        let home = glib::home_dir();
        assert_eq!(resolve("~", &here).unwrap().path().unwrap(), home);
        assert_eq!(
            resolve("~/Documents", &here).unwrap().path().unwrap(),
            home.join("Documents")
        );
        assert_eq!(
            resolve("/usr/share", &here).unwrap().path().unwrap(),
            std::path::PathBuf::from("/usr/share")
        );
        assert_eq!(
            resolve("sub/dir", &here).unwrap().path().unwrap(),
            std::path::PathBuf::from("/tmp/here/sub/dir")
        );
        assert_eq!(resolve("trash:///", &here).unwrap().uri(), "trash:///");
        assert!(resolve("  ", &here).is_none());
    }
}
