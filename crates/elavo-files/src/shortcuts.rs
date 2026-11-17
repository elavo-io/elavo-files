//! Help > Keyboard Shortcuts: Files' own keys, in groups as in its menus.
//! The list has to follow `app::ACCELS` and the views' keys in
//! `window/fileops.rs`.

#![allow(deprecated)] // gtk::ShortcutsWindow, the window GTK still ships.

use gtk::glib;
use gtk::prelude::*;

use crate::i18n::tr;

/// `(group, [(title, accelerators)])`. Accelerators are GTK's syntax,
/// alternatives separated by spaces.
fn groups() -> Vec<(String, Vec<(String, &'static str)>)> {
    vec![
        (
            tr("Windows"),
            vec![
                (tr("New Folio Window"), "<Control>n"),
                (tr("Close Window"), "<Control>w"),
                (tr("Files Settings"), "<Control>comma"),
                (tr("Enter Full Screen"), "F11"),
                (tr("Keyboard Shortcuts"), "<Control>question"),
            ],
        ),
        (
            tr("Files and Folders"),
            vec![
                (tr("New Folder"), "<Control><Shift>n"),
                (tr("Open"), "<Control>o <Control>Down"),
                (tr("Get Info"), "<Control>i"),
                (tr("Rename"), "F2"),
                (tr("Duplicate"), "<Control>d"),
                (tr("Quick Look"), "space"),
                (tr("Add to Sidebar"), "<Control>t"),
                (tr("Move to Trash"), "Delete"),
                (tr("Delete Immediately"), "<Shift>Delete"),
                (tr("Eject"), "<Control>e"),
                (tr("Open in Terminal"), "<Control><Alt>t"),
                (tr("Context Menu"), "Menu <Shift>F10"),
            ],
        ),
        (
            tr("Edit"),
            vec![
                (tr("Undo"), "<Control>z"),
                (tr("Redo"), "<Control><Shift>z"),
                (tr("Cut"), "<Control>x"),
                (tr("Copy"), "<Control>c"),
                (tr("Paste"), "<Control>v"),
                (tr("Move Item Here"), "<Control><Shift>v"),
                (tr("Select All"), "<Control>a"),
                (tr("Find"), "<Control>f"),
            ],
        ),
        (
            tr("View"),
            vec![
                (tr("as Icons"), "<Control>1"),
                (tr("as List"), "<Control>2"),
                (tr("as Columns"), "<Control>3"),
                (tr("as Gallery"), "<Control>4"),
                (tr("Show Sidebar"), "<Control><Alt>s"),
                (tr("Show Preview"), "<Control><Shift>p"),
                (tr("Show Hidden Files"), "<Control>h"),
                (tr("Make Icons Larger"), "<Control>plus"),
                (tr("Make Icons Smaller"), "<Control>minus"),
                (tr("Default Icon Size"), "<Control>0"),
                (tr("Reload"), "F5 <Control>r"),
            ],
        ),
        (
            tr("Go"),
            vec![
                (tr("Back"), "<Alt>Left <Control>bracketleft"),
                (tr("Forward"), "<Alt>Right <Control>bracketright"),
                (tr("Enclosing Folder"), "<Alt>Up <Control>Up"),
                (tr("Home"), "<Control><Shift>h"),
                (tr("Go to Folder"), "<Control><Shift>g"),
                (tr("Connect to Server"), "<Control>k"),
            ],
        ),
    ]
}

fn escape(text: &str) -> String {
    glib::markup_escape_text(text).to_string()
}

fn ui() -> String {
    let mut xml = String::from(
        "<interface><object class=\"GtkShortcutsWindow\" id=\"window\">\
         <property name=\"modal\">true</property>\
         <child><object class=\"GtkShortcutsSection\">\
         <property name=\"section-name\">files</property>\
         <property name=\"max-height\">12</property>",
    );
    for (group, items) in groups() {
        xml.push_str(&format!(
            "<child><object class=\"GtkShortcutsGroup\"><property name=\"title\">{}</property>",
            escape(&group)
        ));
        for (title, accel) in items {
            xml.push_str(&format!(
                "<child><object class=\"GtkShortcutsShortcut\">\
                 <property name=\"title\">{}</property>\
                 <property name=\"accelerator\">{}</property></object></child>",
                escape(&title),
                escape(accel)
            ));
        }
        xml.push_str("</object></child>");
    }
    xml.push_str("</object></child></object></interface>");
    xml
}

pub fn present(app: &gtk::Application, parent: Option<&gtk::ApplicationWindow>) {
    let builder = gtk::Builder::from_string(&ui());
    let Some(window) = builder.object::<gtk::ShortcutsWindow>("window") else {
        return;
    };
    window.set_transient_for(parent);
    // Keeps Files running while it is open, also with no folder window.
    window.set_application(Some(app));
    window.present();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_application_shortcut_is_listed() {
        let listed: Vec<&str> = groups()
            .into_iter()
            .flat_map(|(_, items)| items.into_iter().flat_map(|(_, a)| a.split(' ')))
            .collect();
        for (action, accels) in crate::app::ACCELS {
            // Listed under another of their keys.
            if matches!(*action, "win.show-preview" | "win.home") || action.starts_with("win.view")
            {
                continue;
            }
            assert!(listed.contains(&accels[0]), "{action} {}", accels[0]);
        }
    }
}
