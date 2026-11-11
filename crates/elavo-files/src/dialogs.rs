//! The questions Files asks: a name conflict (Keep Both, Replace, Skip,
//! Stop, with Apply to All), confirmations before something can't be
//! undone, and errors.

use elavo_files_core::ops::{Answer, Conflict, Resolution, Stat};
use elavo_files_core::{entry, format};
use gtk::glib;
use gtk::prelude::*;

use crate::i18n::tr;

fn describe(s: &Stat) -> String {
    let when = entry::date_text(s.modified);
    if s.is_dir {
        format!("{} {when}", tr("Folder, modified"))
    } else {
        format!("{}, {} {when}", format::size(s.size), tr("modified"))
    }
}

/// Ask what to do when `conflict.name` is taken. Closing the dialog stops.
pub async fn conflict(parent: Option<&gtk::ApplicationWindow>, c: &Conflict) -> Answer {
    let (tx, rx) = futures_channel::oneshot::channel::<Answer>();
    let tx = std::rc::Rc::new(std::cell::RefCell::new(Some(tx)));

    let window = gtk::Window::builder()
        .modal(true)
        .resizable(false)
        .title(tr("Name Conflict"))
        .default_width(460)
        .css_classes(["files-dialog"])
        .build();
    window.set_transient_for(parent);
    window.set_titlebar(Some(&gtk::Box::new(gtk::Orientation::Horizontal, 0)));

    let body = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(14)
        .margin_top(20)
        .margin_bottom(16)
        .margin_start(20)
        .margin_end(20)
        .build();
    let top = gtk::Box::builder().spacing(16).build();
    top.append(
        &gtk::Image::builder()
            .icon_name(if c.incoming.is_dir {
                "folder"
            } else {
                "text-x-generic"
            })
            .pixel_size(56)
            .valign(gtk::Align::Start)
            .build(),
    );
    let text = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(8)
        .hexpand(true)
        .build();
    text.append(
        &gtk::Label::builder()
            .label(format!(
                "{} \u{201c}{}\u{201d} {} \u{201c}{}\u{201d}.",
                tr("An item named"),
                c.name,
                tr("already exists in"),
                c.folder
            ))
            .wrap(true)
            .xalign(0.0)
            .max_width_chars(44)
            .css_classes(["files-dialog-title"])
            .build(),
    );
    text.append(
        &gtk::Label::builder()
            .label(if c.moving {
                tr("Do you want to replace it with the one you\u{2019}re moving?")
            } else {
                tr("Do you want to replace it with the one you\u{2019}re copying?")
            })
            .wrap(true)
            .xalign(0.0)
            .max_width_chars(44)
            .build(),
    );
    let grid = gtk::Grid::builder()
        .column_spacing(10)
        .row_spacing(2)
        .margin_top(4)
        .build();
    for (row, (label, stat)) in [
        (tr("Existing:"), &c.existing),
        (tr("Replacing with:"), &c.incoming),
    ]
    .into_iter()
    .enumerate()
    {
        grid.attach(
            &gtk::Label::builder()
                .label(label)
                .xalign(1.0)
                .css_classes(["files-dim"])
                .build(),
            0,
            row as i32,
            1,
            1,
        );
        grid.attach(
            &gtk::Label::builder()
                .label(describe(stat))
                .xalign(0.0)
                .build(),
            1,
            row as i32,
            1,
            1,
        );
    }
    text.append(&grid);
    let all = gtk::CheckButton::builder()
        .label(tr("Apply to All"))
        .visible(c.remaining > 0)
        .build();
    text.append(&all);
    top.append(&text);
    body.append(&top);

    let buttons = gtk::Box::builder().spacing(8).build();
    let add = |label: String, resolution: Resolution, classes: &[&str], pack_end: bool| {
        let b = gtk::Button::builder().label(label).build();
        for class in classes {
            b.add_css_class(class);
        }
        let (tx, window, all) = (tx.clone(), window.downgrade(), all.downgrade());
        b.connect_clicked(move |_| {
            let all = all.upgrade().is_some_and(|a| a.is_active());
            if let Some(tx) = tx.borrow_mut().take() {
                let _ = tx.send(Answer { resolution, all });
            }
            if let Some(w) = window.upgrade() {
                w.close();
            }
        });
        if pack_end {
            buttons.append(&b);
        } else {
            b.set_hexpand(true);
            b.set_halign(gtk::Align::Start);
            buttons.append(&b);
        }
        b
    };
    add(tr("Stop"), Resolution::Stop, &[], false);
    add(tr("Skip"), Resolution::Skip, &[], true);
    add(tr("Keep Both"), Resolution::KeepBoth, &[], true);
    let replace = add(
        tr("Replace"),
        Resolution::Replace,
        &["suggested-action"],
        true,
    );
    body.append(&buttons);
    window.set_child(Some(&body));
    window.set_default_widget(Some(&replace));

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
    let t = tx.clone();
    window.connect_close_request(move |_| {
        if let Some(tx) = t.borrow_mut().take() {
            let _ = tx.send(Answer {
                resolution: Resolution::Stop,
                all: false,
            });
        }
        glib::Propagation::Proceed
    });
    window.present();
    replace.grab_focus();
    rx.await.unwrap_or(Answer {
        resolution: Resolution::Stop,
        all: false,
    })
}

/// A yes-or-no question; true when the user picks `confirm`.
pub async fn confirm(
    parent: &impl IsA<gtk::Window>,
    message: &str,
    detail: &str,
    confirm: &str,
) -> bool {
    let dialog = gtk::AlertDialog::builder()
        .message(message)
        .detail(detail)
        .buttons([tr("Cancel"), confirm.to_owned()])
        .cancel_button(0)
        .default_button(1)
        .modal(true)
        .build();
    matches!(dialog.choose_future(Some(parent)).await, Ok(1))
}

pub fn error(parent: &impl IsA<gtk::Window>, message: &str, detail: &str) {
    gtk::AlertDialog::builder()
        .message(message)
        .detail(detail)
        .modal(true)
        .build()
        .show(Some(parent));
}

/// Errors from an operation, a few at a time.
pub fn errors(parent: &impl IsA<gtk::Window>, message: &str, errors: &[String]) {
    if errors.is_empty() {
        return;
    }
    let mut detail: Vec<String> = errors.iter().take(8).cloned().collect();
    if errors.len() > 8 {
        detail.push(format!("{} {}", errors.len() - 8, tr("more")));
    }
    error(parent, message, &detail.join("\n"));
}
