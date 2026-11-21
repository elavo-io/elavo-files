//! Incoming requests: one model, shown in several places at once.
//!
//! A request lives here from the moment a device asks until the person
//! clears it. Every [`Presenter`] shows the same [`Request`] and is told
//! when it changes, so answering in one place updates the others:
//!
//! - the LocalSend page in Files lists every request ([`Pages`]), so one
//!   that was missed or dismissed can still be answered there;
//! - an alert, the [`Alert`] picked, gets the person's attention. Elavo has
//!   no notification server yet, so that is a small dialog ([`Sheets`]).
//!   A freedesktop notification with Accept and Decline actions would be
//!   one more `Presenter` and an `Alert` variant, chosen when
//!   `org.freedesktop.Notifications` is on the bus.
//!
//! The dialog never takes a keypress meant for another app: labwc focuses
//! every new window, so it ignores the keyboard until the person presses
//! Tab, has no default button, and its buttons only work after a moment.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::{Rc, Weak};
use std::time::{Duration, Instant};

use elavo_files_core::format;
use elavo_files_core::localsend::{Decision, Incoming, SendState, Transfer};
use gtk::prelude::*;
use gtk::{gdk, gio, glib};

use super::{
    avatar, body_box, count_text, dialog_window, items_list, kind_label, label, top_items,
};
use crate::i18n::tr;

/// The dialog's title, constant so a compositor rule can match it.
pub const SHEET_TITLE: &str = "LocalSend Request";

/// How long the dialog's buttons ignore clicks after it appears.
const ARM_AFTER: Duration = Duration::from_millis(600);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum State {
    /// Waiting for Accept or Decline, until the request's deadline.
    Waiting,
    Receiving {
        done: u64,
        total: u64,
    },
    Saved,
    Declined,
    /// Nobody answered in time; the sender was told no.
    Expired,
    /// The sender withdrew it, or the transfer was stopped.
    Cancelled,
    Failed(String),
    /// A text message: nothing to accept, only to read.
    Message,
}

impl State {
    /// Over, so only clearing is left.
    pub fn is_over(&self) -> bool {
        !matches!(self, State::Waiting | State::Receiving { .. })
    }

    /// The same kind of state, ignoring progress, to know when a view
    /// needs other buttons.
    fn same_kind(&self, other: &State) -> bool {
        std::mem::discriminant(self) == std::mem::discriminant(other)
    }
}

pub struct Request {
    pub incoming: Incoming,
    pub folder: PathBuf,
    deadline: Option<Instant>,
    state: RefCell<State>,
}

impl Request {
    pub fn new(incoming: Incoming, folder: PathBuf) -> Self {
        let message = incoming.files.len() == 1 && incoming.files[0].is_message();
        let state = if message {
            State::Message
        } else if incoming.accepted {
            State::Receiving {
                done: 0,
                total: incoming.bytes,
            }
        } else {
            State::Waiting
        };
        Self {
            deadline: incoming.expires_in.map(|d| Instant::now() + d),
            incoming,
            folder,
            state: RefCell::new(state),
        }
    }

    pub fn id(&self) -> &str {
        &self.incoming.id
    }

    pub fn state(&self) -> State {
        self.state.borrow().clone()
    }

    /// Time left to answer, while waiting.
    pub fn remaining(&self) -> Option<Duration> {
        if *self.state.borrow() != State::Waiting {
            return None;
        }
        self.deadline
            .map(|d| d.saturating_duration_since(Instant::now()))
    }

    fn title(&self) -> String {
        let what = if self.state() == State::Message {
            tr("sent you a message")
        } else {
            tr("would like to share")
        };
        format!("\u{201c}{}\u{201d} {what}", self.incoming.sender.alias)
    }

    fn from(&self) -> String {
        format!(
            "{} \u{00b7} {}",
            kind_label(&self.incoming.sender),
            self.incoming.sender.ip
        )
    }

    fn place(&self) -> String {
        self.folder
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    /// One line on where it is: the time left, progress, or how it ended.
    pub fn status(&self) -> String {
        let sender = &self.incoming.sender.alias;
        match self.state() {
            State::Waiting => match self.remaining() {
                Some(left) => format!(
                    "{} \u{00b7} {} {}",
                    tr("Waiting for an answer"),
                    clock(left),
                    tr("left")
                ),
                None => tr("Waiting"),
            },
            State::Receiving { done, total } if done > 0 => format!(
                "{} {} {}",
                format::size(done),
                tr("of"),
                format::size(total)
            ),
            State::Receiving { .. } => tr("Waiting for the files\u{2026}"),
            State::Saved => format!("{} \u{201c}{}\u{201d}.", tr("Saved to"), self.place()),
            State::Declined => tr("Declined."),
            State::Expired => format!(
                "{} \u{201c}{sender}\u{201d} {}",
                tr("No answer in time, so"),
                tr("was told no.")
            ),
            State::Cancelled => format!("\u{201c}{sender}\u{201d} {}", tr("stopped sharing.")),
            State::Failed(e) => e,
            State::Message => String::new(),
        }
    }

    fn fraction(&self) -> Option<f64> {
        match self.state() {
            State::Receiving { done, total } => Some(if total == 0 {
                0.0
            } else {
                (done as f64 / total as f64).min(0.999)
            }),
            State::Saved => Some(1.0),
            _ => None,
        }
    }

    fn message(&self) -> Option<String> {
        (self.state() == State::Message)
            .then(|| self.incoming.files[0].preview.clone().unwrap_or_default())
    }
}

/// `4:05`.
pub fn clock(d: Duration) -> String {
    let s = d.as_secs() + u64::from(d.subsec_millis() > 0);
    format!("{}:{:02}", s / 60, s % 60)
}

/// Something that shows requests: the dialog, the LocalSend page, or one
/// day a notification.
pub trait Presenter {
    /// A new request.
    fn present(&self, request: &Rc<Request>);
    /// Its state changed: answered anywhere, progress, expired, or a
    /// second passed while it waits.
    fn update(&self, request: &Rc<Request>);
    /// Cleared: stop showing it.
    fn withdraw(&self, id: &str);
}

/// How a new request asks for attention, besides the LocalSend page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Alert {
    /// A small dialog, as AirDrop's sheet.
    Dialog,
    /// Only the LocalSend page (`ELAVO_LOCALSEND_ALERT=none`).
    None,
    // Notification: a freedesktop notification, once Elavo has a server.
}

impl Alert {
    pub fn pick() -> Self {
        match std::env::var("ELAVO_LOCALSEND_ALERT").as_deref() {
            Ok("none") => Alert::None,
            _ => Alert::Dialog,
        }
    }
}

struct Registry {
    requests: RefCell<Vec<Rc<Request>>>,
    presenters: Vec<Rc<dyn Presenter>>,
    pages: Rc<Pages>,
    ticking: Cell<bool>,
}

thread_local! {
    static REGISTRY: RefCell<Option<Rc<Registry>>> = const { RefCell::new(None) };
}

fn registry() -> Option<Rc<Registry>> {
    REGISTRY.with(|r| r.borrow().clone())
}

/// Set up the presenters, once, when LocalSend starts.
pub fn init(app: &gtk::Application) {
    let pages = Rc::new(Pages::default());
    let mut presenters: Vec<Rc<dyn Presenter>> = vec![pages.clone()];
    match Alert::pick() {
        Alert::Dialog => presenters.push(Rc::new(Sheets::new(app))),
        Alert::None => {}
    }
    REGISTRY.with(|r| {
        r.replace(Some(Rc::new(Registry {
            requests: RefCell::default(),
            presenters,
            pages,
            ticking: Cell::new(false),
        })))
    });
}

pub fn requests() -> Vec<Rc<Request>> {
    registry()
        .map(|r| r.requests.borrow().clone())
        .unwrap_or_default()
}

fn find(id: &str) -> Option<Rc<Request>> {
    requests().into_iter().find(|r| r.id() == id)
}

fn each(f: impl Fn(&dyn Presenter)) {
    if let Some(r) = registry() {
        for p in &r.presenters {
            f(p.as_ref());
        }
    }
}

fn set(request: &Rc<Request>, state: State) {
    request.state.replace(state);
    each(|p| p.update(request));
}

/// A device asks (or, with approval off, has started sending).
pub fn incoming(incoming: Incoming, folder: PathBuf) {
    let Some(reg) = registry() else {
        return;
    };
    let request = Rc::new(Request::new(incoming, folder));
    reg.requests.borrow_mut().push(request.clone());
    each(|p| p.present(&request));
    tick();
}

/// Once a second while anything waits: the time left.
fn tick() {
    let Some(reg) = registry() else {
        return;
    };
    if reg.ticking.replace(true) {
        return;
    }
    glib::timeout_add_seconds_local(1, move || {
        let waiting: Vec<_> = requests()
            .into_iter()
            .filter(|r| r.state() == State::Waiting)
            .collect();
        for r in &waiting {
            each(|p| p.update(r));
        }
        if waiting.is_empty() {
            if let Some(reg) = registry() {
                reg.ticking.set(false);
            }
            glib::ControlFlow::Break
        } else {
            glib::ControlFlow::Continue
        }
    });
}

/// Accept or Decline, from wherever it was shown.
pub fn answer(id: &str, decision: Decision) {
    let Some(request) = find(id) else {
        return;
    };
    if request.state() != State::Waiting {
        return;
    }
    super::decide(id, decision);
    match decision {
        Decision::Accept => set(
            &request,
            State::Receiving {
                done: 0,
                total: request.incoming.bytes,
            },
        ),
        Decision::Decline => {
            set(&request, State::Declined);
            clear(id);
        }
    }
}

/// Accept this request, add the sender to Favorites, and turn on
/// Automatically Accept from Favorites, so later files from this device
/// are saved without asking. Works from the request dialog while Files
/// is only running as the LocalSend daemon.
pub fn always_accept(id: &str) {
    let Some(request) = find(id) else {
        return;
    };
    if request.state() != State::Waiting {
        return;
    }
    super::always_accept_from(
        &request.incoming.sender.fingerprint,
        &request.incoming.sender.alias,
    );
    answer(id, Decision::Accept);
}

/// Stop a transfer under way.
pub fn stop(id: &str) {
    let Some(request) = find(id) else {
        return;
    };
    if let State::Receiving { .. } = request.state() {
        if let Some(s) = super::service() {
            s.cancel(id);
        }
        set(&request, State::Cancelled);
    }
}

/// Forget a request everywhere.
pub fn clear(id: &str) {
    if let Some(reg) = registry() {
        reg.requests.borrow_mut().retain(|r| r.id() != id);
    }
    each(|p| p.withdraw(id));
}

/// The service no longer waits for an answer: expired, or the sender
/// cancelled.
pub fn dismissed(id: &str) {
    let Some(request) = find(id) else {
        return;
    };
    if request.state() == State::Waiting {
        let expired = request
            .deadline
            .is_some_and(|d| Instant::now() + Duration::from_secs(2) >= d);
        set(
            &request,
            if expired {
                State::Expired
            } else {
                State::Cancelled
            },
        );
    }
}

/// Progress and the end of a transfer.
pub fn transfer(t: &Transfer) {
    let Some(request) = find(&t.id) else {
        return;
    };
    if request.state().is_over() {
        return;
    }
    let state = match &t.state {
        SendState::Sending | SendState::Preparing | SendState::Waiting => State::Receiving {
            done: t.done,
            total: t.total,
        },
        SendState::Done => State::Saved,
        SendState::Cancelled => State::Cancelled,
        SendState::Declined => State::Declined,
        SendState::Error(e) => State::Failed(e.clone()),
        SendState::NeedsPin | SendState::Busy => return,
    };
    set(&request, state);
}

fn show_folder(app: Option<gtk::Application>, folder: &std::path::Path) {
    if let Some(app) = app.or_else(|| gio::Application::default().and_downcast()) {
        crate::app::open_window(&app, Some(gio::File::for_path(folder)));
    }
}

fn button(text: &str, class: Option<&str>, f: impl Fn() + 'static) -> gtk::Button {
    let b = gtk::Button::with_label(text);
    if let Some(c) = class {
        b.add_css_class(c);
    }
    b.connect_clicked(move |_| f());
    b
}

fn names(request: &Request) -> String {
    let items = top_items(&request.incoming.files);
    let mut s = items
        .iter()
        .take(3)
        .map(|(n, _, _)| n.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    if items.len() > 3 {
        s.push_str(&format!(
            ", {} {} {}",
            tr("and"),
            items.len() - 3,
            tr("more")
        ));
    }
    format!(
        "{s} \u{00b7} {}",
        count_text(items.len(), request.incoming.bytes)
    )
}

// The dialog.

/// The dialogs, one per request that hasn't been closed.
struct Sheets {
    app: glib::WeakRef<gtk::Application>,
    open: Rc<RefCell<Vec<Rc<Sheet>>>>,
}

impl Sheets {
    fn new(app: &gtk::Application) -> Self {
        Self {
            app: app.downgrade(),
            open: Rc::default(),
        }
    }

    fn get(&self, id: &str) -> Option<Rc<Sheet>> {
        self.open
            .borrow()
            .iter()
            .find(|s| s.request.id() == id)
            .cloned()
    }
}

impl Presenter for Sheets {
    fn present(&self, request: &Rc<Request>) {
        let Some(app) = self.app.upgrade() else {
            return;
        };
        let sheet = Sheet::new(&app, request.clone());
        self.open.borrow_mut().push(sheet.clone());
        // Closing it only hides it: the request stays on the LocalSend
        // page, still waiting.
        let id = request.id().to_owned();
        let all = Rc::downgrade(&self.open);
        let open = Rc::downgrade(&sheet);
        sheet.window.connect_close_request(move |_| {
            let this = open.upgrade();
            if let Some(all) = all.upgrade() {
                all.borrow_mut().retain(|s| s.request.id() != id);
            }
            if let Some(s) = this {
                super::log(&format!(
                    "localsend: request dialog closed ({:?})",
                    s.request.state()
                ));
                if s.request.state().is_over() && s.request.state() != State::Saved {
                    // Ended and seen: nothing left to show for it.
                    clear(s.request.id());
                }
            }
            glib::Propagation::Proceed
        });
        sheet.sync();
        sheet.window.present();
    }

    fn update(&self, request: &Rc<Request>) {
        if let Some(s) = self.get(request.id()) {
            if request.state() == State::Declined {
                s.window.close();
            } else {
                s.sync();
            }
        }
    }

    fn withdraw(&self, id: &str) {
        if let Some(s) = self.get(id) {
            s.window.close();
        }
    }
}

struct Sheet {
    request: Rc<Request>,
    window: gtk::Window,
    status: gtk::Label,
    progress: gtk::ProgressBar,
    always: gtk::Button,
    buttons: gtk::Box,
    shown: RefCell<Option<State>>,
    _hold: gio::ApplicationHoldGuard,
}

impl Sheet {
    fn new(app: &gtk::Application, request: Rc<Request>) -> Rc<Self> {
        let window = dialog_window(Some(app), &tr(SHEET_TITLE));
        // Not tied to any window and not modal: it waits beside whatever
        // the person is doing.
        window.set_modal(false);
        window.set_focus_visible(false);
        let body = body_box();
        let top = gtk::Box::builder().spacing(14).build();
        top.append(&avatar(request.incoming.sender.kind, 64));
        let text = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(4)
            .hexpand(true)
            .valign(gtk::Align::Center)
            .build();
        text.append(&label(&request.title(), &["files-dialog-title"]));
        text.append(&label(&request.from(), &["files-dim"]));
        top.append(&text);
        body.append(&top);
        if let Some(m) = request.message() {
            body.append(
                &gtk::Label::builder()
                    .label(&m)
                    .selectable(true)
                    .wrap(true)
                    .xalign(0.0)
                    .max_width_chars(44)
                    .css_classes(["files-ls-message"])
                    .build(),
            );
        } else {
            body.append(&items_list(&request.incoming.files));
            body.append(&label(
                &count_text(
                    top_items(&request.incoming.files).len(),
                    request.incoming.bytes,
                ),
                &["files-dim"],
            ));
        }
        let status = label("", &["files-dim", "files-ls-status"]);
        let progress = gtk::ProgressBar::builder().visible(false).build();
        let always = gtk::Button::with_label(&tr("Always accept from this device"));
        always.add_css_class("flat");
        always.set_halign(gtk::Align::Start);
        always.set_hexpand(true);
        always.set_visible(false);
        always.set_tooltip_text(Some(&tr(
            "Add this device to Favorites and automatically accept files from favorites.",
        )));
        let always_id = request.id().to_owned();
        always.connect_clicked(move |_| always_accept(&always_id));
        let buttons = gtk::Box::builder()
            .spacing(8)
            .halign(gtk::Align::End)
            .build();
        let actions = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(8)
            .sensitive(false)
            .build();
        actions.append(&always);
        actions.append(&buttons);
        body.append(&status);
        body.append(&progress);
        body.append(&actions);
        window.set_child(Some(&body));
        guard_keys(&window);
        // A click as it appears is not an answer either.
        let a = actions.downgrade();
        glib::timeout_add_local_once(ARM_AFTER, move || {
            if let Some(a) = a.upgrade() {
                a.set_sensitive(true);
            }
        });
        Rc::new(Self {
            request,
            window,
            status,
            progress,
            always,
            buttons,
            shown: RefCell::default(),
            _hold: app.hold(),
        })
    }

    fn clear_buttons(&self) {
        while let Some(c) = self.buttons.first_child() {
            self.buttons.remove(&c);
        }
    }

    fn sync(&self) {
        let state = self.request.state();
        let status = self.request.status();
        self.status.set_label(&status);
        self.status.set_visible(!status.is_empty());
        match self.request.fraction() {
            Some(f) => {
                self.progress.set_visible(true);
                self.progress.set_fraction(f);
            }
            None => self.progress.set_visible(false),
        }
        if self
            .shown
            .borrow()
            .as_ref()
            .is_some_and(|s| s.same_kind(&state))
        {
            return;
        }
        self.shown.replace(Some(state.clone()));
        self.clear_buttons();
        self.always.set_visible(false);
        let id = self.request.id().to_owned();
        let w = self.window.downgrade();
        let close = move || {
            if let Some(w) = w.upgrade() {
                w.close();
            }
        };
        match state {
            State::Waiting => {
                self.always.set_visible(true);
                // Hides it; it still waits on the LocalSend page.
                let later = button(&tr("Later"), Some("flat"), close.clone());
                later.set_tooltip_text(Some(&tr("Answer later from LocalSend in Files")));
                self.buttons.append(&later);
                let a = id.clone();
                self.buttons.append(&button(&tr("Decline"), None, move || {
                    answer(&a, Decision::Decline)
                }));
                // Not the default button: Enter is not an answer.
                self.buttons.append(&button(
                    &tr("Accept"),
                    Some("suggested-action"),
                    move || answer(&id, Decision::Accept),
                ));
            }
            State::Receiving { .. } => {
                let hide = close.clone();
                self.buttons.append(&button(&tr("Hide"), None, hide));
                self.buttons
                    .append(&button(&tr("Stop"), None, move || stop(&id)));
            }
            State::Saved => {
                let (app, folder) = (self.window.application(), self.request.folder.clone());
                let c = close.clone();
                self.buttons
                    .append(&button(&tr("Show in Files"), None, move || {
                        show_folder(app.clone(), &folder);
                        c();
                    }));
                let (c, a) = (close.clone(), id.clone());
                self.buttons
                    .append(&button(&tr("Done"), Some("suggested-action"), move || {
                        clear(&a);
                        c();
                    }));
            }
            State::Message => {
                let text = self.request.message().unwrap_or_default();
                let w = self.window.clone();
                let c = close.clone();
                self.buttons.append(&button(&tr("Copy"), None, move || {
                    gdk::prelude::DisplayExt::clipboard(&WidgetExt::display(&w)).set_text(&text);
                    c();
                }));
                self.buttons
                    .append(&button(&tr("Close"), Some("suggested-action"), close));
            }
            State::Declined | State::Expired | State::Cancelled | State::Failed(_) => {
                self.buttons.append(&button(&tr("Close"), None, close));
            }
        }
    }
}

/// labwc gives every new window the keyboard, so the dialog may get keys
/// typed for another app. Until the person presses Tab to move to its
/// buttons, it ignores every key.
fn guard_keys(window: &gtk::Window) {
    let armed = Rc::new(Cell::new(false));
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    let a = armed.clone();
    let w = window.downgrade();
    keys.connect_key_pressed(move |_, key, _, _| {
        if a.get() {
            return glib::Propagation::Proceed;
        }
        if matches!(
            key,
            gdk::Key::Tab | gdk::Key::ISO_Left_Tab | gdk::Key::KP_Tab
        ) {
            a.set(true);
            if let Some(w) = w.upgrade() {
                w.set_focus_visible(true);
            }
            return glib::Propagation::Proceed;
        }
        super::log(&format!("localsend: request dialog ignored {key:?}"));
        glib::Propagation::Stop
    });
    window.add_controller(keys);
    // No widget starts focused, so nothing is under Enter or Space.
    let w = window.downgrade();
    window.connect_map(move |_| {
        if let Some(w) = w.upgrade() {
            gtk::prelude::GtkWindowExt::set_focus(&w, None::<&gtk::Widget>);
        }
    });
}

// The LocalSend page.

/// Every LocalSend page open, each with a card per request.
#[derive(Default)]
pub struct Pages {
    lists: RefCell<Vec<Weak<RequestList>>>,
}

impl Pages {
    fn each(&self, f: impl Fn(&RequestList)) {
        self.lists.borrow_mut().retain(|l| l.strong_count() > 0);
        let lists: Vec<_> = self
            .lists
            .borrow()
            .iter()
            .filter_map(Weak::upgrade)
            .collect();
        for l in lists {
            f(&l);
        }
    }
}

impl Presenter for Pages {
    fn present(&self, request: &Rc<Request>) {
        self.each(|l| l.add(request));
    }

    fn update(&self, request: &Rc<Request>) {
        self.each(|l| l.update(request));
    }

    fn withdraw(&self, id: &str) {
        self.each(|l| l.remove(id));
    }
}

/// The requests on one LocalSend page.
pub struct RequestList {
    pub root: gtk::Box,
    cards: gtk::Box,
    shown: RefCell<Vec<Card>>,
}

impl RequestList {
    /// A list for a new page, with the requests so far.
    pub fn new() -> Rc<Self> {
        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(8)
            .margin_top(24)
            .margin_start(24)
            .margin_end(24)
            .halign(gtk::Align::Center)
            .width_request(560)
            .vexpand(false)
            .visible(false)
            .css_classes(["files-ls-requests"])
            .build();
        root.append(&label(&tr("Requests"), &["files-ls-requests-title"]));
        let cards = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(8)
            .build();
        root.append(&cards);
        let this = Rc::new(Self {
            root,
            cards,
            shown: RefCell::default(),
        });
        for r in requests() {
            this.add(&r);
        }
        if let Some(reg) = registry() {
            reg.pages.lists.borrow_mut().push(Rc::downgrade(&this));
        }
        this
    }

    fn add(&self, request: &Rc<Request>) {
        if self
            .shown
            .borrow()
            .iter()
            .any(|c| c.request.id() == request.id())
        {
            return;
        }
        let card = Card::new(request.clone());
        self.cards.prepend(&card.root);
        self.shown.borrow_mut().push(card);
        self.root.set_visible(true);
    }

    fn update(&self, request: &Rc<Request>) {
        if let Some(c) = self
            .shown
            .borrow()
            .iter()
            .find(|c| c.request.id() == request.id())
        {
            c.sync();
        }
    }

    fn remove(&self, id: &str) {
        self.shown.borrow_mut().retain(|c| {
            if c.request.id() == id {
                self.cards.remove(&c.root);
                false
            } else {
                true
            }
        });
        self.root.set_visible(!self.shown.borrow().is_empty());
    }
}

/// A label that never wraps, so a card stays one row high.
fn one_line(l: gtk::Label) -> gtk::Label {
    l.set_wrap(false);
    l.set_ellipsize(gtk::pango::EllipsizeMode::End);
    l.set_max_width_chars(-1);
    l
}

struct Card {
    request: Rc<Request>,
    root: gtk::Box,
    badge: gtk::Label,
    status: gtk::Label,
    progress: gtk::ProgressBar,
    buttons: gtk::Box,
    shown: RefCell<Option<State>>,
}

impl Card {
    fn new(request: Rc<Request>) -> Self {
        let root = gtk::Box::builder()
            .spacing(14)
            // The picture expands inside its circle; the card doesn't.
            .vexpand(false)
            .css_classes(["files-ls-request"])
            .build();
        let picture = avatar(request.incoming.sender.kind, 48);
        picture.set_hexpand(false);
        picture.set_vexpand(false);
        root.append(&picture);
        let text = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(3)
            .hexpand(true)
            .valign(gtk::Align::Center)
            .build();
        let head = gtk::Box::builder().spacing(8).build();
        let title = one_line(label(&request.title(), &["files-ls-request-title"]));
        head.append(&title);
        let badge = gtk::Label::builder()
            .css_classes(["files-ls-badge"])
            .valign(gtk::Align::Center)
            .build();
        head.append(&badge);
        text.append(&head);
        text.append(&one_line(label(&request.from(), &["files-dim"])));
        let what = match request.message() {
            Some(m) => m,
            None => names(&request),
        };
        let what = gtk::Label::builder()
            .label(&what)
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .max_width_chars(48)
            .build();
        text.append(&what);
        let status = one_line(label("", &["files-dim", "files-ls-status"]));
        text.append(&status);
        let progress = gtk::ProgressBar::builder().visible(false).build();
        text.append(&progress);
        root.append(&text);
        let buttons = gtk::Box::builder()
            .spacing(8)
            .valign(gtk::Align::Center)
            .build();
        root.append(&buttons);
        let card = Self {
            request,
            root,
            badge,
            status,
            progress,
            buttons,
            shown: RefCell::default(),
        };
        card.sync();
        card
    }

    fn sync(&self) {
        let state = self.request.state();
        let status = self.request.status();
        self.status.set_label(&status);
        self.status.set_visible(!status.is_empty());
        match self.request.fraction() {
            Some(f) if state != State::Saved => {
                self.progress.set_visible(true);
                self.progress.set_fraction(f);
            }
            _ => self.progress.set_visible(false),
        }
        let badge = match &state {
            State::Waiting => tr("Waiting"),
            State::Receiving { .. } => tr("Receiving"),
            State::Saved => tr("Received"),
            State::Declined => tr("Declined"),
            State::Expired => tr("Expired"),
            State::Cancelled => tr("Cancelled"),
            State::Failed(_) => tr("Failed"),
            State::Message => tr("Message"),
        };
        self.badge.set_label(&badge);
        for c in ["waiting", "over"] {
            self.root.remove_css_class(c);
        }
        self.root
            .add_css_class(if state.is_over() { "over" } else { "waiting" });
        if self
            .shown
            .borrow()
            .as_ref()
            .is_some_and(|s| s.same_kind(&state))
        {
            return;
        }
        self.shown.replace(Some(state.clone()));
        while let Some(c) = self.buttons.first_child() {
            self.buttons.remove(&c);
        }
        let id = self.request.id().to_owned();
        let dismiss = {
            let id = id.clone();
            let b = gtk::Button::builder()
                .icon_name("window-close-symbolic")
                .tooltip_text(tr("Clear"))
                .css_classes(["flat", "circular"])
                .build();
            b.connect_clicked(move |_| clear(&id));
            b
        };
        match state {
            State::Waiting => {
                let a = id.clone();
                let always = button(&tr("Always accept from this device"), Some("flat"), move || {
                    always_accept(&a)
                });
                always.set_tooltip_text(Some(&tr(
                    "Add this device to Favorites and automatically accept files from favorites.",
                )));
                self.buttons.append(&always);
                let a = id.clone();
                self.buttons.append(&button(&tr("Decline"), None, move || {
                    answer(&a, Decision::Decline)
                }));
                self.buttons.append(&button(
                    &tr("Accept"),
                    Some("suggested-action"),
                    move || answer(&id, Decision::Accept),
                ));
            }
            State::Receiving { .. } => {
                self.buttons
                    .append(&button(&tr("Stop"), None, move || stop(&id)));
            }
            State::Saved => {
                let folder = self.request.folder.clone();
                let root = self.root.downgrade();
                self.buttons
                    .append(&button(&tr("Show in Files"), None, move || {
                        let app = root
                            .upgrade()
                            .and_then(|r| r.root())
                            .and_downcast::<gtk::Window>()
                            .and_then(|w| w.application());
                        show_folder(app, &folder);
                    }));
                self.buttons.append(&dismiss);
            }
            State::Message => {
                let text = self.request.message().unwrap_or_default();
                let root = self.root.downgrade();
                self.buttons.append(&button(&tr("Copy"), None, move || {
                    if let Some(r) = root.upgrade() {
                        gdk::prelude::DisplayExt::clipboard(&WidgetExt::display(&r))
                            .set_text(&text);
                    }
                }));
                self.buttons.append(&dismiss);
            }
            State::Declined | State::Expired | State::Cancelled | State::Failed(_) => {
                self.buttons.append(&dismiss);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use elavo_files_core::localsend::{Device, DeviceType, FileMeta};

    fn incoming(accepted: bool, message: bool) -> Incoming {
        let meta = FileMeta {
            id: "f1".into(),
            file_name: if message { "hi.txt" } else { "Report.pdf" }.into(),
            size: 1200,
            file_type: if message {
                "text/plain"
            } else {
                "application/pdf"
            }
            .into(),
            sha256: None,
            preview: message.then(|| "hello".to_owned()),
            metadata: None,
        };
        Incoming {
            id: "r1".into(),
            sender: Device {
                alias: "Pixel".into(),
                model: None,
                kind: DeviceType::Mobile,
                fingerprint: "AB".into(),
                ip: "192.168.1.9".into(),
                port: 53317,
                https: true,
                version: "2.1".into(),
            },
            files: vec![meta],
            bytes: 1200,
            accepted,
            expires_in: (!accepted && !message).then(|| Duration::from_secs(300)),
        }
    }

    #[test]
    fn starts_in_the_right_state_and_counts_down() {
        let r = Request::new(incoming(false, false), "/tmp/in".into());
        assert_eq!(r.state(), State::Waiting);
        let left = r.remaining().unwrap();
        assert!(left <= Duration::from_secs(300) && left > Duration::from_secs(298));
        assert!(r.status().starts_with("Waiting"), "{}", r.status());
        assert!(!r.state().is_over());
        let r = Request::new(incoming(true, false), "/tmp/in".into());
        assert_eq!(
            r.state(),
            State::Receiving {
                done: 0,
                total: 1200
            }
        );
        assert_eq!(r.remaining(), None);
    }

    #[test]
    fn clock_rounds_up() {
        assert_eq!(clock(Duration::from_secs(300)), "5:00");
        assert_eq!(clock(Duration::from_millis(271_400)), "4:32");
        assert_eq!(clock(Duration::from_secs(0)), "0:00");
    }

    #[test]
    fn states_know_when_they_are_over() {
        assert!(State::Expired.is_over() && State::Saved.is_over());
        assert!(!State::Waiting.is_over());
        assert!(State::Receiving { done: 1, total: 2 }
            .same_kind(&State::Receiving { done: 2, total: 2 }));
    }
}
