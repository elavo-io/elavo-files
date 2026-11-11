//! Starting and managing Elavo application bundles (`.eapp`), the way
//! elavo-applications' Nautilus module does. What a bundle is, and its name
//! and icon, are in `elavo_files_core::eapp`.

use std::path::{Path, PathBuf};

use elavo_files_core::eapp;
use gtk::prelude::*;
use gtk::{gio, glib};

use crate::i18n::tr;

extern "C" {
    // GDesktopAppInfo lives in libgio on Unix; gio-rs moved its binding to
    // a separate crate, which this one call does not justify.
    fn g_desktop_app_info_new_from_filename(
        filename: *const std::os::raw::c_char,
    ) -> *mut gio::ffi::GAppInfo;
}

/// The application described by a desktop file.
fn desktop_app(path: &Path) -> Option<gio::AppInfo> {
    use glib::translate::{from_glib_full, ToGlibPtr};
    // SAFETY: the function takes a NUL-terminated file name and returns a
    // new reference to a GDesktopAppInfo (a GAppInfo) or NULL.
    unsafe {
        let ptr = g_desktop_app_info_new_from_filename(path.to_glib_none().0);
        (!ptr.is_null()).then(|| from_glib_full(ptr))
    }
}

/// Run `ctl args...`, returning its standard error on failure.
fn ctl(args: &[&std::ffi::OsStr]) -> Result<String, String> {
    let out = std::process::Command::new(eapp::ctl())
        .args(args)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| format!("{}: {e}", eapp::ctl()))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
    } else {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_owned();
        Err(if err.is_empty() {
            tr("The application service didn't answer.")
        } else {
            err
        })
    }
}

/// Start the application. An install starts from its `launch.desktop`; a
/// copy elsewhere goes through the service, which can put a missing
/// Flatpak back first.
pub fn launch(window: &impl IsA<gtk::Widget>, bundle: &Path) -> Result<(), String> {
    if !eapp::in_applications(bundle) {
        return ctl(&["open".as_ref(), bundle.as_os_str()]).map(|_| ());
    }
    let desktop = bundle.join("launch.desktop");
    let app = desktop_app(&desktop)
        .ok_or_else(|| format!("{} {}", tr("Can't read"), desktop.display()))?;
    let context = window.as_ref().display().app_launch_context();
    app.launch(&[], Some(&context))
        .map_err(|e| e.message().to_owned())
}

/// Whether the bundle's application is running. False when the service
/// can't say.
pub fn running(bundle: &Path) -> bool {
    std::process::Command::new(eapp::ctl())
        .args(["running".as_ref(), bundle.as_os_str()])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Ask the service to remove a shared install. It stays until Uninstall.
pub fn remove(bundle: &Path) -> Result<(), String> {
    ctl(&["remove".as_ref(), bundle.as_os_str()]).map(|_| ())
}

/// A shortcut file standing for an install, made by the service in a new
/// folder under the runtime directory, for a drop or paste to copy instead
/// of the install itself.
pub fn shortcut(bundle: &Path) -> Result<gio::File, String> {
    let root = glib::user_runtime_dir()
        .join("elavo-files")
        .join("shortcuts");
    let _ = std::fs::create_dir_all(&root);
    // Folders from earlier drops go once they are an hour old.
    for e in std::fs::read_dir(&root).into_iter().flatten().flatten() {
        let old = e
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age.as_secs() > 3600);
        if old {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
    let dir: PathBuf = (0..1000)
        .map(|i| root.join(format!("{}-{i}", std::process::id())))
        .find(|d| std::fs::create_dir(d).is_ok())
        .ok_or_else(|| tr("Couldn't make a shortcut."))?;
    let out = ctl(&["shortcut".as_ref(), bundle.as_os_str(), dir.as_os_str()])?;
    if out.is_empty() {
        return Err(tr("Couldn't make a shortcut."));
    }
    Ok(gio::File::for_path(out))
}
