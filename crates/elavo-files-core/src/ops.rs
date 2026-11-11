//! File operations: copy, move, trash, delete, Put Back, Compress and
//! Empty Trash. A job runs on a worker thread with GIO's blocking calls
//! (`gio::File` is thread safe) and talks to the window through [`Ui`]:
//! progress as it goes, and a question when a name is taken. Cancelling
//! the `gio::Cancellable` stops it between, or in the middle of, files.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use gio::glib;
use gio::prelude::*;

use crate::{naming, trash};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Copy,
    Move,
    Trash,
    Delete,
    Restore,
    Compress,
    EmptyTrash,
}

/// Where each source goes.
#[derive(Debug, Clone)]
pub enum Dest {
    /// Into this folder, under the same name.
    Into(gio::File),
    /// Beside the source, as a copy (Duplicate).
    Beside,
    /// To these exact paths, one per source (Put Back, Undo).
    Exactly(Vec<gio::File>),
}

#[derive(Debug, Clone)]
pub struct Job {
    pub kind: Kind,
    pub sources: Vec<gio::File>,
    pub dest: Dest,
    /// The home trash, read directly when GVfs is missing.
    pub trash: PathBuf,
}

impl Job {
    pub fn new(kind: Kind, sources: Vec<gio::File>, dest: Dest) -> Self {
        Self {
            kind,
            sources,
            dest,
            trash: trash::home(),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Progress {
    pub done_bytes: u64,
    pub total_bytes: u64,
    pub done_items: usize,
    pub total_items: usize,
    /// The name of the item in hand.
    pub current: String,
}

impl Progress {
    /// From 0 to 1, or `None` when the amount of work isn't known.
    pub fn fraction(&self) -> Option<f64> {
        if self.total_bytes > 0 {
            Some((self.done_bytes as f64 / self.total_bytes as f64).min(1.0))
        } else if self.total_items > 0 {
            Some((self.done_items as f64 / self.total_items as f64).min(1.0))
        } else {
            None
        }
    }
}

/// Size, date and type of one side of a name conflict.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Stat {
    pub size: u64,
    pub modified: i64,
    pub is_dir: bool,
}

#[derive(Debug, Clone)]
pub struct Conflict {
    pub name: String,
    /// The folder it is going into.
    pub folder: String,
    pub incoming: Stat,
    pub existing: Stat,
    pub moving: bool,
    /// Items still to go after this one, so Apply to All is worth showing.
    pub remaining: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolution {
    KeepBoth,
    Replace,
    Skip,
    Stop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Answer {
    pub resolution: Resolution,
    pub all: bool,
}

/// Remembers an Apply to All answer for the rest of a job.
#[derive(Debug, Default)]
pub struct Policy {
    all: Option<Resolution>,
}

impl Policy {
    pub fn resolve(&mut self, ask: impl FnOnce() -> Answer) -> Resolution {
        if let Some(r) = self.all {
            return r;
        }
        let answer = ask();
        if answer.all && answer.resolution != Resolution::Stop {
            self.all = Some(answer.resolution);
        }
        answer.resolution
    }
}

pub trait Ui {
    fn progress(&mut self, progress: &Progress);
    fn conflict(&mut self, conflict: &Conflict) -> Answer;
}

/// What a job did: `(source, result)` for each item that went through
/// (for Trash, the original location twice), and what went wrong.
#[derive(Debug, Default)]
pub struct Outcome {
    pub done: Vec<(gio::File, gio::File)>,
    pub errors: Vec<String>,
    /// Items whose filesystem has no Trash (such as tmpfs), which Trash
    /// leaves alone so the caller can offer to delete them instead.
    pub untrashable: Vec<gio::File>,
    pub cancelled: bool,
}

enum Step {
    Done(gio::File),
    Skipped,
    Stop,
}

const NOFOLLOW: gio::FileQueryInfoFlags = gio::FileQueryInfoFlags::NOFOLLOW_SYMLINKS;
const REPORT_EVERY: Duration = Duration::from_millis(80);

pub fn display_name(file: &gio::File) -> String {
    file.basename()
        .map(|b| b.to_string_lossy().into_owned())
        .unwrap_or_else(|| file.uri().to_string())
}

fn exists(file: &gio::File) -> bool {
    file.query_file_type(NOFOLLOW, gio::Cancellable::NONE) != gio::FileType::Unknown
}

fn stat(file: &gio::File) -> Stat {
    file.query_info(
        "standard::type,standard::size,time::modified",
        NOFOLLOW,
        gio::Cancellable::NONE,
    )
    .map(|i| Stat {
        size: i.size().max(0) as u64,
        modified: i.modification_date_time().map_or(0, |d| d.to_unix()),
        is_dir: i.file_type() == gio::FileType::Directory,
    })
    .unwrap_or_default()
}

fn report(p: &Progress, last: &mut Instant, ui: &mut dyn Ui, force: bool) {
    if force || last.elapsed() >= REPORT_EVERY {
        *last = Instant::now();
        ui.progress(p);
    }
}

struct Runner<'a> {
    cancel: &'a gio::Cancellable,
    ui: &'a mut dyn Ui,
    p: Progress,
    last: Instant,
    policy: Policy,
    out: Outcome,
    trash: PathBuf,
}

pub fn run(job: &Job, cancel: &gio::Cancellable, ui: &mut dyn Ui) -> Outcome {
    let mut r = Runner {
        cancel,
        ui,
        p: Progress::default(),
        last: Instant::now(),
        policy: Policy::default(),
        out: Outcome::default(),
        trash: job.trash.clone(),
    };
    match job.kind {
        Kind::Copy | Kind::Move | Kind::Restore => r.transfer(job),
        Kind::Trash => r.each(&job.sources, |r, f| match f.trash(Some(r.cancel)) {
            Ok(()) => Ok(Some(f.clone())),
            Err(e) if e.matches(gio::IOErrorEnum::NotSupported) => {
                r.out.untrashable.push(f.clone());
                Ok(None)
            }
            Err(e) => Err(e),
        }),
        Kind::Delete => r.each(&job.sources, |r, f| {
            r.delete_tree(f)?;
            trash::forget(f, &r.trash);
            Ok(Some(f.clone()))
        }),
        Kind::EmptyTrash => {
            let items = trash::items(&job.trash);
            r.each(&items, |r, f| {
                r.delete_tree(f)?;
                trash::forget(f, &r.trash);
                Ok(Some(f.clone()))
            })
        }
        Kind::Compress => r.compress(&job.sources),
    }
    r.out.cancelled = cancel.is_cancelled();
    r.out
}

impl Runner<'_> {
    fn error(&mut self, file: &gio::File, err: &glib::Error) {
        self.out.errors.push(format!(
            "\u{201c}{}\u{201d}: {}",
            display_name(file),
            err.message()
        ));
    }

    fn report(&mut self, force: bool) {
        report(&self.p, &mut self.last, &mut *self.ui, force);
    }

    /// One step per item, counting items: Trash, Delete, Empty Trash.
    fn each(
        &mut self,
        files: &[gio::File],
        mut f: impl FnMut(&mut Self, &gio::File) -> Result<Option<gio::File>, glib::Error>,
    ) {
        self.p.total_items = files.len();
        for file in files {
            if self.cancel.is_cancelled() {
                break;
            }
            self.p.current = display_name(file);
            self.report(true);
            match f(self, file) {
                Ok(Some(result)) => self.out.done.push((file.clone(), result)),
                Ok(None) => {}
                Err(e) if e.matches(gio::IOErrorEnum::Cancelled) => break,
                Err(e) => self.error(file, &e),
            }
            self.p.done_items += 1;
        }
        self.report(true);
    }

    fn size_tree(&self, file: &gio::File) -> u64 {
        let Ok(info) =
            file.query_info("standard::type,standard::size", NOFOLLOW, Some(self.cancel))
        else {
            return 0;
        };
        if info.file_type() != gio::FileType::Directory {
            return info.size().max(0) as u64;
        }
        let Ok(children) = file.enumerate_children("standard::name", NOFOLLOW, Some(self.cancel))
        else {
            return 0;
        };
        children
            .flatten()
            .map(|c| self.size_tree(&file.child(c.name())))
            .sum()
    }

    fn delete_tree(&self, file: &gio::File) -> Result<(), glib::Error> {
        if file.query_file_type(NOFOLLOW, Some(self.cancel)) == gio::FileType::Directory {
            for child in file.enumerate_children("standard::name", NOFOLLOW, Some(self.cancel))? {
                self.delete_tree(&file.child(child?.name()))?;
            }
        }
        file.delete(Some(self.cancel))
    }

    fn copy_tree(
        &mut self,
        src: &gio::File,
        dst: &gio::File,
        overwrite: bool,
    ) -> Result<(), glib::Error> {
        let info = src.query_info("standard::type,standard::size", NOFOLLOW, Some(self.cancel))?;
        if info.file_type() == gio::FileType::Directory {
            match dst.make_directory(Some(self.cancel)) {
                Ok(()) => {}
                Err(e) if overwrite && e.matches(gio::IOErrorEnum::Exists) => {}
                Err(e) => return Err(e),
            }
            for child in src.enumerate_children("standard::name", NOFOLLOW, Some(self.cancel))? {
                let name = child?.name();
                self.copy_tree(&src.child(&name), &dst.child(&name), false)?;
            }
            let _ = src.copy_attributes(
                dst,
                gio::FileCopyFlags::ALL_METADATA | gio::FileCopyFlags::NOFOLLOW_SYMLINKS,
                Some(self.cancel),
            );
            return Ok(());
        }
        let base = self.p.done_bytes;
        let mut flags = gio::FileCopyFlags::NOFOLLOW_SYMLINKS | gio::FileCopyFlags::ALL_METADATA;
        if overwrite {
            flags |= gio::FileCopyFlags::OVERWRITE;
        }
        {
            let (p, last, ui) = (&mut self.p, &mut self.last, &mut *self.ui);
            let mut progress = |current: i64, _total: i64| {
                p.done_bytes = base + current.max(0) as u64;
                report(p, last, ui, false);
            };
            src.copy(dst, flags, Some(self.cancel), Some(&mut progress))?;
        }
        self.p.done_bytes = base + info.size().max(0) as u64;
        Ok(())
    }

    fn transfer(&mut self, job: &Job) {
        let moving = job.kind != Kind::Copy;
        let sizes: Vec<u64> = job.sources.iter().map(|s| self.size_tree(s)).collect();
        self.p.total_bytes = sizes.iter().sum();
        self.p.total_items = job.sources.len();
        for (i, src) in job.sources.iter().enumerate() {
            if self.cancel.is_cancelled() {
                break;
            }
            let (dir, name) = match &job.dest {
                Dest::Into(dir) => (Some(dir.clone()), display_name(src)),
                Dest::Beside => (src.parent(), display_name(src)),
                Dest::Exactly(targets) => match targets.get(i) {
                    Some(t) => (t.parent(), display_name(t)),
                    None => (None, String::new()),
                },
            };
            let Some(dir) = dir else {
                continue;
            };
            if job.kind == Kind::Restore {
                let _ = dir.make_directory_with_parents(Some(self.cancel));
            }
            self.p.current = display_name(src);
            self.report(true);
            let base = self.p.done_bytes;
            let remaining = job.sources.len() - i - 1;
            match self.transfer_one(src, &dir, &name, moving, remaining) {
                Ok(Step::Done(result)) => {
                    if job.kind == Kind::Restore {
                        trash::forget(src, &self.trash);
                    }
                    self.out.done.push((src.clone(), result));
                }
                Ok(Step::Skipped) => {}
                Ok(Step::Stop) => break,
                Err(e) if e.matches(gio::IOErrorEnum::Cancelled) => break,
                Err(e) => self.error(src, &e),
            }
            self.p.done_bytes = base + sizes[i];
            self.p.done_items += 1;
        }
        self.report(true);
    }

    fn transfer_one(
        &mut self,
        src: &gio::File,
        dir: &gio::File,
        name: &str,
        moving: bool,
        remaining: usize,
    ) -> Result<Step, glib::Error> {
        let src_is_dir =
            src.query_file_type(NOFOLLOW, Some(self.cancel)) == gio::FileType::Directory;
        let taken = |n: &str| exists(&dir.child(n));
        let mut target = dir.child(name);
        if target.equal(src) {
            if moving {
                return Ok(Step::Skipped);
            }
            // Copying something onto itself makes a copy beside it.
            target = dir.child(naming::duplicate(name, src_is_dir, taken));
        }
        if target.has_prefix(src) {
            return Err(glib::Error::new(
                gio::IOErrorEnum::InvalidArgument,
                "A folder can't be put inside itself.",
            ));
        }
        let mut replace = false;
        if exists(&target) {
            let conflict = Conflict {
                name: name.to_owned(),
                folder: display_name(dir),
                incoming: stat(src),
                existing: stat(&target),
                moving,
                remaining,
            };
            let ui = &mut *self.ui;
            match self.policy.resolve(|| ui.conflict(&conflict)) {
                Resolution::Skip => return Ok(Step::Skipped),
                Resolution::Stop => {
                    self.cancel.cancel();
                    return Ok(Step::Stop);
                }
                Resolution::KeepBoth => {
                    target = dir.child(naming::keep_both(name, src_is_dir, taken));
                }
                Resolution::Replace => {
                    if target.has_prefix(src) || src.has_prefix(&target) {
                        return Err(glib::Error::new(
                            gio::IOErrorEnum::InvalidArgument,
                            "An item can't replace a folder it is inside.",
                        ));
                    }
                    if src_is_dir || conflict.existing.is_dir {
                        self.delete_tree(&target)?;
                    } else {
                        replace = true;
                    }
                }
            }
        }
        if moving {
            let mut flags = gio::FileCopyFlags::NOFOLLOW_SYMLINKS
                | gio::FileCopyFlags::NO_FALLBACK_FOR_MOVE
                | gio::FileCopyFlags::ALL_METADATA;
            if replace {
                flags |= gio::FileCopyFlags::OVERWRITE;
            }
            match src.move_(&target, flags, Some(self.cancel), None) {
                Ok(()) => return Ok(Step::Done(target)),
                Err(e)
                    if e.matches(gio::IOErrorEnum::WouldRecurse)
                        || e.matches(gio::IOErrorEnum::NotSupported)
                        || e.matches(gio::IOErrorEnum::WouldMerge) => {}
                Err(e) => return Err(e),
            }
            // Another filesystem: copy, then remove the original.
            self.copy_tree(src, &target, replace)?;
            self.delete_tree(src)?;
            return Ok(Step::Done(target));
        }
        self.copy_tree(src, &target, replace)?;
        Ok(Step::Done(target))
    }

    /// Compress with the first of `zip`, `7z` or `bsdtar` found (Elavo
    /// ships `zip`), into one archive beside the items.
    fn compress(&mut self, sources: &[gio::File]) {
        let Some(first) = sources.first() else {
            return;
        };
        let (Some(parent), Some(cwd)) = (first.parent(), first.parent().and_then(|p| p.path()))
        else {
            self.out
                .errors
                .push("Only items on this computer can be compressed.".to_owned());
            return;
        };
        if sources
            .iter()
            .any(|s| s.parent().is_none_or(|p| !p.equal(&parent)))
        {
            self.out
                .errors
                .push("Items to compress have to be in the same folder.".to_owned());
            return;
        }
        let names: Vec<String> = sources.iter().map(display_name).collect();
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let archive = naming::archive(&refs, |n| exists(&parent.child(n)));
        let mut command = if glib::find_program_in_path("zip").is_some() {
            let mut c = std::process::Command::new("zip");
            c.args(["-r", "-q", "-y", &archive, "--"]).args(&names);
            c
        } else if glib::find_program_in_path("7z").is_some() {
            let mut c = std::process::Command::new("7z");
            c.args(["a", "-tzip", "-snl", "-bd", "-bso0", &archive, "--"])
                .args(&names);
            c
        } else if glib::find_program_in_path("bsdtar").is_some() {
            let mut c = std::process::Command::new("bsdtar");
            c.args(["-a", "-cf", &archive, "--"]).args(&names);
            c
        } else {
            self.out
                .errors
                .push("Compress needs zip, 7z or bsdtar, and none is installed.".to_owned());
            return;
        };
        self.p.current = archive.clone();
        self.report(true);
        let child = command
            .current_dir(&cwd)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .spawn();
        let mut child = match child {
            Ok(c) => c,
            Err(e) => {
                self.out.errors.push(e.to_string());
                return;
            }
        };
        let target = parent.child(&archive);
        let status = loop {
            if self.cancel.is_cancelled() {
                let _ = child.kill();
                let _ = child.wait();
                let _ = std::fs::remove_file(cwd.join(&archive));
                return;
            }
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) => std::thread::sleep(Duration::from_millis(60)),
                Err(e) => {
                    self.out.errors.push(e.to_string());
                    return;
                }
            }
        };
        if status.success() {
            self.out.done.push((first.clone(), target));
        } else {
            let mut text = String::new();
            if let Some(mut err) = child.stderr.take() {
                use std::io::Read;
                let _ = err.read_to_string(&mut text);
            }
            let _ = std::fs::remove_file(cwd.join(&archive));
            self.out.errors.push(format!(
                "\u{201c}{archive}\u{201d}: {}",
                text.trim().lines().last().unwrap_or("compressing failed")
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::path::Path;

    use super::*;

    #[derive(Default)]
    struct Script {
        answers: VecDeque<Answer>,
        asked: Vec<String>,
        reports: usize,
    }

    impl Ui for Script {
        fn progress(&mut self, _: &Progress) {
            self.reports += 1;
        }
        fn conflict(&mut self, c: &Conflict) -> Answer {
            self.asked.push(c.name.clone());
            self.answers.pop_front().expect("an answer")
        }
    }

    fn answer(resolution: Resolution, all: bool) -> Answer {
        Answer { resolution, all }
    }

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(tag: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("elavo-files-ops-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
        fn write(&self, rel: &str, body: &str) -> gio::File {
            let p = self.0.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, body).unwrap();
            gio::File::for_path(p)
        }
        fn file(&self, rel: &str) -> gio::File {
            gio::File::for_path(self.0.join(rel))
        }
        fn read(&self, rel: &str) -> String {
            std::fs::read_to_string(self.0.join(rel)).unwrap()
        }
        fn has(&self, rel: &str) -> bool {
            self.0.join(rel).symlink_metadata().is_ok()
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn go(kind: Kind, sources: Vec<gio::File>, dest: Dest, ui: &mut Script) -> Outcome {
        run(&Job::new(kind, sources, dest), &gio::Cancellable::new(), ui)
    }

    #[test]
    fn copies_and_duplicates() {
        let s = Scratch::new("copy");
        let a = s.write("a.txt", "A");
        s.write("tree/inner/b.txt", "B");
        std::fs::create_dir_all(s.0.join("out")).unwrap();
        let mut ui = Script::default();
        let out = go(
            Kind::Copy,
            vec![a.clone(), s.file("tree")],
            Dest::Into(s.file("out")),
            &mut ui,
        );
        assert!(out.errors.is_empty(), "{:?}", out.errors);
        assert_eq!(s.read("out/a.txt"), "A");
        assert_eq!(s.read("out/tree/inner/b.txt"), "B");
        assert!(ui.reports > 0);
        // Pasting into the same folder, or Duplicate, makes copies.
        go(Kind::Copy, vec![a.clone()], Dest::Into(s.file("")), &mut ui);
        go(Kind::Copy, vec![a.clone()], Dest::Beside, &mut ui);
        assert!(s.has("a copy.txt") && s.has("a copy 2.txt"));
        assert!(ui.asked.is_empty());
    }

    #[test]
    fn asks_on_conflicts() {
        let s = Scratch::new("conflict");
        let src: Vec<gio::File> = ["x.txt", "y.txt", "z.txt"]
            .iter()
            .map(|n| s.write(&format!("in/{n}"), "new"))
            .collect();
        for n in ["x.txt", "y.txt", "z.txt"] {
            s.write(&format!("out/{n}"), "old");
        }
        let mut ui = Script::default();
        ui.answers.push_back(answer(Resolution::KeepBoth, false));
        ui.answers.push_back(answer(Resolution::Replace, true));
        let out = go(Kind::Copy, src.clone(), Dest::Into(s.file("out")), &mut ui);
        assert_eq!(ui.asked, vec!["x.txt", "y.txt"], "Apply to All covers z");
        assert_eq!(s.read("out/x.txt"), "old");
        assert_eq!(s.read("out/x 2.txt"), "new");
        assert_eq!(s.read("out/y.txt"), "new");
        assert_eq!(s.read("out/z.txt"), "new");
        assert_eq!(out.done.len(), 3);

        // Skip leaves it; Stop ends the job and says so.
        let mut ui = Script::default();
        ui.answers.push_back(answer(Resolution::Skip, false));
        ui.answers.push_back(answer(Resolution::Stop, false));
        let out = go(Kind::Move, src.clone(), Dest::Into(s.file("out")), &mut ui);
        assert!(out.cancelled);
        assert!(out.done.is_empty());
        assert!(s.has("in/x.txt") && s.has("in/z.txt"));
    }

    #[test]
    fn moves_and_refuses_moving_into_itself() {
        let s = Scratch::new("move");
        s.write("box/inner/f.txt", "F");
        std::fs::create_dir_all(s.0.join("dest")).unwrap();
        let mut ui = Script::default();
        let out = go(
            Kind::Move,
            vec![s.file("box")],
            Dest::Into(s.file("dest")),
            &mut ui,
        );
        assert!(out.errors.is_empty());
        assert!(!s.has("box") && s.read("dest/box/inner/f.txt") == "F");
        let out = go(
            Kind::Move,
            vec![s.file("dest")],
            Dest::Into(s.file("dest/box")),
            &mut ui,
        );
        assert_eq!(out.errors.len(), 1);
        assert!(s.has("dest/box/inner/f.txt"));
        // Moving onto itself does nothing.
        let out = go(
            Kind::Move,
            vec![s.file("dest")],
            Dest::Into(s.file("")),
            &mut ui,
        );
        assert!(out.done.is_empty() && out.errors.is_empty());
        // Undo-style moves back to exact paths, with a rename.
        let out = go(
            Kind::Move,
            vec![s.file("dest/box")],
            Dest::Exactly(vec![s.file("renamed")]),
            &mut ui,
        );
        assert!(out.errors.is_empty());
        assert!(s.has("renamed/inner/f.txt"));
    }

    #[test]
    fn deletes_trees_and_puts_back_from_a_home_trash() {
        let s = Scratch::new("delete");
        s.write("gone/a/b.txt", "x");
        let mut ui = Script::default();
        let out = go(Kind::Delete, vec![s.file("gone")], Dest::Beside, &mut ui);
        assert!(out.errors.is_empty() && !s.has("gone"));

        // A home trash laid out by hand, as g_file_trash would.
        let trash = s.0.join("Trash");
        s.write("Trash/files/note.txt", "N");
        let orig = s.0.join("docs/note.txt");
        s.write(
            "Trash/info/note.txt.trashinfo",
            &format!(
                "[Trash Info]\nPath={}\nDeletionDate=2026-11-10T12:00:00\n",
                orig.display()
            ),
        );
        let item = gio::File::for_path(trash.join("files/note.txt"));
        let to = trash::orig_path(&item, &trash).unwrap();
        let mut job = Job::new(
            Kind::Restore,
            vec![item],
            Dest::Exactly(vec![gio::File::for_path(&to)]),
        );
        job.trash = trash.clone();
        let out = run(&job, &gio::Cancellable::new(), &mut ui);
        assert!(out.errors.is_empty(), "{:?}", out.errors);
        assert_eq!(s.read("docs/note.txt"), "N");
        assert!(!trash.join("info/note.txt.trashinfo").exists());
        assert!(trash::local_items(Path::new(&trash)).is_empty());
    }

    #[test]
    fn compresses_with_zip() {
        if glib::find_program_in_path("zip").is_none() {
            return;
        }
        let s = Scratch::new("zip");
        let a = s.write("Report.pdf", "pdf");
        let b = s.write("notes.txt", "txt");
        let mut ui = Script::default();
        let out = go(Kind::Compress, vec![a.clone()], Dest::Beside, &mut ui);
        assert!(out.errors.is_empty(), "{:?}", out.errors);
        assert!(s.has("Report.pdf.zip"));
        go(Kind::Compress, vec![a, b], Dest::Beside, &mut ui);
        assert!(s.has("Archive.zip"));
    }

    #[test]
    fn apply_to_all_is_remembered_except_stop() {
        let mut p = Policy::default();
        assert_eq!(
            p.resolve(|| answer(Resolution::Stop, true)),
            Resolution::Stop
        );
        assert_eq!(
            p.resolve(|| answer(Resolution::Skip, false)),
            Resolution::Skip
        );
        assert_eq!(
            p.resolve(|| answer(Resolution::Replace, true)),
            Resolution::Replace
        );
        assert_eq!(p.resolve(|| unreachable!()), Resolution::Replace);
    }
}
