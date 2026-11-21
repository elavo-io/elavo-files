//! LocalSend in Files: the service that receives in the background, the
//! nearby devices, and the windows for sending and receiving.
//!
//! The service starts with the app, so receiving works for as long as
//! Files runs: in a window, or with `--localsend-daemon` and no window at
//! all. Someone sending gets an AirDrop-style request with their device,
//! the files and Decline or Accept; accepted files go to the save folder
//! (Downloads unless changed). Sending is by dropping files on a device in
//! the sidebar or the LocalSend view, or Send with LocalSend… on a
//! selection, which asks which device.
//!
//! Other devices see this computer under its name from System Settings
//! (General → About → Name), followed live; see [`watch_name`].
//!
//! Incoming requests are in [`requests`]: shown on the LocalSend page and
//! in a dialog that doesn't take keys meant for other apps.

mod requests;

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

use elavo_files_core::format;
use elavo_files_core::localsend::{
    config, name, Config, Decision, Device, DeviceType, Event, FileMeta, SendState, Service,
    Transfer,
};
use futures_util::StreamExt;
use gtk::prelude::*;
use gtk::{gdk, gio, glib};

use crate::i18n::tr;

type Listener = Box<dyn Fn() -> bool>;

/// Where the LocalSend view is, for the sidebar and history.
pub const URI: &str = "localsend:///";

pub fn is_uri(uri: &str) -> bool {
    uri.starts_with("localsend:")
}

struct Ui {
    service: Option<Service>,
    /// Why there is no service, when there isn't.
    problem: RefCell<Option<String>>,
    devices: RefCell<Vec<Device>>,
    listeners: RefCell<Vec<Listener>>,
    sending: RefCell<HashMap<String, Rc<Send>>>,
    /// The computer's name from hostnamed, once known.
    system_name: RefCell<Option<String>>,
    /// Keeps following hostnamed's name changes.
    name_watch: RefCell<Option<gio::SignalSubscription>>,
    /// Keeps following localsend.conf.
    config_watch: RefCell<Option<gio::FileMonitor>>,
    app: glib::WeakRef<gtk::Application>,
}

thread_local! {
    static UI: RefCell<Option<Rc<Ui>>> = const { RefCell::new(None) };
}

fn ui() -> Option<Rc<Ui>> {
    UI.with(|u| u.borrow().clone())
}

fn log(msg: &str) {
    if std::env::var_os("ELAVO_FILES_LOG").is_some_and(|v| !v.is_empty() && v != "0") {
        eprintln!("elavo-files: {msg}");
    }
}

/// Start receiving and looking for devices. `ELAVO_LOCALSEND=0` leaves it
/// off.
pub fn start(app: &gtk::Application) {
    if ui().is_some() || std::env::var("ELAVO_LOCALSEND").is_ok_and(|v| v == "0") {
        return;
    }
    let mut cfg = config::read();
    config::resolve_pin(&mut cfg);
    cfg.alias = alias_for(&cfg, None);
    let port = cfg.port;
    let (service, problem) = match Service::start(cfg) {
        Ok((service, rx)) => {
            let (tx, mut events) = futures_channel::mpsc::unbounded::<Event>();
            std::thread::Builder::new()
                .name("localsend-events".into())
                .spawn(move || {
                    while let Ok(e) = rx.recv() {
                        if tx.unbounded_send(e).is_err() {
                            return;
                        }
                    }
                })
                .expect("a thread");
            glib::MainContext::default().spawn_local(async move {
                while let Some(e) = events.next().await {
                    handle(e);
                }
            });
            (Some(service), None)
        }
        Err(e) => {
            log(&format!("localsend: port {port}: {e}"));
            (
                None,
                Some(format!(
                    "{} {port}: {e}",
                    tr("LocalSend can't receive: another app is using port")
                )),
            )
        }
    };
    UI.with(|u| {
        u.replace(Some(Rc::new(Ui {
            service,
            problem: RefCell::new(problem),
            devices: RefCell::default(),
            listeners: RefCell::default(),
            sending: RefCell::default(),
            system_name: RefCell::default(),
            name_watch: RefCell::default(),
            config_watch: RefCell::default(),
            app: app.downgrade(),
        })))
    });
    requests::init(app);
    watch_name();
    watch_config();
}

/// Follow localsend.conf, which System Settings' Sharing page writes, so
/// its changes apply at once. The PIN is read again from the keyring each
/// time; Sharing touches the file when it sets one.
fn watch_config() {
    let file = gio::File::for_path(config::path());
    let Ok(monitor) = file.monitor_file(gio::FileMonitorFlags::WATCH_MOVES, gio::Cancellable::NONE)
    else {
        return;
    };
    let pending = Rc::new(std::cell::Cell::new(false));
    monitor.connect_changed(move |_, _, _, event| {
        if !matches!(
            event,
            gio::FileMonitorEvent::ChangesDoneHint
                | gio::FileMonitorEvent::Created
                | gio::FileMonitorEvent::MovedIn
                | gio::FileMonitorEvent::Renamed
                | gio::FileMonitorEvent::Deleted
        ) || pending.replace(true)
        {
            return;
        }
        let pending = pending.clone();
        glib::timeout_add_local_once(std::time::Duration::from_millis(200), move || {
            pending.set(false);
            reload_config();
        });
    });
    if let Some(u) = ui() {
        u.config_watch.replace(Some(monitor));
    }
}

fn reload_config() {
    let Some(u) = ui() else {
        return;
    };
    let mut c = config::read();
    config::resolve_pin(&mut c);
    c.alias = alias_for(&c, system_name().as_deref());
    if let Some(s) = &u.service {
        if s.config() == c {
            return;
        }
        log("localsend: settings changed on disk");
        s.configure(c);
    }
    u.listeners.borrow_mut().retain(|f| f());
}

pub fn is_favorite(fingerprint: &str) -> bool {
    config().is_favorite(fingerprint)
}

/// Add a device nearby to the Favorites, or take it off.
pub fn toggle_favorite(fingerprint: &str) {
    let mut c = config();
    if c.is_favorite(fingerprint) {
        c.favorites
            .retain(|f| !f.fingerprint.eq_ignore_ascii_case(fingerprint));
    } else {
        let alias = device(fingerprint)
            .map(|d| d.alias)
            .unwrap_or_else(|| fingerprint.chars().take(8).collect());
        c.favorites.push(config::Favorite {
            fingerprint: fingerprint.to_owned(),
            alias,
        });
    }
    configure(c);
}

/// Open System Settings on its Sharing page, where LocalSend is set up.
pub fn open_sharing_settings(parent: Option<&gtk::Window>) {
    open_settings_pane("sharing", parent);
}

/// What other devices see: a name set by hand in localsend.conf, else the
/// computer's, else the host name.
fn alias_for(c: &Config, system: Option<&str>) -> String {
    c.custom_alias
        .clone()
        .or_else(|| system.map(str::to_owned))
        .unwrap_or_else(name::fallback)
}

/// Follow the computer's name in hostnamed (what System Settings sets),
/// and announce again when it changes, so devices nearby show the new
/// one. `ELAVO_HOSTNAME_BUS=session` reads a stand-in on the session bus,
/// for tests.
fn watch_name() {
    let bus = if std::env::var("ELAVO_HOSTNAME_BUS").is_ok_and(|b| b == "session") {
        gio::BusType::Session
    } else {
        gio::BusType::System
    };
    glib::spawn_future_local(async move {
        let conn = match gio::bus_get_future(bus).await {
            Ok(c) => c,
            Err(e) => {
                log(&format!("localsend: no bus for the computer's name: {e}"));
                return;
            }
        };
        let read = {
            let conn = conn.clone();
            move || {
                let conn = conn.clone();
                glib::spawn_future_local(async move {
                    let reply = conn
                        .call_future(
                            Some(name::HOSTNAME_BUS_NAME),
                            name::HOSTNAME_PATH,
                            "org.freedesktop.DBus.Properties",
                            "GetAll",
                            Some(&(name::HOSTNAME_INTERFACE,).to_variant()),
                            Some(glib::VariantTy::new("(a{sv})").expect("a type")),
                            gio::DBusCallFlags::NONE,
                            5000,
                        )
                        .await;
                    match reply {
                        Ok(v) => {
                            if let Some(n) = name::from_properties(&v) {
                                set_system_name(n);
                            }
                        }
                        Err(e) => log(&format!("localsend: reading the computer's name: {e}")),
                    }
                });
            }
        };
        read();
        // hostnamed comes and goes; the bus routes its signals by name.
        let watch = conn.subscribe_to_signal(
            Some(name::HOSTNAME_BUS_NAME),
            Some("org.freedesktop.DBus.Properties"),
            Some("PropertiesChanged"),
            Some(name::HOSTNAME_PATH),
            Some(name::HOSTNAME_INTERFACE),
            gio::DBusSignalFlags::NONE,
            move |_| read(),
        );
        if let Some(u) = ui() {
            u.name_watch.replace(Some(watch));
        }
    });
}

fn set_system_name(n: String) {
    let Some(u) = ui() else {
        return;
    };
    if u.system_name.borrow().as_deref() == Some(n.as_str()) {
        return;
    }
    log(&format!(
        "localsend: the computer is called \u{201c}{n}\u{201d}"
    ));
    u.system_name.replace(Some(n));
    if let Some(s) = &u.service {
        let mut c = s.config();
        let alias = alias_for(&c, u.system_name.borrow().as_deref());
        if alias != c.alias {
            c.alias = alias;
            // Announces again with the new name.
            s.configure(c);
        }
    }
    u.listeners.borrow_mut().retain(|f| f());
}

/// The computer's name, as System Settings shows it.
pub fn system_name() -> Option<String> {
    ui().and_then(|u| u.system_name.borrow().clone())
}

/// Use the computer's name again instead of one set in localsend.conf.
pub fn use_system_name() {
    let mut c = config();
    c.custom_alias = None;
    configure(c);
}

/// Open System Settings where the computer is named.
pub fn open_name_settings(parent: Option<&gtk::Window>) {
    open_settings_pane("about", parent);
}

fn open_settings_pane(pane: &str, parent: Option<&gtk::Window>) {
    let launch = gio::AppInfo::create_from_commandline(
        format!("elavo-system-settings --pane {pane}"),
        Some("System Settings"),
        gio::AppInfoCreateFlags::NONE,
    )
    .and_then(|app| {
        let ctx = parent.map(|p| WidgetExt::display(p).app_launch_context());
        app.launch(&[], ctx.as_ref())
    });
    if let Err(e) = launch {
        eprintln!("elavo-files: opening System Settings: {e}");
    }
}

pub fn can_open_name_settings() -> bool {
    glib::find_program_in_path("elavo-system-settings").is_some()
}

fn handle(e: Event) {
    let Some(ui) = ui() else {
        return;
    };
    match e {
        Event::Devices(list) => {
            log(&format!("localsend: {} devices", list.len()));
            ui.devices.replace(list);
            ui.listeners.borrow_mut().retain(|f| f());
        }
        Event::Incoming(i) => {
            log(&format!(
                "localsend: incoming from {} ({} files)",
                i.sender.alias,
                i.files.len()
            ));
            let folder = ui
                .service
                .as_ref()
                .map(|s| s.config().save_folder)
                .unwrap_or_default();
            requests::incoming(i, folder);
        }
        Event::Dismiss(id) => requests::dismissed(&id),
        Event::Transfer(t) => {
            if t.receiving {
                requests::transfer(&t);
            } else {
                let s = ui.sending.borrow().get(&t.id).cloned();
                if let Some(s) = s {
                    s.update(&t);
                }
            }
        }
        Event::Error(msg) => {
            log(&format!("localsend: {msg}"));
            ui.problem.replace(Some(msg));
            ui.listeners.borrow_mut().retain(|f| f());
        }
    }
}

pub fn is_running() -> bool {
    ui().is_some_and(|u| u.service.is_some())
}

pub fn devices() -> Vec<Device> {
    ui().map(|u| u.devices.borrow().clone()).unwrap_or_default()
}

pub fn device(fingerprint: &str) -> Option<Device> {
    devices()
        .into_iter()
        .find(|d| d.same_fingerprint(fingerprint))
}

/// Call `f` when the devices or the status change, while it returns true.
pub fn connect_changed(f: impl Fn() -> bool + 'static) {
    if let Some(u) = ui() {
        u.listeners.borrow_mut().push(Box::new(f));
    }
}

/// Announce again, for Refresh.
pub fn refresh() {
    if let Some(s) = ui().and_then(|u| u.service.clone()) {
        s.discover();
    }
}

pub fn config() -> Config {
    ui().and_then(|u| u.service.as_ref().map(Service::config))
        .unwrap_or_else(config::read)
}

/// Save LocalSend settings and use them at once.
pub fn configure(mut c: Config) {
    c.alias = alias_for(&c, system_name().as_deref());
    if let Err(e) = config::write(&c) {
        eprintln!("elavo-files: saving LocalSend settings: {e}");
    }
    if let Some(u) = ui() {
        if let Some(s) = &u.service {
            s.configure(c);
        }
        u.listeners.borrow_mut().retain(|f| f());
    }
}

/// What System Settings' Sharing page has set, in a sentence.
pub fn sharing_summary() -> String {
    let c = config();
    let from = if !c.receive {
        tr("Receiving is off.")
    } else if c.favorites_only {
        tr("Receiving from favorites only.")
    } else {
        tr("Receiving from everyone nearby.")
    };
    let pin = if c.require_pin {
        tr("A PIN is required.")
    } else {
        tr("No PIN is required.")
    };
    let folder = c
        .save_folder
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| c.save_folder.to_string_lossy().into_owned());
    format!("{from} {pin} {} {folder}.", tr("Files are saved to"))
}

/// One line on how others see this device.
pub fn status() -> String {
    let Some(u) = ui() else {
        return tr("LocalSend is off.");
    };
    if let Some(p) = u.problem.borrow().clone() {
        if u.service.is_none() {
            return p;
        }
    }
    let Some(s) = &u.service else {
        return tr("LocalSend is off.");
    };
    let c = s.config();
    let info = s.info();
    let how = if info.protocol.as_deref() == Some("https") {
        tr("encrypted")
    } else {
        tr("not encrypted")
    };
    if c.receive {
        format!(
            "{} \u{201c}{}\u{201d} \u{00b7} {how}",
            tr("Discoverable as"),
            c.alias
        )
    } else {
        format!(
            "\u{201c}{}\u{201d} \u{00b7} {}",
            c.alias,
            tr("not receiving files")
        )
    }
}

pub fn device_icon(kind: DeviceType) -> &'static str {
    match kind {
        DeviceType::Mobile => "phone-symbolic",
        DeviceType::Desktop => "computer-symbolic",
        DeviceType::Web => "web-browser-symbolic",
        DeviceType::Headless => "utilities-terminal-symbolic",
        DeviceType::Server => "network-server-symbolic",
    }
}

fn kind_label(d: &Device) -> String {
    let kind = match d.kind {
        DeviceType::Mobile => tr("Phone or tablet"),
        DeviceType::Desktop => tr("Computer"),
        DeviceType::Web => tr("Web browser"),
        DeviceType::Headless => tr("Command line"),
        DeviceType::Server => tr("Server"),
    };
    match &d.model {
        Some(m) => format!("{m} \u{00b7} {kind}"),
        None => kind,
    }
}

/// A round picture for a device: the protocol carries no photo, so its
/// kind's icon, as in the official app.
pub fn avatar(kind: DeviceType, size: i32) -> gtk::Widget {
    let image = gtk::Image::builder()
        .icon_name(device_icon(kind))
        .pixel_size(size / 2)
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .hexpand(true)
        .vexpand(true)
        .build();
    let frame = gtk::Box::builder()
        .width_request(size)
        .height_request(size)
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .css_classes(["files-ls-avatar"])
        .build();
    frame.append(&image);
    frame.upcast()
}

fn file_icon(meta: &FileMeta) -> gio::Icon {
    let mime = match meta.file_type.as_str() {
        "image" => "image/png",
        "video" => "video/mp4",
        "pdf" => "application/pdf",
        "text" => "text/plain",
        "apk" => "application/vnd.android.package-archive",
        "other" | "" => "application/octet-stream",
        m => m,
    };
    let name = meta.file_name.rsplit('/').next().unwrap_or(&meta.file_name);
    let ct = if meta.file_name.contains('/') {
        "inode/directory".to_owned()
    } else {
        gio::content_type_from_mime_type(mime)
            .map(|c| c.to_string())
            .unwrap_or_else(|| gio::content_type_guess(Some(name), None).0.to_string())
    };
    gio::content_type_get_icon(&ct)
}

/// The items as the person sees them: a folder sent once, not its files.
fn top_items(files: &[FileMeta]) -> Vec<(String, gio::Icon, u64)> {
    let mut out: Vec<(String, gio::Icon, u64)> = Vec::new();
    for f in files {
        let top = f
            .file_name
            .split('/')
            .next()
            .unwrap_or(&f.file_name)
            .to_owned();
        if let Some(item) = out.iter_mut().find(|(n, _, _)| *n == top) {
            item.2 += f.size;
        } else {
            out.push((top, file_icon(f), f.size));
        }
    }
    out
}

fn label(text: &str, classes: &[&str]) -> gtk::Label {
    gtk::Label::builder()
        .label(text)
        .xalign(0.0)
        .wrap(true)
        .max_width_chars(40)
        .css_classes(classes.iter().map(|c| c.to_string()).collect::<Vec<_>>())
        .build()
}

fn dialog_window(app: Option<&gtk::Application>, title: &str) -> gtk::Window {
    let w = gtk::Window::builder()
        .title(title)
        .resizable(false)
        .default_width(400)
        .css_classes(["background", "files-dialog", "files-ls-dialog"])
        .build();
    if let Some(app) = app {
        w.set_application(Some(app));
    }
    w.set_titlebar(Some(&gtk::Box::new(gtk::Orientation::Horizontal, 0)));
    w
}

fn body_box() -> gtk::Box {
    gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .margin_top(20)
        .margin_bottom(16)
        .margin_start(20)
        .margin_end(20)
        .build()
}

fn items_list(files: &[FileMeta]) -> gtk::Box {
    let list = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(4)
        .css_classes(["files-ls-items"])
        .build();
    let items = top_items(files);
    for (name, icon, size) in items.iter().take(5) {
        let row = gtk::Box::builder().spacing(10).build();
        row.append(&gtk::Image::builder().gicon(icon).pixel_size(32).build());
        row.append(
            &gtk::Label::builder()
                .label(name)
                .xalign(0.0)
                .hexpand(true)
                .ellipsize(gtk::pango::EllipsizeMode::Middle)
                .build(),
        );
        row.append(
            &gtk::Label::builder()
                .label(format::size(*size))
                .css_classes(["files-dim"])
                .build(),
        );
        list.append(&row);
    }
    if items.len() > 5 {
        list.append(&label(
            &format!("{} {} {}", tr("and"), items.len() - 5, tr("more")),
            &["files-dim"],
        ));
    }
    list
}

fn count_text(n: usize, bytes: u64) -> String {
    let items = if n == 1 {
        tr("1 item")
    } else {
        format!("{n} {}", tr("items"))
    };
    format!("{items}, {}", format::size(bytes))
}

fn service() -> Option<Service> {
    ui().and_then(|u| u.service.clone())
}

fn decide(id: &str, d: Decision) {
    if let Some(s) = ui().and_then(|u| u.service.clone()) {
        s.decide(id, d);
    }
}

/// Sending: waiting for an answer, then progress.
struct Send {
    window: gtk::Window,
    status: gtk::Label,
    progress: gtk::ProgressBar,
    buttons: gtk::Box,
    pin: gtk::Entry,
    device: Device,
    paths: Vec<PathBuf>,
    id: RefCell<String>,
    _hold: gio::ApplicationHoldGuard,
}

impl Send {
    fn button(&self, text: &str, suggested: bool, f: impl Fn() + 'static) {
        let b = gtk::Button::with_label(text);
        if suggested {
            b.add_css_class("suggested-action");
        }
        b.connect_clicked(move |_| f());
        self.buttons.append(&b);
    }

    fn clear(&self) {
        while let Some(c) = self.buttons.first_child() {
            self.buttons.remove(&c);
        }
    }

    fn close_button(self: &Rc<Self>) {
        self.clear();
        let w = self.window.clone();
        self.button(&tr("Close"), true, move || w.close());
    }

    fn start(self: &Rc<Self>, pin: Option<String>) {
        let Some(u) = ui() else {
            return;
        };
        let Some(s) = &u.service else {
            return;
        };
        let id = s.send(self.device.clone(), self.paths.clone(), pin);
        let old = self.id.borrow().clone();
        let _ = u.sending.borrow_mut().remove(&old);
        self.id.replace(id.clone());
        u.sending.borrow_mut().insert(id, self.clone());
        self.pin.set_visible(false);
        self.progress.set_visible(false);
        self.clear();
        let weak = Rc::downgrade(self);
        self.button(&tr("Cancel"), false, move || {
            if let Some(me) = weak.upgrade() {
                me.window.close();
            }
        });
    }

    fn update(self: &Rc<Self>, t: &Transfer) {
        let who = format!("\u{201c}{}\u{201d}", t.device);
        match &t.state {
            SendState::Preparing => self.status.set_label(&tr("Preparing\u{2026}")),
            SendState::Waiting => self.status.set_label(&format!(
                "{} {who} {}",
                tr("Waiting for"),
                tr("to accept\u{2026}")
            )),
            SendState::Sending => {
                self.progress.set_visible(true);
                let f = if t.total == 0 {
                    0.0
                } else {
                    t.done as f64 / t.total as f64
                };
                self.progress.set_fraction(f);
                self.status.set_label(&format!(
                    "{} {} {}",
                    format::size(t.done),
                    tr("of"),
                    format::size(t.total)
                ));
            }
            SendState::Done => {
                self.progress.set_visible(true);
                self.progress.set_fraction(1.0);
                self.status.set_label(&format!("{} {who}.", tr("Sent to")));
                self.close_button();
                let w = self.window.downgrade();
                glib::timeout_add_seconds_local_once(2, move || {
                    if let Some(w) = w.upgrade() {
                        w.close();
                    }
                });
            }
            SendState::Declined => {
                self.status.set_label(&format!("{who} {}", tr("declined.")));
                self.close_button();
            }
            SendState::Busy => {
                self.status.set_label(&format!(
                    "{who} {}",
                    tr("is busy with another transfer. Try again in a moment.")
                ));
                self.close_button();
            }
            SendState::NeedsPin => {
                self.status
                    .set_label(&format!("{who} {}", tr("asks for a PIN.")));
                self.pin.set_visible(true);
                self.pin.set_text("");
                self.pin.grab_focus();
                self.clear();
                let w = self.window.clone();
                self.button(&tr("Cancel"), false, move || w.close());
                let weak = Rc::downgrade(self);
                let send = move || {
                    if let Some(me) = weak.upgrade() {
                        let pin = me.pin.text().to_string();
                        me.start(Some(pin));
                    }
                };
                let s2 = send.clone();
                self.pin.connect_activate(move |_| s2());
                self.button(&tr("Send"), true, send);
            }
            SendState::Cancelled => self.window.close(),
            SendState::Error(e) => {
                self.status
                    .set_label(&format!("{} {who}: {e}", tr("Couldn't send to")));
                self.close_button();
            }
        }
    }
}

/// Send local files to a device, showing how it goes.
pub fn send_files(parent: Option<&gtk::Window>, device: &Device, files: &[gio::File]) {
    let Some(u) = ui() else {
        return;
    };
    let paths: Vec<PathBuf> = files.iter().filter_map(|f| f.path()).collect();
    if paths.is_empty() || u.service.is_none() {
        return;
    }
    log(&format!(
        "localsend: sending {} items to {}",
        paths.len(),
        device.alias
    ));
    let app = u.app.upgrade();
    let window = dialog_window(app.as_ref(), &tr("LocalSend"));
    window.set_transient_for(parent);
    let body = body_box();
    let top = gtk::Box::builder().spacing(14).build();
    top.append(&avatar(device.kind, 56));
    let text = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(4)
        .hexpand(true)
        .valign(gtk::Align::Center)
        .build();
    let what = if paths.len() == 1 {
        format!(
            "\u{201c}{}\u{201d}",
            paths[0].file_name().unwrap_or_default().to_string_lossy()
        )
    } else {
        format!("{} {}", paths.len(), tr("items"))
    };
    text.append(&label(
        &format!(
            "{} {what} {} \u{201c}{}\u{201d}",
            tr("Sending"),
            tr("to"),
            device.alias
        ),
        &["files-dialog-title"],
    ));
    let status = label(&tr("Preparing\u{2026}"), &["files-dim"]);
    text.append(&status);
    top.append(&text);
    body.append(&top);
    let pin = gtk::Entry::builder()
        .placeholder_text(tr("PIN"))
        .input_purpose(gtk::InputPurpose::Pin)
        .visibility(false)
        .visible(false)
        .build();
    body.append(&pin);
    let progress = gtk::ProgressBar::builder().visible(false).build();
    body.append(&progress);
    let buttons = gtk::Box::builder()
        .spacing(8)
        .halign(gtk::Align::End)
        .build();
    body.append(&buttons);
    window.set_child(Some(&body));
    let hold = app.as_ref().map(|a| a.hold()).expect("the app");
    let send = Rc::new(Send {
        window: window.clone(),
        status,
        progress,
        buttons,
        pin,
        device: device.clone(),
        paths,
        id: RefCell::default(),
        _hold: hold,
    });
    let weak = Rc::downgrade(&send);
    window.connect_close_request(move |_| {
        if let (Some(me), Some(u)) = (weak.upgrade(), ui()) {
            let id = me.id.borrow().clone();
            if let Some(s) = &u.service {
                s.cancel(&id);
            }
            u.sending.borrow_mut().remove(&id);
        }
        glib::Propagation::Proceed
    });
    send.start(None);
    window.present();
}

/// A device to drop on or click: its picture, name and kind.
pub fn tile(device: &Device) -> gtk::Box {
    let tile = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .width_request(132)
        .css_classes(["files-ls-tile"])
        .build();
    tile.append(&avatar(device.kind, 72));
    tile.append(
        &gtk::Label::builder()
            .label(&device.alias)
            .justify(gtk::Justification::Center)
            .wrap(true)
            .max_width_chars(14)
            .css_classes(["files-ls-tile-name"])
            .build(),
    );
    tile.append(
        &gtk::Label::builder()
            .label(device.model.as_deref().unwrap_or(""))
            .css_classes(["files-dim", "files-ls-tile-model"])
            .build(),
    );
    tile.set_tooltip_text(Some(&format!(
        "{} \u{00b7} {}",
        kind_label(device),
        device.ip
    )));
    tile
}

/// Files dropped on `widget` go to `device` (found again by fingerprint,
/// so the drop uses its address at the time).
pub fn take_drops(widget: &impl IsA<gtk::Widget>, fingerprint: &str) {
    let drop = gtk::DropTarget::new(gdk::FileList::static_type(), gdk::DragAction::COPY);
    let fp = fingerprint.to_owned();
    drop.connect_enter(|t, _, _| {
        if let Some(w) = t.widget() {
            w.add_css_class("files-ls-drop");
        }
        gdk::DragAction::COPY
    });
    drop.connect_leave(|t| {
        if let Some(w) = t.widget() {
            w.remove_css_class("files-ls-drop");
        }
    });
    drop.connect_drop(move |t, value, _, _| {
        let w = t.widget();
        if let Some(w) = &w {
            w.remove_css_class("files-ls-drop");
        }
        let files = crate::views::files_of(value);
        let Some(device) = device(&fp) else {
            return false;
        };
        let parent = w.and_then(|w| w.root()).and_downcast::<gtk::Window>();
        send_files(parent.as_ref(), &device, &files);
        true
    });
    widget.as_ref().add_controller(drop);
}

fn tiles(flow: &gtk::FlowBox, on_pick: Rc<dyn Fn(&Device)>) {
    flow.remove_all();
    for d in devices() {
        let t = tile(&d);
        take_drops(&t, &d.fingerprint);
        let click = gtk::GestureClick::new();
        let (d2, pick) = (d.clone(), on_pick.clone());
        click.connect_released(move |_, _, _, _| pick(&d2));
        t.add_controller(click);
        flow.append(&t);
    }
}

fn looking() -> gtk::Box {
    let b = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(10)
        .valign(gtk::Align::Center)
        .vexpand(true)
        .build();
    b.append(
        &gtk::Spinner::builder()
            .spinning(true)
            .width_request(24)
            .height_request(24)
            .build(),
    );
    b.append(
        &gtk::Label::builder()
            .label(tr("Looking for devices nearby\u{2026}"))
            .css_classes(["files-dim"])
            .build(),
    );
    b.append(
        &gtk::Label::builder()
            .label(tr(
                "Open LocalSend on the other device, on the same network.",
            ))
            .wrap(true)
            .justify(gtk::Justification::Center)
            .css_classes(["files-dim"])
            .build(),
    );
    b
}

/// Send with LocalSend…: pick a device for `files`.
pub fn pick_and_send(parent: Option<&gtk::Window>, files: Vec<gio::File>) {
    let Some(u) = ui() else {
        return;
    };
    refresh();
    let window = dialog_window(u.app.upgrade().as_ref(), &tr("Send with LocalSend"));
    window.set_transient_for(parent);
    window.set_default_width(460);
    let body = body_box();
    body.append(&label(
        &format!(
            "{} {} {}",
            tr("Send"),
            if files.len() == 1 {
                format!(
                    "\u{201c}{}\u{201d}",
                    files[0].basename().unwrap_or_default().to_string_lossy()
                )
            } else {
                format!("{} {}", files.len(), tr("items"))
            },
            tr("with LocalSend")
        ),
        &["files-dialog-title"],
    ));
    let stack = gtk::Stack::builder().vhomogeneous(false).build();
    let flow = gtk::FlowBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .max_children_per_line(3)
        .min_children_per_line(1)
        .homogeneous(true)
        .column_spacing(8)
        .row_spacing(8)
        .build();
    stack.add_named(&flow, Some("devices"));
    stack.add_named(&looking(), Some("empty"));
    stack.set_size_request(-1, 170);
    body.append(&stack);
    let buttons = gtk::Box::builder()
        .spacing(8)
        .halign(gtk::Align::End)
        .build();
    let cancel = gtk::Button::with_label(&tr("Cancel"));
    let w = window.clone();
    cancel.connect_clicked(move |_| w.close());
    buttons.append(&cancel);
    body.append(&buttons);
    window.set_child(Some(&body));
    let files = Rc::new(files);
    let (w, f) = (window.downgrade(), files.clone());
    let on_pick: Rc<dyn Fn(&Device)> = Rc::new(move |d| {
        if let Some(w) = w.upgrade() {
            let parent = w.transient_for();
            w.close();
            send_files(parent.as_ref(), d, &f);
        }
    });
    let fill = {
        let (flow, stack, on_pick) = (flow.clone(), stack.clone(), on_pick.clone());
        move || {
            tiles(&flow, on_pick.clone());
            stack.set_visible_child_name(if devices().is_empty() {
                "empty"
            } else {
                "devices"
            });
        }
    };
    fill();
    let alive = window.downgrade();
    connect_changed(move || {
        if alive.upgrade().is_none() {
            return false;
        }
        fill();
        true
    });
    let keys = gtk::ShortcutController::new();
    keys.add_shortcut(gtk::Shortcut::new(
        gtk::ShortcutTrigger::parse_string("Escape"),
        Some(gtk::CallbackAction::new(|w, _| {
            if let Some(w) = w.downcast_ref::<gtk::Window>() {
                w.close();
            }
            glib::Propagation::Stop
        })),
    ));
    window.add_controller(keys);
    window.present();
}

/// The LocalSend view in a window: the devices nearby, to drop files on,
/// and how this device shows to them.
pub struct Page {
    pub root: gtk::Box,
    _requests: Rc<requests::RequestList>,
}

impl Page {
    pub fn new() -> Rc<Self> {
        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .hexpand(true)
            .vexpand(true)
            .css_classes(["files-ls-page"])
            .build();
        let requests = requests::RequestList::new();
        root.append(&requests.root);
        let stack = gtk::Stack::builder().vexpand(true).build();
        let flow = gtk::FlowBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .valign(gtk::Align::Start)
            .halign(gtk::Align::Center)
            .max_children_per_line(6)
            .homogeneous(true)
            .column_spacing(16)
            .row_spacing(16)
            .margin_top(40)
            .build();
        stack.add_named(
            &gtk::ScrolledWindow::builder()
                .hscrollbar_policy(gtk::PolicyType::Never)
                .child(&flow)
                .build(),
            Some("devices"),
        );
        stack.add_named(&looking(), Some("empty"));
        root.append(&stack);
        let footer = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(8)
            .margin_bottom(28)
            .halign(gtk::Align::Center)
            .build();
        footer.append(
            &gtk::Image::builder()
                .icon_name("network-wireless-symbolic")
                .pixel_size(48)
                .css_classes(["files-ls-radar"])
                .build(),
        );
        let status = gtk::Label::builder()
            .justify(gtk::Justification::Center)
            .wrap(true)
            .css_classes(["files-dim"])
            .build();
        footer.append(&status);
        let row = gtk::Box::builder()
            .spacing(8)
            .halign(gtk::Align::Center)
            .build();
        let refresh_b = gtk::Button::with_label(&tr("Refresh"));
        refresh_b.connect_clicked(|_| refresh());
        let settings = gtk::Button::with_label(&tr("LocalSend Settings\u{2026}"));
        settings.set_action_name(Some("app.localsend-settings"));
        row.append(&refresh_b);
        row.append(&settings);
        footer.append(&row);
        root.append(&footer);
        let this = Rc::new(Self {
            root,
            _requests: requests,
        });
        let on_pick: Rc<dyn Fn(&Device)> = {
            let root = this.root.downgrade();
            Rc::new(move |d: &Device| {
                let parent = root
                    .upgrade()
                    .and_then(|r| r.root())
                    .and_downcast::<gtk::Window>();
                choose_and_send(parent, d.clone());
            })
        };
        let fill = {
            let (flow, stack, status) = (flow.clone(), stack.clone(), status.clone());
            move || {
                tiles(&flow, on_pick.clone());
                stack.set_visible_child_name(if devices().is_empty() {
                    "empty"
                } else {
                    "devices"
                });
                status.set_label(&status_text());
            }
        };
        fill();
        let alive = this.root.downgrade();
        connect_changed(move || {
            if alive.upgrade().is_none() {
                return false;
            }
            fill();
            true
        });
        this
    }
}

fn status_text() -> String {
    status()
}

/// Clicking a device: pick files for it.
fn choose_and_send(parent: Option<gtk::Window>, device: Device) {
    let dialog = gtk::FileDialog::builder()
        .title(format!(
            "{} \u{201c}{}\u{201d}",
            tr("Send to"),
            device.alias
        ))
        .accept_label(tr("Send"))
        .modal(true)
        .build();
    let p = parent.clone();
    dialog.open_multiple(parent.as_ref(), gio::Cancellable::NONE, move |r| {
        let Ok(list) = r else {
            return;
        };
        let files: Vec<gio::File> = (0..list.n_items())
            .filter_map(|i| list.item(i).and_downcast::<gio::File>())
            .collect();
        send_files(p.as_ref(), &device, &files);
    });
}
