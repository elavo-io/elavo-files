//! Running file operations off the main thread, and showing them: a small
//! progress ring in the toolbar that appears when an operation takes more
//! than a moment, with a popover listing each one and a button to stop it.
//! Name conflicts come back here from the worker and are asked in a
//! dialog while the worker waits.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;

use elavo_files_core::format;
use elavo_files_core::ops::{self, Answer, Conflict, Job, Outcome, Progress, Resolution};
use futures_util::StreamExt;
use gtk::prelude::*;
use gtk::{gio, glib};

use crate::dialogs;
use crate::i18n::tr;

/// How long an operation runs before the ring shows.
const SHOW_AFTER: Duration = Duration::from_millis(400);

enum Event {
    Progress(Progress),
    Conflict(Conflict, mpsc::Sender<Answer>),
    Done(Outcome),
}

struct ChannelUi {
    tx: futures_channel::mpsc::UnboundedSender<Event>,
}

impl ops::Ui for ChannelUi {
    fn progress(&mut self, progress: &Progress) {
        let _ = self.tx.unbounded_send(Event::Progress(progress.clone()));
    }

    fn conflict(&mut self, conflict: &Conflict) -> Answer {
        let (tx, rx) = mpsc::channel();
        if self
            .tx
            .unbounded_send(Event::Conflict(conflict.clone(), tx))
            .is_err()
        {
            return stop();
        }
        rx.recv().unwrap_or_else(|_| stop())
    }
}

fn stop() -> Answer {
    Answer {
        resolution: Resolution::Stop,
        all: false,
    }
}

struct Entry {
    row: gtk::Box,
    bar: gtk::ProgressBar,
    detail: gtk::Label,
    fraction: Cell<Option<f64>>,
}

pub struct Jobs {
    /// The toolbar button, hidden while nothing runs.
    pub button: gtk::MenuButton,
    ring: gtk::DrawingArea,
    list: gtk::Box,
    entries: RefCell<Vec<Rc<Entry>>>,
    window: glib::WeakRef<gtk::ApplicationWindow>,
    /// Where focus goes when the button hides with focus in it.
    refocus: RefCell<Option<Box<dyn Fn()>>>,
}

fn detail_text(p: &Progress) -> String {
    let amount = if p.total_bytes > 0 {
        format!(
            "{} {} {}",
            format::size(p.done_bytes),
            tr("of"),
            format::size(p.total_bytes)
        )
    } else if p.total_items > 1 {
        format!(
            "{} {} {}",
            p.done_items.min(p.total_items),
            tr("of"),
            p.total_items
        )
    } else {
        String::new()
    };
    match (amount.is_empty(), p.current.is_empty()) {
        (false, false) => format!("{amount} \u{2014} {}", p.current),
        (false, true) => amount,
        _ => p.current.clone(),
    }
}

impl Jobs {
    pub fn new(window: &gtk::ApplicationWindow) -> Rc<Self> {
        let ring = gtk::DrawingArea::builder()
            .content_width(18)
            .content_height(18)
            .build();
        let list = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .width_request(300)
            .css_classes(["files-jobs"])
            .build();
        let button = gtk::MenuButton::builder()
            .child(&ring)
            .tooltip_text(tr("Operations in progress"))
            .valign(gtk::Align::Center)
            .popover(&gtk::Popover::builder().child(&list).build())
            .css_classes(["files-round"])
            .visible(false)
            .build();
        let this = Rc::new(Self {
            button,
            ring,
            list,
            entries: RefCell::default(),
            refocus: RefCell::default(),
            window: window.downgrade(),
        });
        let weak = Rc::downgrade(&this);
        this.ring.set_draw_func(move |area, cr, w, h| {
            let Some(jobs) = weak.upgrade() else {
                return;
            };
            let known: Vec<f64> = jobs
                .entries
                .borrow()
                .iter()
                .filter_map(|e| e.fraction.get())
                .collect();
            let fraction = if known.is_empty() {
                0.25
            } else {
                known.iter().sum::<f64>() / known.len() as f64
            };
            let color = area.color();
            let (cx, cy) = (w as f64 / 2.0, h as f64 / 2.0);
            let r = cx.min(cy) - 2.0;
            cr.set_line_width(2.5);
            cr.set_source_rgba(
                color.red().into(),
                color.green().into(),
                color.blue().into(),
                0.25,
            );
            cr.arc(cx, cy, r, 0.0, std::f64::consts::TAU);
            let _ = cr.stroke();
            cr.set_source_rgba(
                color.red().into(),
                color.green().into(),
                color.blue().into(),
                1.0,
            );
            let start = -std::f64::consts::FRAC_PI_2;
            cr.arc(
                cx,
                cy,
                r,
                start,
                start + fraction.max(0.02) * std::f64::consts::TAU,
            );
            let _ = cr.stroke();
        });
        this
    }

    pub fn running(&self) -> bool {
        !self.entries.borrow().is_empty()
    }

    /// Run `job` on a worker thread; `done` gets its outcome on the main
    /// thread.
    pub fn start(self: &Rc<Self>, job: Job, title: String, done: impl FnOnce(Outcome) + 'static) {
        let cancel = gio::Cancellable::new();
        let entry = self.add_entry(&title, &cancel);
        let (tx, mut rx) = futures_channel::mpsc::unbounded();
        let worker_cancel = cancel.clone();
        std::thread::spawn(move || {
            let mut ui = ChannelUi { tx: tx.clone() };
            let outcome = ops::run(&job, &worker_cancel, &mut ui);
            let _ = tx.unbounded_send(Event::Done(outcome));
        });

        // Quick operations never show the ring.
        let weak = Rc::downgrade(self);
        let shown = Rc::downgrade(&entry);
        glib::timeout_add_local_once(SHOW_AFTER, move || {
            if let (Some(jobs), Some(_)) = (weak.upgrade(), shown.upgrade()) {
                if jobs.running() {
                    jobs.button.set_visible(true);
                }
            }
        });

        // Keep the app running until the operation ends, even with its
        // window closed.
        let hold = gio::Application::default().map(|a| a.hold());
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let _hold = hold;
            let mut done = Some(done);
            while let Some(event) = rx.next().await {
                let Some(jobs) = weak.upgrade() else {
                    return;
                };
                match event {
                    Event::Progress(p) => {
                        entry.fraction.set(p.fraction());
                        match p.fraction() {
                            Some(f) => entry.bar.set_fraction(f),
                            None => entry.bar.pulse(),
                        }
                        entry.detail.set_label(&detail_text(&p));
                        jobs.ring.queue_draw();
                    }
                    Event::Conflict(conflict, reply) => {
                        let parent = jobs.window.upgrade();
                        let answer = dialogs::conflict(parent.as_ref(), &conflict).await;
                        let _ = reply.send(answer);
                    }
                    Event::Done(outcome) => {
                        jobs.remove_entry(&entry);
                        if let Some(done) = done.take() {
                            done(outcome);
                        }
                        return;
                    }
                }
            }
        });
    }

    fn add_entry(&self, title: &str, cancel: &gio::Cancellable) -> Rc<Entry> {
        let row = gtk::Box::builder().spacing(8).build();
        let text = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(4)
            .hexpand(true)
            .build();
        text.append(
            &gtk::Label::builder()
                .label(title)
                .xalign(0.0)
                .ellipsize(gtk::pango::EllipsizeMode::Middle)
                .css_classes(["files-job-title"])
                .build(),
        );
        let bar = gtk::ProgressBar::builder().pulse_step(0.1).build();
        text.append(&bar);
        let detail = gtk::Label::builder()
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::Middle)
            .css_classes(["files-dim", "caption"])
            .build();
        text.append(&detail);
        row.append(&text);
        let stop = gtk::Button::builder()
            .icon_name("process-stop-symbolic")
            .tooltip_text(tr("Stop"))
            .valign(gtk::Align::Center)
            .css_classes(["flat", "circular"])
            .build();
        let c = cancel.clone();
        stop.connect_clicked(move |_| c.cancel());
        row.append(&stop);
        self.list.append(&row);
        let entry = Rc::new(Entry {
            row,
            bar,
            detail,
            fraction: Cell::new(Some(0.0)),
        });
        self.entries.borrow_mut().push(entry.clone());
        entry
    }

    pub fn set_refocus(&self, f: impl Fn() + 'static) {
        self.refocus.replace(Some(Box::new(f)));
    }

    fn remove_entry(&self, entry: &Rc<Entry>) {
        self.list.remove(&entry.row);
        self.entries.borrow_mut().retain(|e| !Rc::ptr_eq(e, entry));
        if !self.running() {
            let had_focus = self.button.popover().is_some_and(|p| p.is_visible())
                || self.button.has_focus()
                || self
                    .window
                    .upgrade()
                    .and_then(|w| gtk::prelude::GtkWindowExt::focus(&w))
                    .is_some_and(|f| f.is_ancestor(&self.button));
            self.button.popdown();
            self.button.set_visible(false);
            if had_focus {
                if let Some(f) = self.refocus.borrow().as_ref() {
                    f();
                }
            }
        }
        self.ring.queue_draw();
    }
}
