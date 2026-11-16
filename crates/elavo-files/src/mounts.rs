//! Network shares and drives, through GIO and GVfs: mounting a location
//! that needs it (GTK's mount operation asks for a name and password when
//! the server wants them; GVfs and the keyring remember them), and Eject.
//!
//! Eject unmounts, ejects the media where it can, and powers the drive off
//! when it can be removed safely, as GNOME's Safely Remove does. A drive
//! in use shows which apps hold it (GTK's mount operation does that), or
//! offers Force Eject when that isn't known. Windows showing a place on it
//! go Home first, so Files doesn't keep it busy itself.
//!
//! Addresses are logged with any password taken out; credentials are
//! never logged.

use elavo_files_core::servers;
use gtk::prelude::*;
use gtk::{gio, glib};

use crate::i18n::tr;

fn log(msg: &str) {
    if std::env::var_os("ELAVO_FILES_LOG").is_some_and(|v| !v.is_empty() && v != "0") {
        eprintln!("elavo-files: {msg}");
    }
}

/// An address safe to log or show.
pub fn shown(file: &gio::File) -> String {
    servers::without_password(&file.uri())
}

/// The person closed the password prompt, or something already said why.
pub fn quietly_cancelled(e: &glib::Error) -> bool {
    e.matches(gio::IOErrorEnum::FailedHandled) || e.matches(gio::IOErrorEnum::Cancelled)
}

/// Mount the location `file` is in, then call `done`. Already mounted is
/// done too.
pub fn mount(
    parent: Option<&gtk::Window>,
    file: &gio::File,
    done: impl FnOnce(Result<(), glib::Error>) + 'static,
) {
    let op = gtk::MountOperation::new(parent);
    log(&format!("mounting {}", shown(file)));
    let what = shown(file);
    file.mount_enclosing_volume(
        gio::MountMountFlags::NONE,
        Some(&op),
        gio::Cancellable::NONE,
        move |r| {
            let r = match r {
                Err(e) if e.matches(gio::IOErrorEnum::AlreadyMounted) => Ok(()),
                r => r,
            };
            match &r {
                Ok(()) => log(&format!("mounted {what}")),
                Err(e) => log(&format!("mounting {what} failed: {}", e.message())),
            }
            done(r)
        },
    );
}

/// An item that is itself mountable (a share in a server's listing, say):
/// mount it and give where it mounted.
pub fn mount_mountable(
    parent: Option<&gtk::Window>,
    file: &gio::File,
    done: impl FnOnce(Result<gio::File, glib::Error>) + 'static,
) {
    let op = gtk::MountOperation::new(parent);
    log(&format!("mounting {}", shown(file)));
    let f = file.clone();
    file.mount_mountable(
        gio::MountMountFlags::NONE,
        Some(&op),
        gio::Cancellable::NONE,
        move |r| {
            done(match r {
                Err(e) if e.matches(gio::IOErrorEnum::AlreadyMounted) => Ok(f),
                r => r,
            })
        },
    );
}

/// A drive volume that isn't mounted yet (a USB partition, say): mount it
/// and give its root.
pub fn mount_volume(
    parent: Option<&gtk::Window>,
    volume: &gio::Volume,
    done: impl FnOnce(Result<gio::File, glib::Error>) + 'static,
) {
    let op = gtk::MountOperation::new(parent);
    log(&format!("mounting volume {}", volume.name()));
    let v = volume.clone();
    volume.mount(
        gio::MountMountFlags::NONE,
        Some(&op),
        gio::Cancellable::NONE,
        move |r| {
            let r = match r {
                Err(e) if e.matches(gio::IOErrorEnum::AlreadyMounted) => Ok(()),
                r => r,
            };
            done(r.and_then(|()| {
                v.get_mount().map(|m| m.root()).ok_or_else(|| {
                    glib::Error::new(gio::IOErrorEnum::NotMounted, &tr("It didn\u{2019}t mount."))
                })
            }))
        },
    );
}

fn monitor() -> gio::VolumeMonitor {
    gio::VolumeMonitor::get()
}

/// Whether the sidebar shows `mount` with an Eject button.
pub fn can_eject(mount: &gio::Mount) -> bool {
    !mount.is_shadowed() && (mount.can_eject() || mount.can_unmount())
}

/// The ejectable mount `file` is on.
pub fn ejectable(file: &gio::File) -> Option<gio::Mount> {
    monitor().mounts().into_iter().find(|m| {
        let root = m.root();
        can_eject(m) && (root.equal(file) || file.has_prefix(&root))
    })
}

/// The ejectable mount `file` is the top of.
pub fn ejectable_root(file: &gio::File) -> Option<gio::Mount> {
    monitor()
        .mounts()
        .into_iter()
        .find(|m| can_eject(m) && m.root().equal(file))
}

/// Volumes on drives that can be mounted but aren't, for the sidebar.
pub fn unmounted_volumes() -> Vec<gio::Volume> {
    monitor()
        .volumes()
        .into_iter()
        .filter(|v| v.get_mount().is_none() && v.can_mount())
        .collect()
}

pub fn volume_by_id(id: &str) -> Option<gio::Volume> {
    monitor().volumes().into_iter().find(|v| volume_id(v) == id)
}

/// A stable name for a volume: its UUID, or else its device or name.
pub fn volume_id(v: &gio::Volume) -> String {
    v.uuid()
        .or_else(|| v.identifier(gio::VOLUME_IDENTIFIER_KIND_UNIX_DEVICE))
        .map(|s| s.to_string())
        .unwrap_or_else(|| v.name().to_string())
}

/// Eject `mount`: see the module documentation.
pub fn eject(parent: Option<&gtk::Window>, mount: &gio::Mount) {
    eject_with(parent.cloned(), mount.clone(), false);
}

fn eject_with(parent: Option<gtk::Window>, mount: gio::Mount, force: bool) {
    let name = crate::sidebar::mount_label(&mount);
    let root = mount.root();
    log(&format!(
        "ejecting {name} ({}){}",
        shown(&root),
        if force { ", forced" } else { "" }
    ));
    crate::app::leave(&root);
    let op = gtk::MountOperation::new(parent.as_ref());
    let flags = if force {
        gio::MountUnmountFlags::FORCE
    } else {
        gio::MountUnmountFlags::NONE
    };
    let (p, m, n) = (parent.clone(), mount.clone(), name.clone());
    let done = move |r: Result<(), glib::Error>| match r {
        Ok(()) => log(&format!("ejected {n}")),
        Err(e) if quietly_cancelled(&e) => log(&format!("ejecting {n} cancelled")),
        Err(e) if e.matches(gio::IOErrorEnum::Busy) && !force => {
            log(&format!("{n} is busy: {}", e.message()));
            offer_force(p, m, n, e.message().to_owned());
        }
        Err(e) => {
            log(&format!("ejecting {n} failed: {}", e.message()));
            let alert = gtk::AlertDialog::builder()
                .message(format!(
                    "\u{201c}{n}\u{201d} {}",
                    tr("couldn\u{2019}t be ejected.")
                ))
                .detail(e.message())
                .modal(true)
                .build();
            alert.show(p.as_ref());
        }
    };
    // A removable drive with nothing else mounted on it: unmount, eject and
    // power it off in one go.
    if let Some(drive) = mount.drive().filter(|d| {
        d.can_stop()
            && d.is_removable()
            && d.volumes()
                .iter()
                .filter_map(|v| v.get_mount())
                .all(|other| other == mount)
    }) {
        drive.stop(flags, Some(&op), gio::Cancellable::NONE, done);
    } else if mount.can_eject() {
        mount.eject_with_operation(flags, Some(&op), gio::Cancellable::NONE, done);
    } else {
        mount.unmount_with_operation(flags, Some(&op), gio::Cancellable::NONE, done);
    }
}

fn offer_force(parent: Option<gtk::Window>, mount: gio::Mount, name: String, why: String) {
    let alert = gtk::AlertDialog::builder()
        .message(format!(
            "\u{201c}{name}\u{201d} {}",
            tr("can\u{2019}t be ejected because it\u{2019}s in use.")
        ))
        .detail(format!(
            "{why}\n\n{}",
            tr("Quit the apps using it and try again, or eject it anyway. Files open on it may lose changes.")
        ))
        .buttons([tr("Cancel"), tr("Force Eject")])
        .cancel_button(0)
        .default_button(0)
        .modal(true)
        .build();
    let p = parent.clone();
    alert.choose(parent.as_ref(), gio::Cancellable::NONE, move |r| {
        if matches!(r, Ok(1)) {
            eject_with(p, mount, true);
        }
    });
}
