//! What a window does to files: the context menus, Open and Open With,
//! Move to Trash, Delete Immediately, Put Back and Empty Trash, Rename,
//! Compress, Duplicate, Make Alias, the clipboard, drag and drop, and Undo
//! for the last change. Long operations run through [`Jobs`] on a worker
//! thread; quick ones (rename, alias, new folder) run here.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use elavo_files_core::clipboard::Op;
use elavo_files_core::ops::{display_name, Dest, Job, Kind, Outcome};
use elavo_files_core::{eapp, entry, naming, trash};
use gtk::prelude::*;
use gtk::{gdk, gio, glib, graphene};

use super::{log, Window};
use crate::i18n::tr;
use crate::state::Mode;
use crate::views::{self, CellHandler, View};
use crate::{bundles, clip, dialogs, menus};

/// The last change, for Undo.
pub enum Undo {
    /// `(where it is now, where it was)`.
    Moved(Vec<(gio::File, gio::File)>),
    Renamed(gio::File, String),
    /// Original locations of what went to the Trash.
    Trashed(Vec<gio::File>),
    /// Copies, aliases, archives and new folders; undone into the Trash.
    Created(Vec<gio::File>),
}

/// Items to select once the folder lists them, after an operation.
#[derive(Default)]
pub struct Pending {
    uris: Vec<String>,
    rename: bool,
    generation: u32,
}

type WindowFn = Box<dyn Fn(&Rc<Window>)>;

/// Shortcuts that act on the selection. They live on the views, not the
/// application, so typing in the search field or a rename keeps them.
const VIEW_KEYS: &[(&str, &str)] = &[
    ("<Control>c", "win.copy"),
    ("<Control>x", "win.cut"),
    ("<Control>v", "win.paste"),
    ("<Control><Shift>v|<Control><Alt>v", "win.paste-move"),
    ("Delete|KP_Delete", "win.delete-key"),
    ("<Shift>Delete|<Shift>KP_Delete", "win.delete"),
    ("F2", "win.rename"),
    ("<Control>d", "win.duplicate"),
    ("<Control>a", "win.select-all"),
    ("<Control>z", "win.undo"),
    ("<Control>o|<Control>Down", "win.open"),
    ("Menu|<Shift>F10", "win.context-menu"),
];

fn quoted(name: &str) -> String {
    format!("\u{201c}{name}\u{201d}")
}

/// `“Report.pdf”` or `3 items`, for titles and questions.
fn what(files: &[gio::File]) -> String {
    match files {
        [one] => quoted(&display_name(one)),
        _ => format!("{} {}", files.len(), tr("items")),
    }
}

/// An installed application bundle: Files never moves, duplicates or
/// deletes one itself.
fn is_install(file: &gio::File) -> bool {
    file.path().is_some_and(|p| eapp::is_install(&p))
}

fn same_filesystem(a: &gio::File, b: &gio::File) -> bool {
    let fs = |f: &gio::File| {
        f.query_info(
            "id::filesystem",
            gio::FileQueryInfoFlags::NONE,
            gio::Cancellable::NONE,
        )
        .ok()
        .and_then(|i| i.attribute_string("id::filesystem"))
        .map(|s| s.to_string())
    };
    match (fs(a), fs(b)) {
        (Some(x), Some(y)) => !x.is_empty() && x == y,
        _ => false,
    }
}

/// Places that can't take new files.
fn read_only_place(uri: &str) -> bool {
    uri.starts_with("recent:") || uri.starts_with("network:") || trash::is_trash_uri(uri)
}

fn first_label(widget: &gtk::Widget) -> Option<gtk::Label> {
    if let Some(label) = widget.downcast_ref::<gtk::Label>() {
        return Some(label.clone());
    }
    let mut child = widget.first_child();
    while let Some(c) = child {
        if let Some(l) = first_label(&c) {
            return Some(l);
        }
        child = c.next_sibling();
    }
    None
}

fn is_in_cell(widget: Option<gtk::Widget>, stop: &gtk::Widget) -> bool {
    let mut w = widget;
    while let Some(x) = w {
        if &x == stop {
            return false;
        }
        if x.has_css_class("files-cell") {
            return true;
        }
        w = x.parent();
    }
    false
}

impl Window {
    fn view_kind(&self) -> View {
        match self.state.get().mode {
            Mode::Icons => View::Icons,
            Mode::List => View::List,
        }
    }

    fn sel_of(&self, view: View) -> &gtk::MultiSelection {
        match view {
            View::Icons => &self.grid_sel,
            View::List => &self.list_sel,
        }
    }

    fn view_widget(&self, view: View) -> gtk::Widget {
        match view {
            View::Icons => self.grid.clone().upcast(),
            View::List => self.list.clone().upcast(),
        }
    }

    pub(super) fn in_trash(&self) -> bool {
        trash::is_trash_uri(self.history.borrow().current())
    }

    fn writable_here(&self) -> bool {
        !read_only_place(self.history.borrow().current())
    }

    /// The selected rows of the view in use, with their positions.
    fn selected(&self) -> Vec<(u32, gio::FileInfo)> {
        let sel = self.sel_of(self.view_kind());
        let set = sel.selection();
        (0..set.size())
            .filter_map(|i| {
                let pos = set.nth(i as u32);
                sel.item(pos)
                    .and_then(|o| views::info_of(&o))
                    .map(|info| (pos, info))
            })
            .collect()
    }

    /// The file an operation acts on: the item itself in the Trash, what it
    /// points to elsewhere (Recents).
    fn file_of(&self, info: &gio::FileInfo) -> Option<gio::File> {
        if self.in_trash() {
            entry::file(info)
        } else {
            entry::target(info)
        }
    }

    fn selected_files(&self) -> Vec<gio::File> {
        self.selected()
            .iter()
            .filter_map(|(_, info)| self.file_of(info))
            .collect()
    }

    pub(super) fn set_undo(&self, undo: Undo) {
        self.undo.replace(Some(undo));
        self.update_actions();
    }

    /// Enable what the selection, the place and the clipboard allow.
    pub(super) fn update_actions(&self) {
        let selected = self.selected();
        let n = selected.len();
        let trash_view = self.in_trash();
        let writable = self.writable_here();
        let clip_files = clip::has_files(&self.window);
        let any = n > 0;
        let installs = selected
            .iter()
            .filter_map(|(_, i)| entry::file(i))
            .any(|f| is_install(&f));
        let one_bundle = matches!(selected.as_slice(), [(_, i)] if eapp::is_marked(i));
        for (name, on) in [
            ("open", any),
            ("open-with", any && !trash_view),
            ("open-with-other", n >= 1 && !trash_view),
            ("copy", any),
            ("cut", any && !trash_view && !installs),
            ("show-contents", one_bundle && !trash_view),
            ("trash", any && !trash_view),
            ("delete", any),
            ("delete-key", any),
            ("restore", any && trash_view),
            ("rename", n == 1 && !trash_view && !installs),
            ("compress", any && !trash_view),
            ("duplicate", any && !trash_view && !installs),
            ("make-alias", any && !trash_view && !installs),
            ("paste", writable && clip_files),
            ("paste-move", writable && clip_files),
            ("new-folder", writable),
            ("empty-trash", trash_view),
            ("undo", self.undo.borrow().is_some() && !self.jobs.running()),
            ("get-info", false),
            ("quick-look", false),
            ("share", false),
        ] {
            self.set_enabled(name, on);
        }
    }

    pub(super) fn connect_files(self: &Rc<Self>) {
        for sel in [&self.grid_sel, &self.list_sel] {
            let weak = Rc::downgrade(self);
            sel.connect_selection_changed(move |_, _, _| {
                if let Some(w) = weak.upgrade() {
                    w.update_actions();
                }
            });
            let weak = Rc::downgrade(self);
            sel.connect_items_changed(move |_, _, _, added| {
                if added == 0 {
                    return;
                }
                let weak = weak.clone();
                glib::idle_add_local_once(move || {
                    if let Some(w) = weak.upgrade() {
                        w.apply_pending();
                    }
                });
            });
        }
        let weak = Rc::downgrade(self);
        self.window.clipboard().connect_changed(move |_| {
            if let Some(w) = weak.upgrade() {
                w.update_actions();
            }
        });

        // Shortcuts on the views.
        let keys = gtk::ShortcutController::new();
        for (trigger, action) in VIEW_KEYS {
            keys.add_shortcut(gtk::Shortcut::new(
                gtk::ShortcutTrigger::parse_string(trigger),
                Some(gtk::NamedAction::new(action)),
            ));
        }
        self.stack.add_controller(keys);

        // Right-click or a long press on empty space; a click there clears
        // the selection.
        for view in [View::Icons, View::List] {
            let widget = self.view_widget(view);
            let click = gtk::GestureClick::builder().button(0).build();
            let weak = Rc::downgrade(self);
            click.connect_pressed(move |g, _, x, y| {
                let Some(w) = weak.upgrade() else {
                    return;
                };
                let widget = w.view_widget(view);
                widget.grab_focus();
                if is_in_cell(widget.pick(x, y, gtk::PickFlags::DEFAULT), &widget) {
                    return;
                }
                match g.current_button() {
                    gdk::BUTTON_SECONDARY => {
                        w.sel_of(view).unselect_all();
                        w.background_menu(&widget, x, y);
                    }
                    gdk::BUTTON_PRIMARY => {
                        let state = g.current_event_state();
                        if !state.intersects(
                            gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::SHIFT_MASK,
                        ) {
                            w.sel_of(view).unselect_all();
                        }
                    }
                    _ => {}
                }
            });
            widget.add_controller(click);
            let press = gtk::GestureLongPress::builder().touch_only(true).build();
            let weak = Rc::downgrade(self);
            press.connect_pressed(move |_, x, y| {
                let Some(w) = weak.upgrade() else {
                    return;
                };
                let widget = w.view_widget(view);
                if !is_in_cell(widget.pick(x, y, gtk::PickFlags::DEFAULT), &widget) {
                    w.background_menu(&widget, x, y);
                }
            });
            widget.add_controller(press);
        }

        // Drops on empty space go into the folder shown.
        for scroll in [&self.grid_scroll, &self.list_scroll] {
            let drop = gtk::DropTarget::new(
                gdk::FileList::static_type(),
                gdk::DragAction::COPY | gdk::DragAction::MOVE,
            );
            drop.set_preload(true);
            let weak = Rc::downgrade(self);
            drop.connect_accept(move |_, d| {
                weak.upgrade()
                    .is_some_and(|w| w.writable_here() || w.in_trash())
                    && d.formats().contains_type(gdk::FileList::static_type())
            });
            let weak = Rc::downgrade(self);
            drop.connect_motion(move |t, _, _| {
                let (Some(w), Some(dest)) =
                    (weak.upgrade(), weak.upgrade().and_then(|w| w.drop_dest()))
                else {
                    return gdk::DragAction::empty();
                };
                let files = t.value().map(|v| views::files_of(&v)).unwrap_or_default();
                let offered = views::offered(t);
                w.drop_action(&dest, &files, views::force_copy(t), offered)
            });
            let weak = Rc::downgrade(self);
            drop.connect_drop(move |t, value, _, _| {
                let Some(w) = weak.upgrade() else {
                    return false;
                };
                let Some(dest) = w.drop_dest() else {
                    return false;
                };
                let files = views::files_of(value);
                let offered = views::offered(t);
                let action = w.drop_action(&dest, &files, views::force_copy(t), offered);
                if action.is_empty() || files.is_empty() {
                    return false;
                }
                w.drop_files(&dest, files, action);
                true
            });
            scroll.add_controller(drop);
        }
    }

    /// Where a drop on empty space goes: the folder, or the Trash.
    fn drop_dest(&self) -> Option<gio::File> {
        if self.in_trash() {
            return Some(gio::File::for_uri(trash::URI));
        }
        self.dir.file()
    }

    pub(super) fn add_file_actions(self: &Rc<Self>) {
        let add = |name: &str, f: WindowFn| {
            let action = gio::SimpleAction::new(name, None);
            let weak = Rc::downgrade(self);
            let action_name = name.to_owned();
            action.connect_activate(move |_, _| {
                log(&format!("action {action_name}"));
                if let Some(w) = weak.upgrade() {
                    f(&w);
                }
            });
            self.window.add_action(&action);
        };
        add("open", Box::new(|w| w.open_selected()));
        add(
            "show-contents",
            Box::new(|w| {
                if let [(_, info)] = w.selected().as_slice() {
                    if let Some(f) = entry::file(info) {
                        w.go(f);
                    }
                }
            }),
        );
        add("open-with-other", Box::new(|w| w.open_with_other()));
        add("trash", Box::new(|w| w.trash_files(w.selected_files())));
        add(
            "delete-key",
            Box::new(|w| {
                if w.in_trash() {
                    w.delete_selected();
                } else {
                    w.trash_files(w.selected_files());
                }
            }),
        );
        add("delete", Box::new(|w| w.delete_selected()));
        add("restore", Box::new(|w| w.restore_selected()));
        add("empty-trash", Box::new(|w| w.empty_trash()));
        add("rename", Box::new(|w| w.start_rename()));
        add("compress", Box::new(|w| w.compress()));
        add("duplicate", Box::new(|w| w.duplicate()));
        add("make-alias", Box::new(|w| w.make_alias()));
        add(
            "copy",
            Box::new(|w| clip::put(&w.window, &w.selected_files(), Op::Copy)),
        );
        add(
            "cut",
            Box::new(|w| clip::put(&w.window, &w.selected_files(), Op::Cut)),
        );
        add("paste", Box::new(|w| w.paste(false)));
        add("paste-move", Box::new(|w| w.paste(true)));
        add(
            "select-all",
            Box::new(|w| {
                w.sel_of(w.view_kind()).select_all();
            }),
        );
        add("undo", Box::new(|w| w.undo()));
        add("context-menu", Box::new(|w| w.keyboard_menu()));
        // Later phases: Get Info (4), Quick Look (3), Share (LocalSend).
        add("get-info", Box::new(|_| {}));
        add("quick-look", Box::new(|_| {}));
        add("share", Box::new(|_| {}));

        let open_with = gio::SimpleAction::new("open-with", Some(glib::VariantTy::STRING));
        let weak = Rc::downgrade(self);
        open_with.connect_activate(move |_, id| {
            if let (Some(w), Some(id)) =
                (weak.upgrade(), id.and_then(|v| v.str().map(str::to_owned)))
            {
                w.open_with(&id);
            }
        });
        self.window.add_action(&open_with);
        self.update_actions();
    }

    // Menus

    fn show_menu(&self, model: &gio::Menu, from: &gtk::Widget, x: f64, y: f64) {
        if let Some(old) = self.menu.take() {
            old.popdown();
        }
        let Some(point) =
            from.compute_point(&self.content, &graphene::Point::new(x as f32, y as f32))
        else {
            return;
        };
        let popover = gtk::PopoverMenu::from_model_full(model, gtk::PopoverMenuFlags::NESTED);
        popover.add_css_class("files-menu");
        popover.set_has_arrow(false);
        popover.set_halign(gtk::Align::Start);
        popover.set_position(gtk::PositionType::Bottom);
        popover.set_parent(&self.content);
        popover.set_pointing_to(Some(&gdk::Rectangle::new(
            point.x() as i32,
            point.y() as i32,
            1,
            1,
        )));
        let weak = self.me.borrow().clone();
        popover.connect_closed(move |p| {
            let p = p.clone();
            let weak = weak.clone();
            glib::idle_add_local_once(move || {
                p.unparent();
                if let Some(w) = weak.upgrade() {
                    if !gtk::prelude::GtkWindowExt::focus(&w.window)
                        .is_some_and(|f| f.is_ancestor(&w.stack))
                    {
                        w.focus_view();
                    }
                }
            });
        });
        popover.popup();
        self.menu.replace(Some(popover));
    }

    fn items_menu(&self) -> gio::Menu {
        let selected = self.selected();
        let first = selected.first().map(|(_, i)| i);
        let name = first.map(entry::name).unwrap_or_default();
        let ct = first.map(entry::content_type).unwrap_or_default();
        menus::items(&menus::Selection {
            count: selected.len(),
            first_name: &name,
            content_type: &ct,
            in_trash: self.in_trash(),
            bundle: selected.len() == 1 && first.is_some_and(eapp::is_marked),
        })
    }

    fn background_menu(&self, from: &gtk::Widget, x: f64, y: f64) {
        self.update_actions();
        self.show_menu(&menus::background(self.in_trash()), from, x, y);
    }

    /// Menu key or Shift+F10: the menu for the selection, by its item.
    fn keyboard_menu(&self) {
        let view = self.view_kind();
        let widget = self.view_widget(view);
        let selected = self.selected();
        let cell = selected
            .first()
            .and_then(|(_, info)| entry::file(info))
            .and_then(|f| self.cells.widget(&f.uri()));
        match cell {
            Some(cell) => {
                let (w, h) = (cell.width() as f64, cell.height() as f64);
                self.update_actions();
                self.show_menu(&self.items_menu(), &cell, w / 2.0, h / 2.0);
            }
            None => self.background_menu(&widget, 40.0, 40.0),
        }
    }

    // Opening

    fn open_selected(&self) {
        let selected = self.selected();
        if let Some((_, folder)) = selected
            .iter()
            .find(|(_, i)| entry::is_dir(i) && !eapp::is_marked(i))
        {
            self.open(folder);
            return;
        }
        for (_, info) in &selected {
            self.open(info);
        }
    }

    fn launch(&self, app: &gio::AppInfo, files: &[gio::File]) {
        let context = gtk::prelude::WidgetExt::display(&self.window).app_launch_context();
        if let Err(err) = app.launch(files, Some(&context)) {
            dialogs::error(
                &self.window,
                &format!("{} {}", tr("Couldn't open with"), app.display_name()),
                err.message(),
            );
        }
    }

    fn open_with(&self, id: &str) {
        let files = self.selected_files();
        match gio::AppInfo::all()
            .into_iter()
            .find(|a| a.id().as_deref() == Some(id))
        {
            Some(app) => self.launch(&app, &files),
            None => log(&format!("no app {id}")),
        }
    }

    #[allow(deprecated)]
    fn open_with_other(self: &Rc<Self>) {
        let selected = self.selected();
        let Some((_, info)) = selected.first() else {
            return;
        };
        let dialog = gtk::AppChooserDialog::for_content_type(
            Some(&self.window),
            gtk::DialogFlags::MODAL | gtk::DialogFlags::DESTROY_WITH_PARENT,
            &entry::content_type(info),
        );
        let weak = Rc::downgrade(self);
        dialog.connect_response(move |d, response| {
            if response == gtk::ResponseType::Ok {
                if let (Some(w), Some(app)) = (weak.upgrade(), d.app_info()) {
                    w.launch(&app, &w.selected_files());
                }
            }
            d.destroy();
        });
        dialog.present();
    }

    // Operations

    fn run(
        self: &Rc<Self>,
        job: Job,
        title: String,
        then: impl FnOnce(&Rc<Self>, Outcome) + 'static,
    ) {
        log(&format!("job {:?}: {title}", job.kind));
        let weak = Rc::downgrade(self);
        self.jobs.start(job, title, move |out| {
            if let Some(w) = weak.upgrade() {
                log(&format!(
                    "job done: {} done, {} errors{}",
                    out.done.len(),
                    out.errors.len(),
                    if out.cancelled { ", cancelled" } else { "" }
                ));
                then(&w, out);
                w.update_actions();
            }
        });
        self.update_actions();
    }

    fn results(out: &Outcome) -> Vec<gio::File> {
        out.done.iter().map(|(_, d)| d.clone()).collect()
    }

    /// Copy or move files into a folder (or the Trash).
    pub(super) fn transfer(self: &Rc<Self>, files: Vec<gio::File>, dest: gio::File, moving: bool) {
        if files.is_empty() {
            return;
        }
        if trash::is_trash_uri(&dest.uri()) {
            self.trash_files(files);
            return;
        }
        // An install is never moved or copied: elsewhere it becomes a
        // shortcut, and into an Applications folder nothing happens.
        let (installs, files): (Vec<_>, Vec<_>) = files.into_iter().partition(is_install);
        if !installs.is_empty() && !dest.path().is_some_and(|p| eapp::in_applications(&p)) {
            let mut shortcuts = Vec::new();
            let mut errors = Vec::new();
            for bundle in &installs {
                match bundle.path().map(|p| bundles::shortcut(&p)) {
                    Some(Ok(f)) => shortcuts.push(f),
                    Some(Err(e)) => errors.push(format!("{}: {e}", quoted(&display_name(bundle)))),
                    None => {}
                }
            }
            dialogs::errors(
                &self.window,
                &tr("Some shortcuts couldn't be made."),
                &errors,
            );
            if !shortcuts.is_empty() {
                self.transfer(shortcuts, dest.clone(), false);
            }
        }
        if files.is_empty() {
            return;
        }
        let title = format!(
            "{} {} {} {}",
            if moving { tr("Moving") } else { tr("Copying") },
            what(&files),
            tr("to"),
            quoted(&display_name(&dest))
        );
        let kind = if moving { Kind::Move } else { Kind::Copy };
        self.run(
            Job::new(kind, files, Dest::Into(dest)),
            title,
            move |w, out| {
                dialogs::errors(
                    &w.window,
                    &if moving {
                        tr("Some items couldn't be moved.")
                    } else {
                        tr("Some items couldn't be copied.")
                    },
                    &out.errors,
                );
                if out.done.is_empty() {
                    return;
                }
                let results = Self::results(&out);
                w.set_undo(if moving {
                    Undo::Moved(
                        out.done
                            .iter()
                            .map(|(s, d)| (d.clone(), s.clone()))
                            .collect(),
                    )
                } else {
                    Undo::Created(results.clone())
                });
                w.select_when_listed(&results, false);
            },
        );
    }

    /// Installs are the service's: one that is running stays, and a shared
    /// one is handed to `elavo-applications-ctl remove`. The rest go on.
    fn guard_installs(&self, files: Vec<gio::File>) -> Vec<gio::File> {
        let mut rest = Vec::new();
        let mut errors = Vec::new();
        for file in files {
            let Some(path) = file.path().filter(|p| eapp::is_install(p)) else {
                rest.push(file);
                continue;
            };
            let name = quoted(&display_name(&file));
            if bundles::running(&path) {
                errors.push(format!("{name} {}", tr("is open. Quit it first.")));
            } else if eapp::in_shared(&path) {
                if let Err(e) = bundles::remove(&path) {
                    errors.push(format!("{name}: {e}"));
                }
            } else {
                rest.push(file);
            }
        }
        dialogs::errors(
            &self.window,
            &tr("Some applications weren't removed."),
            &errors,
        );
        rest
    }

    fn trash_files(self: &Rc<Self>, files: Vec<gio::File>) {
        if files.is_empty() || self.in_trash() {
            return;
        }
        let files = self.guard_installs(files);
        if files.is_empty() {
            return;
        }
        let title = format!("{} {} {}", tr("Moving"), what(&files), tr("to the Trash"));
        self.run(
            Job::new(Kind::Trash, files, Dest::Beside),
            title,
            |w, out| {
                dialogs::errors(
                    &w.window,
                    &tr("Some items couldn't be moved to the Trash."),
                    &out.errors,
                );
                if !out.done.is_empty() {
                    w.set_undo(Undo::Trashed(
                        out.done.iter().map(|(s, _)| s.clone()).collect(),
                    ));
                }
                if !out.untrashable.is_empty() && !out.cancelled {
                    let message = match out.untrashable.as_slice() {
                        [one] => format!(
                            "{} {} {}",
                            quoted(&display_name(one)),
                            tr("can\u{2019}t be moved to the Trash."),
                            tr("Do you want to delete it immediately?")
                        ),
                        many => format!(
                            "{} {} {}",
                            many.len(),
                            tr("items can\u{2019}t be moved to the Trash."),
                            tr("Do you want to delete them immediately?")
                        ),
                    };
                    w.confirm_delete(out.untrashable, message);
                }
            },
        );
    }

    fn delete_selected(self: &Rc<Self>) {
        let files = self.selected_files();
        if files.is_empty() {
            return;
        }
        let message = match files.as_slice() {
            [one] => format!(
                "{} {}?",
                tr("Are you sure you want to delete"),
                quoted(&display_name(one))
            ),
            _ => format!(
                "{} {} {}?",
                tr("Are you sure you want to delete these"),
                files.len(),
                tr("items")
            ),
        };
        self.confirm_delete(files, message);
    }

    /// Ask, then delete for good.
    fn confirm_delete(self: &Rc<Self>, files: Vec<gio::File>, message: String) {
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let Some(w) = weak.upgrade() else {
                return;
            };
            let detail = tr("This can\u{2019}t be undone.");
            let yes = dialogs::confirm(&w.window, &message, &detail, &tr("Delete")).await;
            w.focus_view();
            if !yes {
                return;
            }
            let files = w.guard_installs(files);
            if files.is_empty() {
                return;
            }
            let title = format!("{} {}", tr("Deleting"), what(&files));
            w.run(
                Job::new(Kind::Delete, files, Dest::Beside),
                title,
                |w, out| {
                    dialogs::errors(
                        &w.window,
                        &tr("Some items couldn't be deleted."),
                        &out.errors,
                    );
                    // Deleted for good: nothing to undo into.
                    w.undo.replace(None);
                },
            );
        });
    }

    fn restore_selected(self: &Rc<Self>) {
        let home = trash::home();
        let mut items = Vec::new();
        let mut targets = Vec::new();
        let mut missing = Vec::new();
        for file in self.selected_files() {
            match trash::orig_path(&file, &home) {
                Some(orig) => {
                    items.push(file);
                    targets.push(gio::File::for_path(orig));
                }
                None => missing.push(display_name(&file)),
            }
        }
        if !missing.is_empty() {
            dialogs::error(
                &self.window,
                &tr("Some items can\u{2019}t be put back."),
                &format!(
                    "{} {}",
                    tr("Their original location isn\u{2019}t known:"),
                    missing.join(", ")
                ),
            );
        }
        self.put_back(items, targets);
    }

    fn put_back(self: &Rc<Self>, items: Vec<gio::File>, targets: Vec<gio::File>) {
        if items.is_empty() {
            return;
        }
        let title = format!("{} {}", tr("Putting back"), what(&targets));
        self.run(
            Job::new(Kind::Restore, items, Dest::Exactly(targets)),
            title,
            |w, out| {
                dialogs::errors(
                    &w.window,
                    &tr("Some items couldn't be put back."),
                    &out.errors,
                );
                if !out.done.is_empty() {
                    w.set_undo(Undo::Created(Self::results(&out)));
                }
            },
        );
    }

    fn empty_trash(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let Some(w) = weak.upgrade() else {
                return;
            };
            let ok = dialogs::confirm(
                &w.window,
                &tr("Are you sure you want to permanently erase the items in the Trash?"),
                &tr("You can\u{2019}t undo this action."),
                &tr("Empty Trash"),
            )
            .await;
            w.focus_view();
            if ok {
                w.run(
                    Job::new(Kind::EmptyTrash, Vec::new(), Dest::Beside),
                    tr("Emptying the Trash"),
                    |w, out| {
                        dialogs::errors(
                            &w.window,
                            &tr("The Trash couldn't be emptied."),
                            &out.errors,
                        );
                        w.undo.replace(None);
                    },
                );
            }
        });
    }

    fn compress(self: &Rc<Self>) {
        let files = self.selected_files();
        if files.is_empty() {
            return;
        }
        let title = format!("{} {}", tr("Compressing"), what(&files));
        self.run(
            Job::new(Kind::Compress, files, Dest::Beside),
            title,
            |w, out| {
                dialogs::errors(&w.window, &tr("Compress didn't finish."), &out.errors);
                if !out.done.is_empty() {
                    let results = Self::results(&out);
                    w.set_undo(Undo::Created(results.clone()));
                    w.select_when_listed(&results, false);
                }
            },
        );
    }

    fn duplicate(self: &Rc<Self>) {
        let files = self.selected_files();
        if files.is_empty() {
            return;
        }
        let title = format!("{} {}", tr("Duplicating"), what(&files));
        self.run(
            Job::new(Kind::Copy, files, Dest::Beside),
            title,
            |w, out| {
                dialogs::errors(
                    &w.window,
                    &tr("Some items couldn't be duplicated."),
                    &out.errors,
                );
                if !out.done.is_empty() {
                    let results = Self::results(&out);
                    w.set_undo(Undo::Created(results.clone()));
                    w.select_when_listed(&results, false);
                }
            },
        );
    }

    /// Make Alias: a symbolic link named `name alias` beside each item.
    fn make_alias(self: &Rc<Self>) {
        let mut made = Vec::new();
        let mut errors = Vec::new();
        for file in self.selected_files() {
            let (Some(path), Some(dir)) = (file.path(), file.parent()) else {
                continue;
            };
            let name = naming::alias(&display_name(&file), |n| {
                dir.child(n).query_exists(gio::Cancellable::NONE)
            });
            let link = dir.child(&name);
            match link.make_symbolic_link(&path, gio::Cancellable::NONE) {
                Ok(()) => made.push(link),
                Err(e) => errors.push(format!("{}: {}", quoted(&name), e.message())),
            }
        }
        dialogs::errors(&self.window, &tr("Some aliases couldn't be made."), &errors);
        if !made.is_empty() {
            self.set_undo(Undo::Created(made.clone()));
            self.select_when_listed(&made, false);
        }
    }

    fn paste(self: &Rc<Self>, force_move: bool) {
        if !self.writable_here() {
            return;
        }
        let Some(dest) = self.dir.file() else {
            return;
        };
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let Some(w) = weak.upgrade() else {
                return;
            };
            let Some((op, files)) = clip::read(&w.window).await else {
                log("paste: nothing usable on the clipboard");
                return;
            };
            let moving = force_move || op == Op::Cut;
            if op == Op::Cut {
                // A cut pastes once, as in Nautilus.
                clip::clear(&w.window);
            }
            w.transfer(files, dest, moving);
        });
    }

    fn undo(self: &Rc<Self>) {
        let Some(undo) = self.undo.take() else {
            return;
        };
        self.update_actions();
        match undo {
            Undo::Moved(pairs) => {
                let (now, was): (Vec<_>, Vec<_>) = pairs.into_iter().unzip();
                let title = format!("{} {}", tr("Moving back"), what(&was));
                self.run(
                    Job::new(Kind::Move, now, Dest::Exactly(was)),
                    title,
                    |w, out| {
                        dialogs::errors(&w.window, &tr("Undo didn't finish."), &out.errors);
                        w.select_when_listed(&Self::results(&out), false);
                    },
                );
            }
            Undo::Renamed(now, was) => {
                let weak = Rc::downgrade(self);
                now.set_display_name_async(
                    &was,
                    glib::Priority::DEFAULT,
                    gio::Cancellable::NONE,
                    move |r| {
                        let Some(w) = weak.upgrade() else {
                            return;
                        };
                        match r {
                            Ok(file) => w.select_when_listed(&[file], false),
                            Err(e) => {
                                dialogs::error(&w.window, &tr("Undo didn't finish."), e.message())
                            }
                        }
                    },
                );
            }
            Undo::Trashed(origs) => {
                let home = trash::home();
                let (mut items, mut targets) = (Vec::new(), Vec::new());
                for orig in origs {
                    if let Some(item) = orig.path().and_then(|p| trash::find(&p, &home)) {
                        items.push(item);
                        targets.push(orig);
                    }
                }
                if items.is_empty() {
                    dialogs::error(
                        &self.window,
                        &tr("Undo didn't finish."),
                        &tr("The items are no longer in the Trash."),
                    );
                    return;
                }
                let title = format!("{} {}", tr("Putting back"), what(&targets));
                self.run(
                    Job::new(Kind::Restore, items, Dest::Exactly(targets)),
                    title,
                    |w, out| {
                        dialogs::errors(&w.window, &tr("Undo didn't finish."), &out.errors);
                        w.select_when_listed(&Self::results(&out), false);
                    },
                );
            }
            Undo::Created(files) => {
                let title = format!("{} {} {}", tr("Moving"), what(&files), tr("to the Trash"));
                self.run(
                    Job::new(Kind::Trash, files, Dest::Beside),
                    title,
                    |w, out| {
                        dialogs::errors(&w.window, &tr("Undo didn't finish."), &out.errors);
                    },
                );
            }
        }
    }

    // Selecting new items, and Rename

    /// Select `files` once the folder lists them, and rename the one if
    /// `rename` (New Folder).
    pub(super) fn select_when_listed(self: &Rc<Self>, files: &[gio::File], rename: bool) {
        if files.is_empty() {
            return;
        }
        let generation = {
            let mut p = self.pending.borrow_mut();
            p.uris = files.iter().map(|f| f.uri().to_string()).collect();
            p.rename = rename;
            p.generation = p.generation.wrapping_add(1);
            p.generation
        };
        self.apply_pending();
        // Tools such as zip write a temporary file and rename it, which the
        // folder monitor can miss; list the folder again if a result that
        // exists has not shown up.
        let weak = Rc::downgrade(self);
        glib::timeout_add_local_once(Duration::from_millis(800), move || {
            let Some(w) = weak.upgrade() else {
                return;
            };
            let here = w.dir.file();
            let missing = {
                let p = w.pending.borrow();
                p.generation == generation
                    && p.uris.iter().any(|u| {
                        let f = gio::File::for_uri(u);
                        f.parent()
                            .zip(here.as_ref())
                            .is_some_and(|(a, b)| a.equal(b))
                            && f.query_exists(gio::Cancellable::NONE)
                    })
            };
            if missing && !w.dir.is_loading() {
                log("results not listed; reloading");
                w.reload();
            }
        });
        let weak = Rc::downgrade(self);
        glib::timeout_add_local_once(Duration::from_secs(3), move || {
            if let Some(w) = weak.upgrade() {
                let mut p = w.pending.borrow_mut();
                if p.generation == generation {
                    p.uris.clear();
                }
            }
        });
    }

    fn apply_pending(self: &Rc<Self>) {
        let (uris, rename) = {
            let p = self.pending.borrow();
            if p.uris.is_empty() {
                return;
            }
            (p.uris.clone(), p.rename)
        };
        let view = self.view_kind();
        let sel = self.sel_of(view);
        let mut found = Vec::new();
        for i in 0..sel.n_items() {
            let uri = sel
                .item(i)
                .and_then(|o| views::info_of(&o))
                .and_then(|info| entry::file(&info))
                .map(|f| f.uri().to_string());
            if uri.is_some_and(|u| uris.contains(&u)) {
                found.push(i);
            }
        }
        if found.is_empty() {
            return;
        }
        sel.unselect_all();
        for pos in &found {
            sel.select_item(*pos, false);
        }
        match view {
            View::Icons => self
                .grid
                .scroll_to(found[0], gtk::ListScrollFlags::FOCUS, None),
            View::List => self.list.scroll_to(
                found[0],
                None::<&gtk::ColumnViewColumn>,
                gtk::ListScrollFlags::FOCUS,
                None,
            ),
        }
        if found.len() >= uris.len() {
            self.pending.borrow_mut().uris.clear();
            if rename && found.len() == 1 {
                let weak = Rc::downgrade(self);
                glib::timeout_add_local_once(Duration::from_millis(60), move || {
                    if let Some(w) = weak.upgrade() {
                        w.start_rename();
                    }
                });
            }
        }
    }

    pub(super) fn start_rename(self: &Rc<Self>) {
        self.rename_try(3);
    }

    /// The cell may not be on screen yet after a scroll; try again shortly.
    fn rename_try(self: &Rc<Self>, tries: u32) {
        let selected = self.selected();
        let [(pos, info)] = selected.as_slice() else {
            return;
        };
        if self.in_trash() {
            return;
        }
        let Some(file) = entry::file(info) else {
            return;
        };
        if let Some(cell) = self.cells.widget(&file.uri()) {
            self.rename_at(&cell, info);
            return;
        }
        if tries == 0 {
            return;
        }
        let view = self.view_kind();
        match view {
            View::Icons => self.grid.scroll_to(*pos, gtk::ListScrollFlags::FOCUS, None),
            View::List => self.list.scroll_to(
                *pos,
                None::<&gtk::ColumnViewColumn>,
                gtk::ListScrollFlags::FOCUS,
                None,
            ),
        }
        let weak = Rc::downgrade(self);
        glib::timeout_add_local_once(Duration::from_millis(80), move || {
            if let Some(w) = weak.upgrade() {
                w.rename_try(tries - 1);
            }
        });
    }

    /// The name field, over the item's name.
    fn rename_at(self: &Rc<Self>, cell: &gtk::Widget, info: &gio::FileInfo) {
        let Some(file) = entry::file(info) else {
            return;
        };
        let target = first_label(cell)
            .map(|l| l.upcast::<gtk::Widget>())
            .unwrap_or_else(|| cell.clone());
        let Some(bounds) = target.compute_bounds(&self.content) else {
            return;
        };
        // A copied bundle is renamed without its .eapp, which stays on.
        let suffix = if eapp::is_marked(info) {
            eapp::SUFFIX
        } else {
            ""
        };
        let name = match file.path() {
            Some(p) if !suffix.is_empty() => eapp::plain_name(&p),
            _ => entry::name(info),
        };
        let is_dir = entry::is_dir(info);
        let field = gtk::Entry::builder()
            .text(&name)
            .width_chars(name.chars().count().clamp(14, 40) as i32)
            .css_classes(["files-rename-entry"])
            .build();
        let popover = gtk::Popover::builder()
            .child(&field)
            .has_arrow(false)
            .position(gtk::PositionType::Bottom)
            .css_classes(["files-rename"])
            .build();
        popover.set_parent(&self.content);
        let h = bounds.height().max(16.0);
        popover.set_pointing_to(Some(&gdk::Rectangle::new(
            bounds.x() as i32,
            bounds.y() as i32,
            bounds.width().max(1.0) as i32,
            h as i32,
        )));
        // Sit over the name rather than under it.
        popover.set_offset(0, -(h as i32) - 6);
        let weak = Rc::downgrade(self);
        popover.connect_closed(move |p| {
            let p = p.clone();
            let weak = weak.clone();
            glib::idle_add_local_once(move || {
                p.unparent();
                if let Some(w) = weak.upgrade() {
                    w.focus_view();
                }
            });
        });
        let done = Rc::new(Cell::new(false));
        let weak = Rc::downgrade(self);
        let (old, pop, d) = (name.clone(), popover.downgrade(), done.clone());
        field.connect_activate(move |field| {
            d.set(true);
            if let Some(w) = weak.upgrade() {
                w.finish_rename(
                    &file,
                    &format!("{old}{suffix}"),
                    &format!("{}{suffix}", field.text()),
                );
            }
            if let Some(p) = pop.upgrade() {
                p.popdown();
            }
        });
        popover.popup();
        if self.view_kind() == View::List {
            // A list row's name is left-aligned: line the field up with it
            // rather than centring it over the icon.
            let (p, label_width, h) = (popover.downgrade(), bounds.width(), h as i32);
            glib::idle_add_local_once(move || {
                if let Some(p) = p.upgrade() {
                    let (_, natural, _, _) = p.measure(gtk::Orientation::Horizontal, -1);
                    let shift = ((natural as f32 - label_width) / 2.0 - 6.0).max(0.0);
                    p.set_offset(shift as i32, -h - 6);
                }
            });
        }
        field.grab_focus();
        field.select_region(0, naming::stem_chars(&name, is_dir) as i32);
        log(&format!("renaming {name}"));
    }

    fn finish_rename(self: &Rc<Self>, file: &gio::File, old: &str, new: &str) {
        if new == old {
            return;
        }
        if let Err(why) = naming::check(new) {
            dialogs::error(
                &self.window,
                &tr("That name can\u{2019}t be used."),
                &tr(why.message()),
            );
            return;
        }
        let taken = file
            .parent()
            .is_some_and(|dir| dir.child(new).query_exists(gio::Cancellable::NONE));
        if taken && !new.eq_ignore_ascii_case(old) {
            dialogs::error(
                &self.window,
                &format!(
                    "{} {} {}",
                    tr("The name"),
                    quoted(new),
                    tr("is already taken.")
                ),
                &tr("Please choose a different name."),
            );
            return;
        }
        let weak = Rc::downgrade(self);
        let old = old.to_owned();
        file.set_display_name_async(
            new,
            glib::Priority::DEFAULT,
            gio::Cancellable::NONE,
            move |r| {
                let Some(w) = weak.upgrade() else {
                    return;
                };
                match r {
                    Ok(renamed) => {
                        log(&format!("renamed to {}", renamed.uri()));
                        w.set_undo(Undo::Renamed(renamed.clone(), old));
                        w.select_when_listed(&[renamed], false);
                    }
                    Err(e) => dialogs::error(
                        &w.window,
                        &tr("Couldn\u{2019}t rename the item."),
                        e.message(),
                    ),
                }
            },
        );
    }
}

impl CellHandler for Window {
    fn item_menu(&self, view: View, position: u32, cell: &gtk::Widget, x: f64, y: f64) {
        let sel = self.sel_of(view);
        if !sel.is_selected(position) {
            sel.select_item(position, true);
        }
        self.view_widget(view).grab_focus();
        self.update_actions();
        self.show_menu(&self.items_menu(), cell, x, y);
    }

    fn drag_files(&self, view: View, position: u32) -> Vec<gio::File> {
        let sel = self.sel_of(view);
        if !sel.is_selected(position) {
            sel.select_item(position, true);
        }
        let files = self.selected_files();
        log(&format!("dragging {}", what(&files)));
        files
    }

    fn drop_action(
        &self,
        dest: &gio::File,
        files: &[gio::File],
        force_copy: bool,
        offered: gdk::DragAction,
    ) -> gdk::DragAction {
        let pick = |want: gdk::DragAction| {
            if offered.contains(want) {
                want
            } else if offered.contains(gdk::DragAction::COPY) {
                gdk::DragAction::COPY
            } else {
                offered & gdk::DragAction::MOVE
            }
        };
        let uri = dest.uri();
        if trash::is_trash_uri(&uri) {
            return offered & gdk::DragAction::MOVE;
        }
        if read_only_place(&uri) {
            return gdk::DragAction::empty();
        }
        if files.iter().any(is_install) {
            // Never moved; dropped elsewhere it becomes a shortcut.
            return if dest.path().is_some_and(|p| eapp::in_applications(&p)) {
                gdk::DragAction::empty()
            } else {
                offered & gdk::DragAction::COPY
            };
        }
        if force_copy {
            return pick(gdk::DragAction::COPY);
        }
        match files.first() {
            Some(f) if !same_filesystem(f, dest) => pick(gdk::DragAction::COPY),
            _ => pick(gdk::DragAction::MOVE),
        }
    }

    fn drop_files(&self, dest: &gio::File, files: Vec<gio::File>, action: gdk::DragAction) {
        log(&format!(
            "drop {} on {} as {action:?}",
            what(&files),
            dest.uri()
        ));
        // The handler is called through a Weak<dyn CellHandler>; find the
        // Rc to start a job from.
        let Some(w) = self.rc() else {
            return;
        };
        let dest = dest.clone();
        // After the drag ends, so the source's drag-end runs first.
        glib::idle_add_local_once(move || {
            w.transfer(files, dest, action == gdk::DragAction::MOVE);
        });
    }
}
