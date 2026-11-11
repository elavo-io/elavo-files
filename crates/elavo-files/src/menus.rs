//! The context menus, in Finder's order. Items name `win.*` actions, so
//! what is possible right now shows through each action's enabled state.

use std::collections::HashSet;

use gtk::gio;
use gtk::prelude::*;

use crate::i18n::tr;

fn item(label: &str, action: &str, accel: Option<&str>) -> gio::MenuItem {
    let item = gio::MenuItem::new(Some(label), Some(action));
    if let Some(accel) = accel {
        item.set_attribute_value("accel", Some(&accel.to_variant()));
    }
    item
}

fn section(items: &[gio::MenuItem]) -> gio::Menu {
    let menu = gio::Menu::new();
    for i in items {
        menu.append_item(i);
    }
    menu
}

/// What the menu is for.
pub struct Selection<'a> {
    pub count: usize,
    pub first_name: &'a str,
    pub content_type: &'a str,
    pub in_trash: bool,
    /// One application bundle (`.eapp`).
    pub bundle: bool,
}

fn quoted_or_count(s: &Selection) -> String {
    if s.count == 1 {
        format!("\u{201c}{}\u{201d}", s.first_name)
    } else {
        format!("{} {}", s.count, tr("Items"))
    }
}

/// Open With: the default app first, then the others that take this type,
/// and Other… for the app chooser.
pub fn open_with(content_type: &str) -> gio::Menu {
    let apps = gio::Menu::new();
    let mut seen = HashSet::new();
    let default = gio::AppInfo::default_for_type(content_type, false);
    if let Some(app) = &default {
        if let Some(id) = app.id() {
            seen.insert(id.to_string());
            let label = format!("{} ({})", app.display_name(), tr("default"));
            let i = gio::MenuItem::new(Some(&label), None);
            i.set_action_and_target_value(Some("win.open-with"), Some(&id.to_variant()));
            apps.append_item(&i);
        }
    }
    let mut others: Vec<gio::AppInfo> = gio::AppInfo::recommended_for_type(content_type)
        .into_iter()
        .chain(gio::AppInfo::all_for_type(content_type))
        .filter(|a| a.should_show())
        .collect();
    others.sort_by_key(|a| a.display_name().to_lowercase());
    for app in others {
        let Some(id) = app.id() else {
            continue;
        };
        if !seen.insert(id.to_string()) {
            continue;
        }
        let i = gio::MenuItem::new(Some(&app.display_name()), None);
        i.set_action_and_target_value(Some("win.open-with"), Some(&id.to_variant()));
        apps.append_item(&i);
    }
    let menu = gio::Menu::new();
    menu.append_section(None, &apps);
    menu.append_section(
        None,
        &section(&[item(&tr("Other\u{2026}"), "win.open-with-other", None)]),
    );
    menu
}

/// The menu for selected items.
pub fn items(s: &Selection) -> gio::Menu {
    let menu = gio::Menu::new();
    let what = quoted_or_count(s);
    if s.in_trash {
        menu.append_section(None, &section(&[item(&tr("Open"), "win.open", None)]));
        menu.append_section(
            None,
            &section(&[
                item(&tr("Put Back"), "win.restore", None),
                item(
                    &tr("Delete Immediately\u{2026}"),
                    "win.delete",
                    Some("Delete"),
                ),
            ]),
        );
        menu.append_section(
            None,
            &section(&[
                item(&tr("Get Info"), "win.get-info", None),
                item(&tr("Quick Look"), "win.quick-look", None),
            ]),
        );
        menu.append_section(
            None,
            &section(&[item(
                &format!("{} {what}", tr("Copy")),
                "win.copy",
                Some("<Control>c"),
            )]),
        );
        return menu;
    }
    let first = gio::Menu::new();
    first.append_item(&item(&tr("Open"), "win.open", Some("<Control>o")));
    if s.bundle {
        first.append_item(&item(
            &tr("Show Package Contents"),
            "win.show-contents",
            None,
        ));
    } else {
        first.append_submenu(Some(&tr("Open With")), &open_with(s.content_type));
    }
    menu.append_section(None, &first);
    menu.append_section(
        None,
        &section(&[item(&tr("Move to Trash"), "win.trash", Some("Delete"))]),
    );
    let compress = if s.count == 1 {
        format!("{} {what}", tr("Compress"))
    } else {
        format!("{} {} {}", tr("Compress"), s.count, tr("Items"))
    };
    menu.append_section(
        None,
        &section(&[
            item(&tr("Get Info"), "win.get-info", Some("<Control>i")),
            item(&tr("Rename"), "win.rename", Some("F2")),
            item(&compress, "win.compress", None),
            item(&tr("Duplicate"), "win.duplicate", Some("<Control>d")),
            item(&tr("Make Alias"), "win.make-alias", None),
            item(&tr("Quick Look"), "win.quick-look", Some("space")),
        ]),
    );
    menu.append_section(
        None,
        &section(&[
            item(
                &format!("{} {what}", tr("Copy")),
                "win.copy",
                Some("<Control>c"),
            ),
            item(&tr("Share\u{2026}"), "win.share", None),
        ]),
    );
    menu
}

/// The menu for an empty part of the folder.
pub fn background(in_trash: bool) -> gio::Menu {
    let menu = gio::Menu::new();
    if in_trash {
        menu.append_section(
            None,
            &section(&[item(&tr("Empty Trash\u{2026}"), "win.empty-trash", None)]),
        );
    } else {
        menu.append_section(
            None,
            &section(&[item(
                &tr("New Folder"),
                "win.new-folder",
                Some("<Control><Shift>n"),
            )]),
        );
        menu.append_section(
            None,
            &section(&[
                item(&tr("Paste Item"), "win.paste", Some("<Control>v")),
                item(
                    &tr("Move Item Here"),
                    "win.paste-move",
                    Some("<Control><Shift>v"),
                ),
            ]),
        );
    }
    let view = gio::Menu::new();
    view.append_section(
        None,
        &section(&[
            item(&tr("as Icons"), "win.view::icons", Some("<Control>1")),
            item(&tr("as List"), "win.view::list", Some("<Control>2")),
        ]),
    );
    view.append_section(
        None,
        &section(&[
            item(
                &tr("Make Icons Larger"),
                "win.zoom-in",
                Some("<Control>plus"),
            ),
            item(
                &tr("Make Icons Smaller"),
                "win.zoom-out",
                Some("<Control>minus"),
            ),
        ]),
    );
    let last = gio::Menu::new();
    last.append_item(&item(
        &tr("Show Hidden Files"),
        "win.show-hidden",
        Some("<Control>h"),
    ));
    last.append_submenu(Some(&tr("View")), &view);
    menu.append_section(None, &last);
    menu
}
