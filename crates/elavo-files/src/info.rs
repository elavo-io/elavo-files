//! Get Info: one window per item, as in Finder. The filesystem attributes
//! and folder size load asynchronously. Renaming never replaces an item;
//! an installed app bundle can't be renamed here. Changing Open With
//! changes the default for every file of that type, not just this one.

use std::cell::RefCell;
use std::rc::Rc;

use elavo_files_core::{eapp, entry, format, naming};
use gtk::prelude::*;
use gtk::{gio, glib};

use crate::i18n::tr;

const ATTRIBUTES: &str = "standard::*,time::*,owner::*,unix::*,access::*";

fn value(text: &str) -> gtk::Label {
    gtk::Label::builder()
        .label(text)
        .xalign(0.0)
        .wrap(true)
        .wrap_mode(gtk::pango::WrapMode::WordChar)
        .selectable(true)
        .hexpand(true)
        .build()
}

fn row(grid: &gtk::Grid, title: &str, text: &str) -> gtk::Label {
    let n = grid.observe_children().n_items() as i32 / 2;
    let label = gtk::Label::builder()
        .label(title)
        .xalign(1.0)
        .valign(gtk::Align::Start)
        .css_classes(["files-dim"])
        .build();
    let val = value(text);
    grid.attach(&label, 0, n, 1, 1);
    grid.attach(&val, 1, n, 1, 1);
    val
}

fn section(title: &str, grid: &gtk::Grid) -> gtk::Expander {
    gtk::Expander::builder()
        .label(title)
        .expanded(true)
        .child(grid)
        .css_classes(["files-info-section"])
        .build()
}

fn grid() -> gtk::Grid {
    gtk::Grid::builder()
        .column_spacing(14)
        .row_spacing(6)
        .margin_top(10)
        .margin_bottom(8)
        .margin_start(10)
        .margin_end(2)
        .build()
}

/// The permissions a Unix mode grants one class, in Get Info's words.
fn permission(bits: u32) -> String {
    let base = match bits & 6 {
        6 => tr("Read & Write"),
        4 => tr("Read only"),
        2 => tr("Write only"),
        _ => tr("No access"),
    };
    if bits & 1 != 0 {
        format!("{base}, {}", tr("execute"))
    } else {
        base
    }
}

/// A long local date, in the person's time zone.
fn date(info: &gio::FileInfo, attr: &str) -> String {
    let secs = info.attribute_uint64(attr) as i64;
    if secs <= 0 {
        return tr("Not available");
    }
    glib::DateTime::from_unix_local(secs)
        .ok()
        .and_then(|d| d.format("%B %-d, %Y at %-I:%M %p %Z").ok())
        .map(|s| s.to_string())
        .unwrap_or_else(|| tr("Not available"))
}

#[allow(deprecated)]
fn open_with(window: &gtk::Window, file: &gio::File, info: &gio::FileInfo) -> gtk::Box {
    let content_type = entry::content_type(info);
    let default = gio::AppInfo::default_for_type(&content_type, false)
        .map(|a| a.display_name().to_string())
        .unwrap_or_else(|| tr("Choose an application…"));
    let root = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(8)
        .margin_top(8)
        .margin_bottom(10)
        .margin_start(10)
        .build();
    let button = gtk::Button::with_label(&default);
    button.set_halign(gtk::Align::Fill);
    let hint = value(&tr("Use this application to open all files of this kind."));
    hint.add_css_class("files-dim");
    root.append(&button);
    root.append(&hint);
    let (window, file) = (window.downgrade(), file.clone());
    button.connect_clicked(move |button| {
        let Some(window) = window.upgrade() else {
            return;
        };
        let chooser = gtk::AppChooserDialog::for_content_type(
            Some(&window),
            gtk::DialogFlags::MODAL | gtk::DialogFlags::DESTROY_WITH_PARENT,
            &content_type,
        );
        chooser.set_heading(&tr("Choose the default application for this kind of file"));
        let (ct, button, window, file) = (
            content_type.clone(),
            button.clone(),
            window.clone(),
            file.clone(),
        );
        chooser.connect_response(move |d, response| {
            if response == gtk::ResponseType::Ok {
                if let Some(app) = d.app_info() {
                    match app.set_as_default_for_type(&ct) {
                        Ok(()) => {
                            button.set_label(&app.display_name());
                            eprintln!("elavo-files: default app for {ct} changed ({} is open in Get Info)", file.uri());
                        }
                        Err(e) => crate::dialogs::error(
                            &window,
                            &tr("Couldn't change the default application"),
                            e.message(),
                        ),
                    }
                }
            }
            d.destroy();
        });
        chooser.present();
    });
    root
}

/// The version of an installed application, if the files or its package
/// manager give one. Looking at Flatpak/RPM is asynchronous and read-only.
async fn app_version(path: &std::path::Path, details: &eapp::Details) -> Option<String> {
    let key = glib::KeyFile::new();
    if key
        .load_from_file(path.join("launch.desktop"), glib::KeyFileFlags::NONE)
        .is_ok()
    {
        for k in ["X-AppImage-Version", "X-Elavo-Version"] {
            if let Ok(v) = key.string("Desktop Entry", k) {
                if !v.is_empty() {
                    return Some(v.to_string());
                }
            }
        }
    }
    // entry.json may include a version in newer bundles.
    if let Ok(text) = std::fs::read_to_string(path.join("entry.json")) {
        if let Some(v) = eapp::json_field(&text, "version").filter(|v| !v.is_empty()) {
            return Some(v);
        }
    }
    if details.kind == "appimage" {
        if let Some(v) = eapp::version_in(&details.source) {
            return Some(v);
        }
    }
    if details.kind == "flatpak" {
        let launcher = gio::SubprocessLauncher::new(
            gio::SubprocessFlags::STDOUT_PIPE | gio::SubprocessFlags::STDERR_SILENCE,
        );
        launcher.setenv("LC_ALL", "C", true);
        let sub = launcher
            .spawn(&["flatpak".as_ref(), "info".as_ref(), details.id.as_ref()])
            .ok()?;
        let (text, _) = sub.communicate_utf8_future(None).await.ok()?;
        if sub.is_successful() {
            return text?
                .lines()
                .find_map(|l| l.trim().strip_prefix("Version:"))
                .map(|v| v.trim().to_owned())
                .filter(|v| !v.is_empty());
        }
    }
    None
}

/// Show the info window immediately, then fill it as the metadata arrives.
pub fn present(parent: &gtk::ApplicationWindow, file: &gio::File) {
    let name = file
        .basename()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| file.uri().to_string());
    let window = gtk::Window::builder()
        .application(&parent.application().expect("Files application"))
        .title(format!("{name} {}", tr("Info")))
        .default_width(368)
        .default_height(624)
        .css_classes(["background", "files-info"])
        .build();
    let header = gtk::HeaderBar::new();
    window.set_titlebar(Some(&header));
    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .margin_top(16)
        .margin_bottom(16)
        .margin_start(16)
        .margin_end(16)
        .build();
    let loading = value(&tr("Loading information…"));
    content.append(&loading);
    window.set_child(Some(
        &gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .child(&content)
            .build(),
    ));
    let cancel = gio::Cancellable::new();
    let c = cancel.clone();
    window.connect_close_request(move |_| {
        c.cancel();
        glib::Propagation::Proceed
    });
    let keys = gtk::ShortcutController::new();
    keys.add_shortcut(gtk::Shortcut::new(
        gtk::ShortcutTrigger::parse_string("<Control>w|Escape"),
        Some(gtk::CallbackAction::new(|w, _| {
            if let Some(w) = w.downcast_ref::<gtk::Window>() {
                w.close();
            }
            glib::Propagation::Stop
        })),
    ));
    window.add_controller(keys);
    window.present();
    let (file, weak_window) = (file.clone(), window.downgrade());
    glib::spawn_future_local(async move {
        let result = file
            .query_info_future(
                ATTRIBUTES,
                gio::FileQueryInfoFlags::NOFOLLOW_SYMLINKS,
                glib::Priority::DEFAULT,
            )
            .await;
        let Some(window) = weak_window.upgrade().filter(|_| !cancel.is_cancelled()) else {
            return;
        };
        let info = match result {
            Ok(info) => info,
            Err(e) => {
                loading.set_label(e.message());
                return;
            }
        };
        content.remove(&loading);
        info.set_attribute_object("standard::file", &file);
        let bundle = eapp::decorate(&info, &crate::views::themed(&content.display()));
        let top = gtk::Box::builder().spacing(12).build();
        let icon = gtk::Image::builder().pixel_size(64).build();
        if let Some(i) = info.icon() {
            icon.set_from_gicon(&i);
        }
        top.append(&icon);
        let title_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(4)
            .hexpand(true)
            .build();
        let name = value(&entry::name(&info));
        name.add_css_class("files-info-name");
        let size = value(&format::size_exact(entry::size(&info)));
        size.add_css_class("files-dim");
        title_box.append(&name);
        title_box.append(&size);
        top.append(&title_box);
        content.append(&top);

        let general = grid();
        row(&general, &tr("Kind:"), &entry::kind(&info));
        let size_row = row(
            &general,
            &tr("Size:"),
            &if entry::is_dir(&info) {
                tr("Calculating…")
            } else {
                format::size_exact(entry::size(&info))
            },
        );
        row(
            &general,
            &tr("Where:"),
            &file
                .parent()
                .map(|p| p.parse_name().to_string())
                .unwrap_or_default(),
        );
        if info.is_symlink() {
            row(
                &general,
                &tr("Original:"),
                &info
                    .symlink_target()
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_default(),
            );
        }
        for (label, attr) in [
            (tr("Created:"), "time::created"),
            (tr("Modified:"), "time::modified"),
            (tr("Last opened:"), "time::access"),
        ] {
            row(&general, &label, &date(&info, attr));
        }
        let version = if bundle {
            let path = file.path().unwrap();
            if let Some(d) = eapp::details(&path) {
                row(&general, &tr("Application ID:"), &d.id);
                row(
                    &general,
                    &tr("Installed as:"),
                    &tr(eapp::kind_label(&d.kind)),
                );
                if !d.origin.is_empty() {
                    row(&general, &tr("From:"), &d.origin);
                }
                let label = row(&general, &tr("Version:"), &tr("Checking…"));
                Some((path, d, label))
            } else {
                None
            }
        } else {
            None
        };
        content.append(&section(&tr("General"), &general));

        let rename_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(8)
            .margin_top(8)
            .margin_bottom(8)
            .margin_start(10)
            .build();
        let rename = gtk::Entry::builder()
            .text(info.name().to_string_lossy().as_ref())
            .hexpand(true)
            .build();
        let rename_button = gtk::Button::with_label(&tr("Rename"));
        rename_button.set_halign(gtk::Align::End);
        let writable = info.boolean("access::can-rename")
            && !file.path().is_some_and(|p| eapp::is_install(&p));
        rename.set_sensitive(writable);
        rename_button.set_sensitive(writable);
        rename_box.append(&rename);
        rename_box.append(&rename_button);
        let current = Rc::new(RefCell::new(file.clone()));
        let (entry, b, w, c, title) = (
            rename.clone(),
            rename_button.clone(),
            window.downgrade(),
            current.clone(),
            name.clone(),
        );
        let rename_action: Rc<dyn Fn()> = Rc::new(move || {
            let Some(window) = w.upgrade() else {
                return;
            };
            let new = entry.text().to_string();
            if let Err(e) = naming::check(&new) {
                crate::dialogs::error(&window, &tr("That name can't be used"), &tr(e.message()));
                return;
            }
            let file = c.borrow().clone();
            if file.basename().is_some_and(|n| n.to_string_lossy() == new) {
                return;
            }
            if file
                .parent()
                .is_some_and(|p| p.child(&new).query_exists(gio::Cancellable::NONE))
            {
                crate::dialogs::error(
                    &window,
                    &tr("That name is already taken"),
                    &tr("Please choose a different name."),
                );
                return;
            }
            b.set_sensitive(false);
            let (c, b, title, asked) = (c.clone(), b.clone(), title.clone(), new.clone());
            file.set_display_name_async(
                &asked,
                glib::Priority::DEFAULT,
                gio::Cancellable::NONE,
                move |result| {
                    b.set_sensitive(true);
                    match result {
                        Ok(file) => {
                            window.set_title(Some(&format!("{new} {}", tr("Info"))));
                            title.set_label(&new);
                            c.replace(file);
                        }
                        Err(e) => crate::dialogs::error(
                            &window,
                            &tr("Couldn't rename the item"),
                            e.message(),
                        ),
                    }
                },
            );
        });
        let f = rename_action.clone();
        rename_button.connect_clicked(move |_| f());
        rename.connect_activate(move |_| rename_action());
        content.append(
            &gtk::Expander::builder()
                .label(tr("Name & Extension"))
                .expanded(true)
                .child(&rename_box)
                .css_classes(["files-info-section"])
                .build(),
        );

        if !entry::is_dir(&info) {
            content.append(
                &gtk::Expander::builder()
                    .label(tr("Open with:"))
                    .expanded(true)
                    .child(&open_with(&window, &file, &info))
                    .css_classes(["files-info-section"])
                    .build(),
            );
        }
        let perm = grid();
        let mode = info.attribute_uint32("unix::mode");
        let owner = info
            .attribute_string("owner::user-real")
            .filter(|s| !s.is_empty())
            .or_else(|| info.attribute_string("owner::user"))
            .map(|s| s.to_string())
            .unwrap_or_else(|| tr("Unknown"));
        let group = info
            .attribute_string("owner::group")
            .map(|s| s.to_string())
            .unwrap_or_else(|| tr("Unknown"));
        row(&perm, &tr("Owner:"), &owner);
        row(&perm, &tr("Group:"), &group);
        if info.has_attribute("unix::mode") {
            row(&perm, &tr("Owner access:"), &permission((mode >> 6) & 7));
            row(&perm, &tr("Group access:"), &permission((mode >> 3) & 7));
            row(&perm, &tr("Everyone:"), &permission(mode & 7));
            row(
                &perm,
                &tr("Permissions:"),
                &format!("{:04o}", mode & 0o7777),
            );
        } else {
            row(
                &perm,
                &tr("Access:"),
                &tr("Not available on this filesystem"),
            );
        }
        content.append(&section(&tr("Sharing & Permissions"), &perm));
        if entry::is_dir(&info) {
            let (size, size_row) = (size.downgrade(), size_row.downgrade());
            file.measure_disk_usage_async(
                gio::FileMeasureFlags::APPARENT_SIZE,
                glib::Priority::LOW,
                Some(&cancel),
                None,
                move |result| match result {
                    Ok((bytes, dirs, files)) => {
                        let text = format!(
                            "{} {} {}",
                            format::size_exact(bytes),
                            tr("for"),
                            format::grouped(dirs.saturating_sub(1) + files)
                        );
                        if let Some(l) = size.upgrade() {
                            l.set_label(&format::size(bytes));
                        }
                        if let Some(l) = size_row.upgrade() {
                            l.set_label(&format!("{text} {}", tr("items")));
                        }
                    }
                    Err(e) if !e.matches(gio::IOErrorEnum::Cancelled) => {
                        if let Some(l) = size_row.upgrade() {
                            l.set_label(e.message());
                        }
                    }
                    _ => {}
                },
            );
        }
        if let Some((path, details, label)) = version {
            let version = app_version(&path, &details).await;
            label.set_label(&version.unwrap_or_else(|| tr("Not available")));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permission_text() {
        assert_eq!(permission(6), "Read & Write");
        assert_eq!(permission(4), "Read only");
        assert_eq!(permission(0), "No access");
        assert_eq!(permission(7), "Read & Write, execute");
    }
}
