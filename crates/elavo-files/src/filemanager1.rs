//! `org.freedesktop.FileManager1`, the interface other apps use to show a
//! folder or a file in the file manager: a browser's Show in Folder, a
//! download's Open Containing Folder, Show in Files from an editor.
//!
//! - `ShowFolders(as uris, s startup_id)` opens a window for each folder.
//! - `ShowItems(as uris, s startup_id)` shows each item selected in its
//!   folder, in a window already showing that folder when there is one.
//! - `ShowItemProperties(as uris, s startup_id)` does that and opens Get
//!   Info for each.
//!
//! Files takes the name when it starts (or is started for it by D-Bus, with
//! `--gapplication-service`), replacing another file manager that allows it.

use gtk::gio;
use gtk::prelude::*;

const NAME: &str = "org.freedesktop.FileManager1";
const PATH: &str = "/org/freedesktop/FileManager1";
const XML: &str = r#"<node>
  <interface name="org.freedesktop.FileManager1">
    <method name="ShowFolders">
      <arg type="as" name="URIs" direction="in"/>
      <arg type="s" name="StartupId" direction="in"/>
    </method>
    <method name="ShowItems">
      <arg type="as" name="URIs" direction="in"/>
      <arg type="s" name="StartupId" direction="in"/>
    </method>
    <method name="ShowItemProperties">
      <arg type="as" name="URIs" direction="in"/>
      <arg type="s" name="StartupId" direction="in"/>
    </method>
  </interface>
</node>"#;

fn log(msg: &str) {
    if std::env::var_os("ELAVO_FILES_LOG").is_some_and(|v| !v.is_empty() && v != "0") {
        eprintln!("elavo-files: {msg}");
    }
}

/// What a call asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Show {
    Folders,
    Items,
    Properties,
}

/// A path or URI as GIO takes it; callers send both.
fn file_of(s: &str) -> gio::File {
    if s.starts_with('/') {
        gio::File::for_path(s)
    } else {
        gio::File::for_uri(s)
    }
}

pub fn start(app: &gtk::Application) {
    let Some(conn) = app.dbus_connection() else {
        return;
    };
    let iface = match gio::DBusNodeInfo::for_xml(XML)
        .ok()
        .and_then(|n| n.lookup_interface(NAME))
    {
        Some(i) => i,
        None => return,
    };
    let weak = app.downgrade();
    let registered = conn
        .register_object(PATH, &iface)
        .method_call(move |_, sender, _, _, method, params, invocation| {
            let show = match method {
                "ShowFolders" => Show::Folders,
                "ShowItems" => Show::Items,
                "ShowItemProperties" => Show::Properties,
                _ => {
                    invocation.return_error(gio::DBusError::UnknownMethod, method);
                    return;
                }
            };
            let (uris, startup) = params.get::<(Vec<String>, String)>().unwrap_or_default();
            log(&format!(
                "FileManager1.{method} from {} ({} items)",
                sender.unwrap_or("?"),
                uris.len()
            ));
            invocation.return_value(None);
            let files: Vec<gio::File> = uris.iter().map(|u| file_of(u)).collect();
            if let Some(app) = weak.upgrade() {
                let id = (!startup.is_empty()).then_some(startup);
                crate::app::show(&app, show, &files, id.as_deref());
            }
        })
        .build();
    if let Err(e) = registered {
        log(&format!("FileManager1: {e}"));
        return;
    }
    // Owned for the life of the app; the id is never released.
    let _owner = gio::bus_own_name_on_connection(
        &conn,
        NAME,
        gio::BusNameOwnerFlags::REPLACE | gio::BusNameOwnerFlags::ALLOW_REPLACEMENT,
        |_, name| log(&format!("{name}: Files is the file manager")),
        |_, name| log(&format!("{name}: another file manager has it")),
    );
}
