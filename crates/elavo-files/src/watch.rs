//! Refresh a listed file when it finishes writing.
//!
//! `gtk::DirectoryList` follows creates, deletes, renames and attribute
//! changes, but in GTK 4.24 it ignores content writes (`CHANGED` and
//! `CHANGES_DONE_HINT`). A screenshot saved into the open folder would
//! otherwise keep the size and icon from when the empty file first
//! appeared, until the folder was listed again.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use elavo_files_core::entry;
use gtk::prelude::*;
use gtk::{gio, glib};

use crate::thumbs;
use crate::views::{self, Cells};

const CHANGED_DELAY: Duration = Duration::from_millis(300);
const CREATED_DELAY: Duration = Duration::from_millis(400);
const MISSING_DELAY: Duration = Duration::from_millis(200);

struct Watch {
    dir: gtk::DirectoryList,
    cells: Rc<Cells>,
    monitor: RefCell<Option<gio::FileMonitor>>,
    pending: RefCell<HashMap<String, u32>>,
}

/// Follow content writes in `dir`, and catch a create whose first query
/// ran before the file had its contents. Safe to call once per list.
pub fn attach(dir: &gtk::DirectoryList, cells: Rc<Cells>) {
    // SAFETY: the key holds a plain bool and is only read with that type.
    unsafe {
        if dir.data::<bool>("elavo-files-watch").is_some() {
            return;
        }
        dir.set_data("elavo-files-watch", true);
    }
    let watch = Rc::new(Watch {
        dir: dir.clone(),
        cells,
        monitor: RefCell::default(),
        pending: RefCell::default(),
    });
    let w = watch.clone();
    dir.connect_notify_local(Some("file"), move |_, _| w.remonitor());
    let w = watch.clone();
    dir.connect_items_changed(move |dir, position, _, added| {
        if added == 0 || dir.is_loading() {
            return;
        }
        w.check_added(position, added);
    });
    watch.remonitor();
}

impl Watch {
    fn remonitor(self: &Rc<Self>) {
        self.monitor.replace(None);
        self.pending.borrow_mut().clear();
        let Some(file) = self.dir.file() else {
            return;
        };
        let Ok(monitor) =
            file.monitor_directory(gio::FileMonitorFlags::WATCH_MOVES, gio::Cancellable::NONE)
        else {
            return;
        };
        let watch = Rc::downgrade(self);
        monitor.connect_changed(move |_, file, other, event| {
            let Some(watch) = watch.upgrade() else {
                return;
            };
            watch.on_event(file, other, event);
        });
        self.monitor.replace(Some(monitor));
    }

    fn on_event(
        self: &Rc<Self>,
        file: &gio::File,
        other: Option<&gio::File>,
        event: gio::FileMonitorEvent,
    ) {
        if self.dir.file().is_some_and(|dir| dir.equal(file)) {
            return;
        }
        match event {
            gio::FileMonitorEvent::ChangesDoneHint => self.schedule(file, Duration::ZERO, true),
            gio::FileMonitorEvent::Changed => self.schedule(file, CHANGED_DELAY, true),
            gio::FileMonitorEvent::Created | gio::FileMonitorEvent::MovedIn => {
                self.schedule(file, CREATED_DELAY, true);
            }
            gio::FileMonitorEvent::Renamed => {
                if let Some(other) = other {
                    self.schedule(other, CREATED_DELAY, true);
                }
            }
            _ => {}
        }
    }

    fn schedule(self: &Rc<Self>, file: &gio::File, delay: Duration, retry: bool) {
        let uri = file.uri().to_string();
        let gen = {
            let mut pending = self.pending.borrow_mut();
            let gen = pending.get(&uri).copied().unwrap_or(0).wrapping_add(1);
            pending.insert(uri.clone(), gen);
            gen
        };
        let file = file.clone();
        let watch = Rc::downgrade(self);
        let run = move || {
            let Some(watch) = watch.upgrade() else {
                return;
            };
            {
                let mut pending = watch.pending.borrow_mut();
                if pending.get(&uri) != Some(&gen) {
                    return;
                }
                pending.remove(&uri);
            }
            watch.refresh(&file, retry);
        };
        if delay.is_zero() {
            glib::idle_add_local_once(run);
        } else {
            glib::timeout_add_local_once(delay, run);
        }
    }

    fn check_added(&self, position: u32, added: u32) {
        for i in position..position + added {
            let Some(info) = self.dir.item(i).and_downcast::<gio::FileInfo>() else {
                continue;
            };
            let Some(file) = entry::file(&info) else {
                continue;
            };
            let Ok(fresh) = file.query_info(
                entry::ATTRIBUTES,
                gio::FileQueryInfoFlags::NONE,
                gio::Cancellable::NONE,
            ) else {
                continue;
            };
            if entry::stale(&info, &fresh) {
                self.apply(&info, &fresh, &file);
            }
        }
    }

    fn refresh(self: &Rc<Self>, file: &gio::File, retry: bool) {
        let Some(info) = find(&self.dir, file) else {
            if retry && still_here(&self.dir, file) {
                self.schedule(file, MISSING_DELAY, false);
            }
            return;
        };
        let Ok(fresh) = file.query_info(
            entry::ATTRIBUTES,
            gio::FileQueryInfoFlags::NONE,
            gio::Cancellable::NONE,
        ) else {
            return;
        };
        if !entry::stale(&info, &fresh) {
            return;
        }
        self.apply(&info, &fresh, file);
    }

    fn apply(&self, info: &gio::FileInfo, fresh: &gio::FileInfo, file: &gio::File) {
        entry::refresh_from(info, fresh);
        thumbs::forget_failure(&file.uri());
        views::refresh_cells(&self.cells, info);
    }
}

fn find(dir: &gtk::DirectoryList, file: &gio::File) -> Option<gio::FileInfo> {
    for i in 0..dir.n_items() {
        let info = dir.item(i).and_downcast::<gio::FileInfo>()?;
        if entry::file(&info).is_some_and(|f| f.equal(file)) {
            return Some(info);
        }
    }
    None
}

fn still_here(dir: &gtk::DirectoryList, file: &gio::File) -> bool {
    file.query_exists(gio::Cancellable::NONE)
        && dir
            .file()
            .zip(file.parent())
            .is_some_and(|(dir, parent)| dir.equal(&parent))
}
