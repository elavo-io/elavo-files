//! File operations and Quick Look for other programs, on the session bus
//! under Files' own name, `io.elavo.Files` (D-Bus starts Files with
//! `--gapplication-service` when it isn't running). Folio's desktop and
//! panel use them in place of Nautilus's FileOperations2 and Sushi.
//!
//! `/io/elavo/Files/FileOperations`, `io.elavo.Files.FileOperations`:
//!
//! - `CopyURIs(as sources, s destination, a{sv} platform_data)`
//! - `MoveURIs(as sources, s destination, a{sv} platform_data)`
//! - `TrashURIs(as uris, a{sv} platform_data)`
//! - `CreateFolder(s uri, a{sv} platform_data)`, the new folder's own URI
//! - `RenameURI(s uri, s name, a{sv} platform_data) -> s new_uri`
//! - `Undo(a{sv} platform_data)`, `Redo(a{sv} platform_data)`
//! - properties `CanUndo` and `CanRedo` (`b`), and `UndoLabel` and
//!   `RedoLabel` (`s`, “Undo Move to Trash”), with `PropertiesChanged`
//!
//! Copy, move and trash return once the operation starts. It then runs
//! through the same engine as in a Files window: name conflicts are asked
//! about, and progress shows in a small window when it takes more than a
//! moment. Undo and Redo share one history with every Files window
//! ([`crate::history`]): Undo here can undo what a window did, and a Files
//! window can undo what the desktop did. Files keeps running while there
//! is something to undo or redo.
//!
//! `/io/elavo/Files/Previewer`, `io.elavo.Files.Previewer`:
//!
//! - `ShowFile(s uri, s window_handle, b close_if_shown)`: Quick Look in a
//!   window of its own; Space, Escape or a second call closes it
//! - `Close()`

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use elavo_files_core::ops::{display_name, Dest, Job, Kind, Outcome};
use elavo_files_core::{eapp, entry, trash};
use gtk::prelude::*;
use gtk::{gdk, gio, glib};

use crate::bundles;
use crate::history::{self, Action, Busy};
use crate::i18n::tr;
use crate::jobs::Jobs;
use crate::quicklook::QuickLook;
use crate::window::fileops::{is_install, quoted, what, Redo, Undo};

pub const OPS_PATH: &str = "/io/elavo/Files/FileOperations";
pub const OPS_IFACE: &str = "io.elavo.Files.FileOperations";
pub const PREVIEW_PATH: &str = "/io/elavo/Files/Previewer";
pub const PREVIEW_IFACE: &str = "io.elavo.Files.Previewer";

const XML: &str = r#"<node>
  <interface name="io.elavo.Files.FileOperations">
    <method name="CopyURIs">
      <arg type="as" name="sources" direction="in"/>
      <arg type="s" name="destination" direction="in"/>
      <arg type="a{sv}" name="platform_data" direction="in"/>
    </method>
    <method name="MoveURIs">
      <arg type="as" name="sources" direction="in"/>
      <arg type="s" name="destination" direction="in"/>
      <arg type="a{sv}" name="platform_data" direction="in"/>
    </method>
    <method name="TrashURIs">
      <arg type="as" name="uris" direction="in"/>
      <arg type="a{sv}" name="platform_data" direction="in"/>
    </method>
    <method name="CreateFolder">
      <arg type="s" name="uri" direction="in"/>
      <arg type="a{sv}" name="platform_data" direction="in"/>
    </method>
    <method name="RenameURI">
      <arg type="s" name="uri" direction="in"/>
      <arg type="s" name="name" direction="in"/>
      <arg type="a{sv}" name="platform_data" direction="in"/>
      <arg type="s" name="new_uri" direction="out"/>
    </method>
    <method name="Undo">
      <arg type="a{sv}" name="platform_data" direction="in"/>
    </method>
    <method name="Redo">
      <arg type="a{sv}" name="platform_data" direction="in"/>
    </method>
    <property name="CanUndo" type="b" access="read"/>
    <property name="CanRedo" type="b" access="read"/>
    <property name="UndoLabel" type="s" access="read"/>
    <property name="RedoLabel" type="s" access="read"/>
  </interface>
  <interface name="io.elavo.Files.Previewer">
    <method name="ShowFile">
      <arg type="s" name="uri" direction="in"/>
      <arg type="s" name="window_handle" direction="in"/>
      <arg type="b" name="close_if_shown" direction="in"/>
    </method>
    <method name="Close"/>
  </interface>
</node>"#;

type PlatformData = HashMap<String, glib::Variant>;

thread_local! {
    static CONN: RefCell<Option<gio::DBusConnection>> = const { RefCell::new(None) };
    static IDS: RefCell<Vec<gio::RegistrationId>> = const { RefCell::new(Vec::new()) };
    static SERVICE: RefCell<Option<Rc<Service>>> = const { RefCell::new(None) };
    static WATCHING: Cell<bool> = const { Cell::new(false) };
    static ANNOUNCED: RefCell<PlatformData> = RefCell::new(HashMap::new());
}

fn log(msg: &str) {
    if std::env::var_os("ELAVO_FILES_LOG").is_some_and(|v| !v.is_empty() && v != "0") {
        eprintln!("elavo-files: {msg}");
    }
}

/// A path or URI as GIO takes it; callers send both.
fn file_of(s: &str) -> gio::File {
    if s.starts_with('/') {
        gio::File::for_path(s)
    } else {
        gio::File::for_uri(s)
    }
}

/// Export both objects on `conn`. Called from the application's D-Bus
/// registration, before Files owns its name, so a call that D-Bus started
/// Files for finds them.
pub fn register(conn: &gio::DBusConnection) {
    let node = match gio::DBusNodeInfo::for_xml(XML) {
        Ok(node) => node,
        Err(e) => {
            log(&format!("service: {e}"));
            return;
        }
    };
    let (Some(ops), Some(preview)) = (
        node.lookup_interface(OPS_IFACE),
        node.lookup_interface(PREVIEW_IFACE),
    ) else {
        return;
    };
    let ops = conn
        .register_object(OPS_PATH, &ops)
        .method_call(|_, sender, _, _, method, params, invocation| {
            log(&format!(
                "FileOperations.{method} from {}",
                sender.unwrap_or("?")
            ));
            ops_call(method, &params, invocation);
        })
        .property(|_, _, _, _, name| {
            let props = undo_properties();
            props
                .get(name)
                .cloned()
                .unwrap_or_else(|| false.to_variant())
        })
        .build();
    let preview = conn
        .register_object(PREVIEW_PATH, &preview)
        .method_call(|_, sender, _, _, method, params, invocation| {
            log(&format!(
                "Previewer.{method} from {}",
                sender.unwrap_or("?")
            ));
            preview_call(method, &params, invocation);
        })
        .build();
    let mut ids = Vec::new();
    for registered in [ops, preview] {
        match registered {
            Ok(id) => ids.push(id),
            Err(e) => log(&format!("service: {e}")),
        }
    }
    IDS.with(|all| all.borrow_mut().extend(ids));
    CONN.with(|c| c.replace(Some(conn.clone())));
    if !WATCHING.with(|w| w.replace(true)) {
        history::watch(|| {
            announce();
            true
        });
    }
}

/// What Undo and Redo can do now, as the properties have it.
fn undo_properties() -> PlatformData {
    HashMap::from([
        ("CanUndo".to_owned(), history::can_undo().to_variant()),
        ("CanRedo".to_owned(), history::can_redo().to_variant()),
        ("UndoLabel".to_owned(), history::undo_label().to_variant()),
        ("RedoLabel".to_owned(), history::redo_label().to_variant()),
    ])
}

/// `PropertiesChanged` when the history changes what Undo or Redo can do,
/// whichever window or caller changed it.
fn announce() {
    let Some(conn) = CONN.with(|c| c.borrow().clone()) else {
        return;
    };
    let props = undo_properties();
    let mut changed: PlatformData = HashMap::new();
    ANNOUNCED.with(|last| {
        let mut last = last.borrow_mut();
        for (name, value) in &props {
            if last.get(name) != Some(value) {
                changed.insert(name.clone(), value.clone());
            }
        }
        *last = props;
    });
    if changed.is_empty() {
        return;
    }
    let _ = conn.emit_signal(
        None,
        OPS_PATH,
        "org.freedesktop.DBus.Properties",
        "PropertiesChanged",
        Some(&(OPS_IFACE, changed, Vec::<String>::new()).to_variant()),
    );
}

/// Take the objects down again, as when another Files instance has the name.
pub fn unregister(conn: &gio::DBusConnection) {
    for id in IDS.with(|all| std::mem::take(&mut *all.borrow_mut())) {
        let _ = conn.unregister_object(id);
    }
    CONN.with(|c| c.replace(None));
}

fn bad_args(invocation: gio::DBusMethodInvocation) {
    invocation.return_error(gio::DBusError::InvalidArgs, "unexpected arguments");
}

fn ops_call(method: &str, params: &glib::Variant, invocation: gio::DBusMethodInvocation) {
    let s = service();
    match method {
        "CopyURIs" | "MoveURIs" => {
            let Some((sources, dest, _)) = params.get::<(Vec<String>, String, PlatformData)>()
            else {
                return bad_args(invocation);
            };
            invocation.return_value(None);
            let files = sources.iter().map(|u| file_of(u)).collect();
            s.transfer(files, file_of(&dest), method == "MoveURIs");
        }
        "TrashURIs" => {
            let Some((uris, _)) = params.get::<(Vec<String>, PlatformData)>() else {
                return bad_args(invocation);
            };
            invocation.return_value(None);
            s.trash_files(uris.iter().map(|u| file_of(u)).collect());
        }
        "CreateFolder" => {
            let Some((uri, _)) = params.get::<(String, PlatformData)>() else {
                return bad_args(invocation);
            };
            let folder = file_of(&uri);
            match folder.make_directory(gio::Cancellable::NONE) {
                Ok(()) => {
                    s.set_undo(Action::NewFolder, Undo::Created(vec![folder]));
                    invocation.return_value(None);
                }
                Err(e) => invocation.return_gerror(e),
            }
        }
        "RenameURI" => {
            let Some((uri, name, _)) = params.get::<(String, String, PlatformData)>() else {
                return bad_args(invocation);
            };
            let file = file_of(&uri);
            let was = display_name(&file);
            match file.set_display_name(&name, gio::Cancellable::NONE) {
                Ok(renamed) => {
                    let new_uri = renamed.uri().to_string();
                    s.set_undo(Action::Rename, Undo::Renamed(renamed, was));
                    invocation.return_value(Some(&(new_uri,).to_variant()));
                }
                Err(e) => invocation.return_gerror(e),
            }
        }
        "Undo" => {
            invocation.return_value(None);
            s.undo();
        }
        "Redo" => {
            invocation.return_value(None);
            s.redo();
        }
        _ => invocation.return_error(gio::DBusError::UnknownMethod, method),
    }
}

fn preview_call(method: &str, params: &glib::Variant, invocation: gio::DBusMethodInvocation) {
    match method {
        "ShowFile" => {
            let Some((uri, _, close_if_shown)) = params.get::<(String, String, bool)>() else {
                return bad_args(invocation);
            };
            invocation.return_value(None);
            service().preview.show(&uri, close_if_shown);
        }
        "Close" => {
            invocation.return_value(None);
            service().preview.close();
        }
        _ => invocation.return_error(gio::DBusError::UnknownMethod, method),
    }
}

fn service() -> Rc<Service> {
    SERVICE.with(|s| {
        s.borrow_mut()
            .get_or_insert_with(|| {
                Rc::new(Service {
                    jobs: Jobs::detached(),
                    preview: Preview::new(),
                })
            })
            .clone()
    })
}

fn hold_app() -> Option<gio::ApplicationHoldGuard> {
    gio::Application::default().map(|a| a.hold())
}

/// An alert with no Files window under it. Files stays running until it
/// is dismissed.
fn alert(message: &str, detail: &str) {
    let dialog = gtk::AlertDialog::builder()
        .message(message)
        .detail(detail)
        .modal(false)
        .build();
    let hold = hold_app();
    glib::spawn_future_local(async move {
        let _ = dialog.choose_future(None::<&gtk::Window>).await;
        drop(hold);
    });
}

/// Errors from an operation, a few at a time.
fn alert_errors(message: &str, errors: &[String]) {
    if errors.is_empty() {
        return;
    }
    let mut detail: Vec<String> = errors.iter().take(8).cloned().collect();
    if errors.len() > 8 {
        detail.push(format!("{} {}", errors.len() - 8, tr("more")));
    }
    alert(message, &detail.join("\n"));
}

/// A yes-or-no question with no window under it.
async fn ask(message: &str, detail: &str, confirm: &str) -> bool {
    let dialog = gtk::AlertDialog::builder()
        .message(message)
        .detail(detail)
        .buttons([tr("Cancel"), confirm.to_owned()])
        .cancel_button(0)
        .default_button(1)
        .build();
    let _hold = hold_app();
    matches!(dialog.choose_future(None::<&gtk::Window>).await, Ok(1))
}

fn results(out: &Outcome) -> Vec<gio::File> {
    out.done.iter().map(|(_, d)| d.clone()).collect()
}

/// `(where it is now, where it was)` for each item an operation moved.
fn swapped(out: &Outcome) -> Vec<(gio::File, gio::File)> {
    out.done
        .iter()
        .map(|(s, d)| (d.clone(), s.clone()))
        .collect()
}

/// Installs are the service's, as in a Files window: one that is running
/// stays, and a shared one is handed to `elavo-applications-ctl remove`.
fn guard_installs(files: Vec<gio::File>) -> Vec<gio::File> {
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
    alert_errors(&tr("Some applications weren't removed."), &errors);
    rest
}

struct Service {
    jobs: Rc<Jobs>,
    preview: Rc<Preview>,
}

impl Service {
    /// A new operation, on the history every Files window shares.
    fn set_undo(&self, action: Action, undo: Undo) {
        history::record(action, undo);
    }

    fn run(
        self: &Rc<Self>,
        job: Job,
        title: String,
        then: impl FnOnce(&Rc<Self>, Outcome) + 'static,
    ) {
        log(&format!("job {:?}: {title}", job.kind));
        let weak = Rc::downgrade(self);
        self.jobs.start(job, title, move |out| {
            if let Some(s) = weak.upgrade() {
                log(&format!(
                    "job done: {} done, {} errors{}",
                    out.done.len(),
                    out.errors.len(),
                    if out.cancelled { ", cancelled" } else { "" }
                ));
                then(&s, out);
            }
        });
    }

    /// Copy or move files into a folder (or the Trash).
    fn transfer(self: &Rc<Self>, files: Vec<gio::File>, dest: gio::File, moving: bool) {
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
            alert_errors(&tr("Some shortcuts couldn't be made."), &errors);
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
            move |s, out| {
                alert_errors(
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
                if moving {
                    s.set_undo(Action::Move, Undo::Moved(swapped(&out)));
                } else {
                    s.set_undo(Action::Copy, Undo::Created(results(&out)));
                }
            },
        );
    }

    fn trash_files(self: &Rc<Self>, files: Vec<gio::File>) {
        self.trash_files_redoing(files, None);
    }

    /// Move to Trash; `redoing` while Redo does it again, so what is left
    /// to redo stays.
    fn trash_files_redoing(self: &Rc<Self>, files: Vec<gio::File>, redoing: Option<Busy>) {
        let files = guard_installs(files);
        if files.is_empty() {
            return;
        }
        let title = format!("{} {} {}", tr("Moving"), what(&files), tr("to the Trash"));
        self.run(
            Job::new(Kind::Trash, files, Dest::Beside),
            title,
            move |s, out| {
                alert_errors(
                    &tr("Some items couldn't be moved to the Trash."),
                    &out.errors,
                );
                if !out.done.is_empty() {
                    let undo = Undo::Trashed(out.done.iter().map(|(o, _)| o.clone()).collect());
                    match redoing {
                        Some(_busy) => history::redone(Action::Trash, undo),
                        None => s.set_undo(Action::Trash, undo),
                    }
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
                    s.confirm_delete(out.untrashable, message);
                }
            },
        );
    }

    /// Ask, then delete for good.
    fn confirm_delete(self: &Rc<Self>, files: Vec<gio::File>, message: String) {
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            if !ask(&message, &tr("This can\u{2019}t be undone."), &tr("Delete")).await {
                return;
            }
            let Some(s) = weak.upgrade() else {
                return;
            };
            let files = guard_installs(files);
            if files.is_empty() {
                return;
            }
            let title = format!("{} {}", tr("Deleting"), what(&files));
            s.run(
                Job::new(Kind::Delete, files, Dest::Beside),
                title,
                |_, out| {
                    alert_errors(&tr("Some items couldn't be deleted."), &out.errors);
                    // Deleted for good: Undo can't go back past this.
                    history::clear();
                },
            );
        });
    }

    /// Undo the most recent operation, the desktop's or a window's.
    fn undo(self: &Rc<Self>) {
        let Some((action, undo, busy)) = history::take_undo() else {
            return;
        };
        let failed = tr("Undo didn't finish.");
        match undo {
            Undo::Moved(pairs) => {
                let (now, was): (Vec<_>, Vec<_>) = pairs.into_iter().unzip();
                let title = format!("{} {}", tr("Moving back"), what(&was));
                self.run(
                    Job::new(Kind::Move, now, Dest::Exactly(was)),
                    title,
                    move |_, out| {
                        let _busy = busy;
                        alert_errors(&failed, &out.errors);
                        if !out.done.is_empty() {
                            history::undone(action, Redo::Move(swapped(&out)));
                        }
                    },
                );
            }
            Undo::Renamed(now, was) => {
                let name = display_name(&now);
                // Nothing else holds Files while the name changes back.
                let hold = hold_app();
                now.set_display_name_async(
                    &was,
                    glib::Priority::DEFAULT,
                    gio::Cancellable::NONE,
                    move |r| {
                        let _hold = hold;
                        let _busy = busy;
                        match r {
                            Ok(file) => history::undone(action, Redo::Rename(file, name)),
                            Err(e) => alert(&failed, e.message()),
                        }
                    },
                );
            }
            Undo::Trashed(origs) => {
                self.restore_to(origs, failed, move |_, out| {
                    let _busy = busy;
                    let results = results(&out);
                    if !results.is_empty() {
                        history::undone(action, Redo::Trash(results));
                    }
                });
            }
            Undo::Created(files) => {
                let title = format!("{} {} {}", tr("Moving"), what(&files), tr("to the Trash"));
                self.run(
                    Job::new(Kind::Trash, files, Dest::Beside),
                    title,
                    move |_, out| {
                        let _busy = busy;
                        alert_errors(&failed, &out.errors);
                        if !out.done.is_empty() {
                            history::undone(
                                action,
                                Redo::Restore(out.done.iter().map(|(o, _)| o.clone()).collect()),
                            );
                        }
                    },
                );
            }
        }
    }

    /// Do again what Undo undid last; Undo then undoes it once more.
    fn redo(self: &Rc<Self>) {
        let Some((action, redo, busy)) = history::take_redo() else {
            return;
        };
        let failed = tr("Redo didn't finish.");
        match redo {
            Redo::Move(pairs) => {
                let (now, to): (Vec<_>, Vec<_>) = pairs.into_iter().unzip();
                let title = format!("{} {}", tr("Moving"), what(&now));
                self.run(
                    Job::new(Kind::Move, now, Dest::Exactly(to)),
                    title,
                    move |_, out| {
                        let _busy = busy;
                        alert_errors(&failed, &out.errors);
                        if !out.done.is_empty() {
                            history::redone(action, Undo::Moved(swapped(&out)));
                        }
                    },
                );
            }
            Redo::Rename(file, name) => {
                let was = display_name(&file);
                let hold = hold_app();
                file.set_display_name_async(
                    &name,
                    glib::Priority::DEFAULT,
                    gio::Cancellable::NONE,
                    move |r| {
                        let _hold = hold;
                        let _busy = busy;
                        match r {
                            Ok(renamed) => history::redone(action, Undo::Renamed(renamed, was)),
                            Err(e) => alert(&failed, e.message()),
                        }
                    },
                );
            }
            Redo::Trash(files) => self.trash_files_redoing(files, Some(busy)),
            Redo::Restore(origs) => {
                self.restore_to(origs, failed, move |_, out| {
                    let _busy = busy;
                    let results = results(&out);
                    if !results.is_empty() {
                        history::redone(action, Undo::Created(results));
                    }
                });
            }
        }
    }

    /// Move items from the Trash back to `origs`, where they were.
    fn restore_to(
        self: &Rc<Self>,
        origs: Vec<gio::File>,
        failed: String,
        then: impl FnOnce(&Rc<Self>, Outcome) + 'static,
    ) {
        let home = trash::home();
        let (mut items, mut targets) = (Vec::new(), Vec::new());
        for orig in origs {
            if let Some(item) = orig.path().and_then(|p| trash::find(&p, &home)) {
                items.push(item);
                targets.push(orig);
            }
        }
        if items.is_empty() {
            alert(&failed, &tr("The items are no longer in the Trash."));
            return;
        }
        let title = format!("{} {}", tr("Putting back"), what(&targets));
        self.run(
            Job::new(Kind::Restore, items, Dest::Exactly(targets)),
            title,
            move |s, out| {
                alert_errors(&failed, &out.errors);
                then(s, out);
            },
        );
    }
}

/// Quick Look for the desktop: Files' own Quick Look, in a window of its
/// own over whatever is on screen.
struct Preview {
    window: gtk::Window,
    look: Rc<QuickLook>,
    uri: RefCell<Option<String>>,
    generation: Cell<u32>,
    /// Held while it shows, so Files stays running with no window of its own.
    hold: RefCell<Option<gio::ApplicationHoldGuard>>,
}

impl Preview {
    fn new() -> Rc<Self> {
        let look = QuickLook::new();
        look.root.set_margin_top(16);
        look.root.set_margin_bottom(16);
        look.root.set_margin_start(16);
        look.root.set_margin_end(16);
        let handle = gtk::WindowHandle::builder().child(&look.root).build();
        let window = gtk::Window::builder()
            .title(tr("Quick Look"))
            .decorated(false)
            .default_width(880)
            .default_height(620)
            .css_classes(["files-quicklook-window"])
            .child(&handle)
            .build();
        let this = Rc::new(Self {
            window,
            look,
            uri: RefCell::default(),
            generation: Cell::new(0),
            hold: RefCell::default(),
        });
        let weak = Rc::downgrade(&this);
        this.look.connect_close(move || {
            if let Some(p) = weak.upgrade() {
                p.close();
            }
        });
        let weak = Rc::downgrade(&this);
        this.look.connect_open(move || {
            if let Some(p) = weak.upgrade() {
                p.open();
            }
        });
        let weak = Rc::downgrade(&this);
        this.window.connect_close_request(move |_| {
            if let Some(p) = weak.upgrade() {
                p.close();
            }
            glib::Propagation::Stop
        });
        let keys = gtk::EventControllerKey::new();
        let weak = Rc::downgrade(&this);
        keys.connect_key_pressed(move |_, key, _, _| {
            if matches!(key, gdk::Key::space | gdk::Key::Escape) {
                if let Some(p) = weak.upgrade() {
                    p.close();
                }
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        this.window.add_controller(keys);
        this
    }

    fn is_showing(&self, uri: &str) -> bool {
        self.window.is_visible() && self.uri.borrow().as_deref() == Some(uri)
    }

    /// Show `uri`; when it shows already, close it if `close_if_shown`.
    fn show(self: &Rc<Self>, uri: &str, close_if_shown: bool) {
        if self.is_showing(uri) {
            if close_if_shown {
                self.close();
            }
            return;
        }
        self.uri.replace(Some(uri.to_owned()));
        let generation = self.generation.get().wrapping_add(1);
        self.generation.set(generation);
        let file = file_of(uri);
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let info = file
                .query_info_future(
                    entry::ATTRIBUTES,
                    gio::FileQueryInfoFlags::NONE,
                    glib::Priority::DEFAULT,
                )
                .await;
            let Some(p) = weak.upgrade().filter(|p| p.generation.get() == generation) else {
                return;
            };
            match info {
                Ok(info) => {
                    p.look.show(&file, &info);
                    if p.hold.borrow().is_none() {
                        p.hold.replace(hold_app());
                    }
                    p.window.present();
                }
                Err(e) => log(&format!("quick look {}: {e}", file.uri())),
            }
        });
    }

    fn close(&self) {
        self.generation.set(self.generation.get().wrapping_add(1));
        self.look.hide();
        self.window.set_visible(false);
        self.uri.replace(None);
        self.hold.replace(None);
    }

    /// Open: a folder in a Files window, anything else in its app.
    fn open(&self) {
        let Some(uri) = self.uri.borrow().clone() else {
            return;
        };
        let file = file_of(&uri);
        let folder = file.query_file_type(gio::FileQueryInfoFlags::NONE, gio::Cancellable::NONE)
            == gio::FileType::Directory;
        let app = gio::Application::default().and_downcast::<gtk::Application>();
        match (folder, app) {
            (true, Some(app)) => {
                crate::app::open_window(&app, Some(file));
            }
            _ => {
                if let Err(e) =
                    gio::AppInfo::launch_default_for_uri(&uri, None::<&gio::AppLaunchContext>)
                {
                    alert(&tr("Couldn\u{2019}t open the item."), e.message());
                }
            }
        }
        self.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interfaces_parse_with_every_method() {
        let node = gio::DBusNodeInfo::for_xml(XML).unwrap();
        let ops = node.lookup_interface(OPS_IFACE).unwrap();
        for method in [
            "CopyURIs",
            "MoveURIs",
            "TrashURIs",
            "CreateFolder",
            "RenameURI",
            "Undo",
            "Redo",
        ] {
            assert!(ops.lookup_method(method).is_some(), "{method}");
        }
        for property in ["CanUndo", "CanRedo", "UndoLabel", "RedoLabel"] {
            assert!(ops.lookup_property(property).is_some(), "{property}");
        }
        let preview = node.lookup_interface(PREVIEW_IFACE).unwrap();
        assert!(preview.lookup_method("ShowFile").is_some());
        assert!(preview.lookup_method("Close").is_some());
    }

    /// The argument shapes the desktop and the panel send.
    #[test]
    fn calls_parse_as_sent() {
        let empty = PlatformData::new();
        let copy = (
            vec!["file:///tmp/a".to_owned()],
            "file:///tmp/b",
            empty.clone(),
        )
            .to_variant();
        let (sources, dest, _) = copy.get::<(Vec<String>, String, PlatformData)>().unwrap();
        assert_eq!(sources, ["file:///tmp/a"]);
        assert_eq!(dest, "file:///tmp/b");
        let trash = (vec!["file:///tmp/a".to_owned()], empty.clone()).to_variant();
        assert!(trash.get::<(Vec<String>, PlatformData)>().is_some());
        let undo = glib::Variant::parse(None, "(@a{sv} {},)").unwrap();
        assert!(undo.get::<(PlatformData,)>().is_some());
        let show = ("file:///tmp/a", "", true).to_variant();
        assert_eq!(
            show.get::<(String, String, bool)>(),
            Some(("file:///tmp/a".to_owned(), String::new(), true))
        );
    }

    #[test]
    fn paths_and_uris_both_name_files() {
        assert_eq!(file_of("/tmp/a b").uri().as_str(), "file:///tmp/a%20b");
        assert_eq!(
            file_of("file:///tmp/a%20b").path().unwrap().to_str(),
            Some("/tmp/a b")
        );
    }
}
