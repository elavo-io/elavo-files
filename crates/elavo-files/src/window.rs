//! A Files window: the sidebar on the left with the window buttons over
//! it, and on the right a toolbar over the folder, shown as Icons or List.
//! The folder is a monitored `gtk::DirectoryList`, so it follows changes on
//! disk as they happen. File operations, the context menus, the clipboard,
//! drag and drop, Rename and Undo are in `fileops.rs`.

mod fileops;

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use elavo_files_core::history::History;
use elavo_files_core::sort::Key;
use elavo_files_core::{eapp, entry, format, trash, zoom};
use gtk::prelude::*;
use gtk::{gio, glib};

use crate::bundles;
use crate::i18n::tr;
use crate::jobs::Jobs;
use crate::rubberband;
use crate::sidebar::{self, Sidebar};
use crate::state::{self, Mode, State};
use crate::titlebox::TitleBox;
use crate::views::{self, CellHandler, Cells};

/// The sidebar column, shared by its band in the title bar and the list.
const SIDEBAR_WIDTH: i32 = 200;

/// A window action's handler.
type Handler = Box<dyn Fn(&Rc<Window>)>;

pub struct Window {
    pub window: gtk::ApplicationWindow,
    sidebar: Rc<Sidebar>,
    dir: gtk::DirectoryList,
    filter: gtk::CustomFilter,
    grid: gtk::GridView,
    grid_sel: gtk::MultiSelection,
    grid_scroll: gtk::ScrolledWindow,
    list: gtk::ColumnView,
    list_sel: gtk::MultiSelection,
    list_scroll: gtk::ScrolledWindow,
    name_column: gtk::ColumnViewColumn,
    stack: gtk::Stack,
    message: gtk::Label,
    title: Rc<TitleBox>,
    search_toggle: gtk::ToggleButton,
    search: gtk::SearchEntry,
    history: RefCell<History<String>>,
    state: Cell<State>,
    show_hidden: Rc<Cell<bool>>,
    query: Rc<RefCell<String>>,
    cells: Rc<Cells>,
    jobs: Rc<Jobs>,
    /// The trash bar and the views, which popovers are parented to.
    content: gtk::Box,
    trash_bar: gtk::Revealer,
    undo: RefCell<Option<fileops::Undo>>,
    pending: RefCell<fileops::Pending>,
    menu: RefCell<Option<gtk::PopoverMenu>>,
    me: RefCell<std::rc::Weak<Window>>,
}

fn icon_button(icon: &str, tooltip: &str) -> gtk::Button {
    gtk::Button::builder()
        .icon_name(icon)
        .tooltip_text(tooltip)
        .build()
}

fn pill() -> gtk::Box {
    gtk::Box::builder()
        .css_classes(["files-pill"])
        .valign(gtk::Align::Center)
        .build()
}

fn more_menu() -> gio::Menu {
    let menu = gio::Menu::new();
    let undo = gio::Menu::new();
    undo.append(Some(&tr("Undo")), Some("win.undo"));
    menu.append_section(None, &undo);
    let first = gio::Menu::new();
    first.append(Some(&tr("New Folder")), Some("win.new-folder"));
    first.append(Some(&tr("Show Hidden Files")), Some("win.show-hidden"));
    menu.append_section(None, &first);
    let zoom = gio::Menu::new();
    zoom.append(Some(&tr("Make Icons Larger")), Some("win.zoom-in"));
    zoom.append(Some(&tr("Make Icons Smaller")), Some("win.zoom-out"));
    zoom.append(Some(&tr("Default Icon Size")), Some("win.zoom-reset"));
    menu.append_section(None, &zoom);
    menu
}

fn scrolled(child: &impl IsA<gtk::Widget>) -> gtk::ScrolledWindow {
    gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .hexpand(true)
        .vexpand(true)
        .child(child)
        .css_classes(["files-scroll"])
        .build()
}

/// `ELAVO_FILES_LOG=1` logs navigation and actions to stderr, for tests.
fn log(msg: &str) {
    if std::env::var_os("ELAVO_FILES_LOG").is_some_and(|v| !v.is_empty() && v != "0") {
        eprintln!("elavo-files: {msg}");
    }
}

fn display_name(file: &gio::File) -> String {
    file.query_info(
        "standard::display-name",
        gio::FileQueryInfoFlags::NONE,
        gio::Cancellable::NONE,
    )
    .map(|i| i.display_name().to_string())
    .ok()
    .or_else(|| file.basename().map(|b| b.to_string_lossy().into_owned()))
    .unwrap_or_else(|| file.uri().to_string())
}

impl Window {
    pub fn new(app: &gtk::Application, folder: gio::File) -> Rc<Self> {
        let st = state::read();
        let window = gtk::ApplicationWindow::builder()
            .application(app)
            .title(tr("Files"))
            .default_width(st.width)
            .default_height(st.height)
            .show_menubar(false)
            .build();

        // Title bar: the window buttons over the sidebar, the toolbar over
        // the folder. gtk::WindowControls follows gtk-decoration-layout, so
        // Elavo's close,minimize,maximize: puts them on the left.
        let band = gtk::Box::builder()
            .width_request(SIDEBAR_WIDTH)
            .css_classes(["files-sidebar-band"])
            .build();
        let start_controls = gtk::WindowControls::new(gtk::PackType::Start);
        start_controls.set_valign(gtk::Align::Center);
        band.append(&start_controls);

        let back = icon_button("go-previous-symbolic", &tr("Back"));
        let forward = icon_button("go-next-symbolic", &tr("Forward"));
        back.set_action_name(Some("win.back"));
        forward.set_action_name(Some("win.forward"));
        let nav = pill();
        nav.append(&back);
        nav.append(&forward);

        // Picking a folder in its path menu goes there; the window is made
        // below, so it is reached through this cell.
        let go_to: Rc<RefCell<Option<std::rc::Weak<Window>>>> = Rc::default();
        let title = {
            let go_to = go_to.clone();
            TitleBox::new(move |file| {
                if let Some(w) = go_to.borrow().as_ref().and_then(|w| w.upgrade()) {
                    w.go(file);
                }
            })
        };

        let views_pill = pill();
        let icons_btn = gtk::ToggleButton::builder()
            .icon_name("view-grid-symbolic")
            .tooltip_text(tr("Icons"))
            .action_name("win.view")
            .action_target(&"icons".to_variant())
            .build();
        let list_btn = gtk::ToggleButton::builder()
            .icon_name("view-list-symbolic")
            .tooltip_text(tr("List"))
            .action_name("win.view")
            .action_target(&"list".to_variant())
            .build();
        let columns_btn = gtk::Button::builder()
            .icon_name("elavo-files-view-columns-symbolic")
            .tooltip_text(tr("Columns"))
            .sensitive(false)
            .build();
        let gallery_btn = gtk::Button::builder()
            .icon_name("elavo-files-view-gallery-symbolic")
            .tooltip_text(tr("Gallery"))
            .sensitive(false)
            .build();
        for b in [icons_btn.upcast_ref::<gtk::Widget>(), list_btn.upcast_ref()] {
            views_pill.append(b);
        }
        views_pill.append(&columns_btn);
        views_pill.append(&gallery_btn);

        let group_pill = pill();
        group_pill.append(
            &gtk::Button::builder()
                .icon_name("elavo-files-group-symbolic")
                .tooltip_text(tr("Group"))
                .sensitive(false)
                .build(),
        );

        let share_pill = pill();
        share_pill.append(
            &gtk::Button::builder()
                .icon_name("elavo-files-share-symbolic")
                .tooltip_text(tr("Share"))
                .sensitive(false)
                .build(),
        );
        share_pill.append(
            &gtk::MenuButton::builder()
                .icon_name("view-more-horizontal-symbolic")
                .tooltip_text(tr("More"))
                .menu_model(&more_menu())
                .build(),
        );

        let search = gtk::SearchEntry::builder()
            .placeholder_text(tr("Search"))
            .width_chars(12)
            .valign(gtk::Align::Center)
            .build();
        let revealer = gtk::Revealer::builder()
            .transition_type(gtk::RevealerTransitionType::SlideLeft)
            .child(&search)
            .build();
        let jobs = Jobs::new(&window);
        let search_toggle = gtk::ToggleButton::builder()
            .icon_name("system-search-symbolic")
            .tooltip_text(tr("Search this folder"))
            .valign(gtk::Align::Center)
            .css_classes(["files-round"])
            .build();
        search_toggle
            .bind_property("active", &revealer, "reveal-child")
            .sync_create()
            .build();

        let toolbar = gtk::Box::builder()
            .spacing(8)
            .hexpand(true)
            .css_classes(["files-toolbar"])
            .build();
        toolbar.append(&nav);
        toolbar.append(&title.root);
        toolbar.append(&views_pill);
        toolbar.append(&group_pill);
        toolbar.append(&share_pill);
        toolbar.append(&jobs.button);
        toolbar.append(&revealer);
        toolbar.append(&search_toggle);
        let end_controls = gtk::WindowControls::new(gtk::PackType::End);
        end_controls.set_valign(gtk::Align::Center);
        toolbar.append(&end_controls);

        let chrome = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        chrome.append(&band);
        chrome.append(&gtk::Separator::new(gtk::Orientation::Vertical));
        chrome.append(&toolbar);
        window.set_titlebar(Some(&gtk::WindowHandle::builder().child(&chrome).build()));

        // The folder model, shared by both views.
        let show_hidden = Rc::new(Cell::new(st.show_hidden));
        let query: Rc<RefCell<String>> = Rc::default();
        let filter = {
            let show_hidden = show_hidden.clone();
            let query = query.clone();
            let theme = gtk::IconTheme::for_display(&WidgetExt::display(&window));
            // A theme name counts only if it is a file that is there: the
            // icon cache can list Flatpak exports whose files are missing.
            let themed = move |name: &str| {
                theme.has_icon(name)
                    && theme
                        .lookup_icon(
                            name,
                            &[],
                            64,
                            1,
                            gtk::TextDirection::Ltr,
                            gtk::IconLookupFlags::empty(),
                        )
                        .file()
                        .and_then(|f| f.path())
                        .is_some_and(|p| p.exists())
            };
            gtk::CustomFilter::new(move |obj| {
                let Some(info) = obj.downcast_ref::<gio::FileInfo>() else {
                    return false;
                };
                // Application bundles show as their app, once each.
                eapp::decorate(info, &themed);
                if !show_hidden.get() && entry::is_hidden(info) {
                    return false;
                }
                let q = query.borrow();
                q.is_empty() || entry::name(info).to_lowercase().contains(q.as_str())
            })
        };
        let dir = gtk::DirectoryList::new(Some(entry::ATTRIBUTES), None::<&gio::File>);
        dir.set_monitored(true);
        let filtered = gtk::FilterListModel::new(Some(dir.clone()), Some(filter.clone()));

        let cells = Rc::new(Cells::default());

        // Icons
        let icon_sorted =
            gtk::SortListModel::new(Some(filtered.clone()), Some(views::sorter(Key::Name)));
        let grid_sel = gtk::MultiSelection::new(Some(icon_sorted));
        let grid = gtk::GridView::builder()
            .model(&grid_sel)
            .factory(&views::icons_factory(st.icon_size, &cells))
            .max_columns(40)
            .css_classes(["files-icons"])
            .build();
        let grid_scroll = scrolled(&grid);

        // List, with folders that expand in place.
        let child_filter = filter.clone();
        let tree = gtk::TreeListModel::new(filtered.clone(), false, false, move |obj| {
            let info = obj.downcast_ref::<gio::FileInfo>()?;
            if !entry::is_dir(info) || eapp::is_marked(info) {
                return None;
            }
            // The folder is set when the row is first expanded (views.rs).
            let children = gtk::DirectoryList::new(Some(entry::ATTRIBUTES), None::<&gio::File>);
            children.set_monitored(true);
            Some(gtk::FilterListModel::new(Some(children), Some(child_filter.clone())).upcast())
        });
        let list = gtk::ColumnView::builder()
            .reorderable(false)
            .show_row_separators(false)
            .css_classes(["files-list"])
            .build();
        let name_column = gtk::ColumnViewColumn::builder()
            .title(tr("Name"))
            .factory(&views::name_factory(st.list_icon_size, &cells))
            .sorter(&views::sorter(Key::Name))
            .expand(true)
            .resizable(true)
            .build();
        let column = |title: String, text: fn(&gio::FileInfo) -> String, key, width, xalign| {
            gtk::ColumnViewColumn::builder()
                .title(title)
                .factory(&views::text_factory(text, xalign, &cells))
                .sorter(&views::sorter(key))
                .fixed_width(width)
                .resizable(true)
                .build()
        };
        list.append_column(&name_column);
        list.append_column(&column(
            tr("Date Modified"),
            entry::modified_text,
            Key::Modified,
            200,
            0.0,
        ));
        list.append_column(&column(tr("Size"), entry::size_text, Key::Size, 90, 1.0));
        list.append_column(&column(tr("Kind"), entry::kind, Key::Kind, 170, 0.0));
        let list_sorted =
            gtk::SortListModel::new(Some(tree), Some(gtk::TreeListRowSorter::new(list.sorter())));
        let list_sel = gtk::MultiSelection::new(Some(list_sorted));
        list.set_model(Some(&list_sel));
        list.sort_by_column(Some(&name_column), gtk::SortType::Ascending);
        let list_scroll = scrolled(&list);

        let message = gtk::Label::builder()
            .wrap(true)
            .justify(gtk::Justification::Center)
            .css_classes(["files-placeholder"])
            .build();
        let stack = gtk::Stack::builder().hexpand(true).vexpand(true).build();
        stack.add_named(
            &rubberband::attach(&grid, &grid_scroll, &grid_sel, &cells),
            Some("icons"),
        );
        stack.add_named(
            &rubberband::attach(&list, &list_scroll, &list_sel, &cells),
            Some("list"),
        );
        stack.add_named(&message, Some("message"));

        let weak_open: Rc<RefCell<Option<std::rc::Weak<Window>>>> = Rc::default();
        let opener = weak_open.clone();
        let sidebar = Sidebar::new(
            Rc::new(move |file| {
                if let Some(w) = opener.borrow().as_ref().and_then(|w| w.upgrade()) {
                    w.go(file);
                }
            }),
            cells.clone(),
        );
        sidebar.widget.set_width_request(SIDEBAR_WIDTH);
        let columns = gtk::SizeGroup::new(gtk::SizeGroupMode::Horizontal);
        columns.add_widget(&band);
        columns.add_widget(&sidebar.widget);

        // Finder's Trash bar, with Empty.
        let trash_box = gtk::Box::builder()
            .spacing(8)
            .css_classes(["files-trash-bar"])
            .build();
        trash_box.append(
            &gtk::Label::builder()
                .label(tr("Trash"))
                .xalign(0.0)
                .hexpand(true)
                .css_classes(["files-dim"])
                .build(),
        );
        trash_box.append(
            &gtk::Button::builder()
                .label(tr("Empty"))
                .action_name("win.empty-trash")
                .css_classes(["files-trash-empty"])
                .build(),
        );
        let trash_bar = gtk::Revealer::builder().child(&trash_box).build();
        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .hexpand(true)
            .build();
        content.append(&trash_bar);
        content.append(&stack);

        let body = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        body.append(&sidebar.widget);
        body.append(&gtk::Separator::new(gtk::Orientation::Vertical));
        body.append(&content);
        window.set_child(Some(&body));

        let this = Rc::new(Self {
            window,
            sidebar,
            dir,
            filter,
            grid,
            grid_sel,
            grid_scroll,
            list,
            list_sel,
            list_scroll,
            name_column,
            stack,
            message,
            title,
            search_toggle,
            search,
            history: RefCell::new(History::new(folder.uri().to_string())),
            state: Cell::new(st),
            show_hidden,
            query,
            cells,
            jobs,
            content,
            trash_bar,
            undo: RefCell::default(),
            pending: RefCell::default(),
            menu: RefCell::default(),
            me: RefCell::default(),
        });
        this.me.replace(Rc::downgrade(&this));
        weak_open.replace(Some(Rc::downgrade(&this)));
        go_to.replace(Some(Rc::downgrade(&this)));
        let handler: std::rc::Weak<dyn CellHandler> = Rc::downgrade(&this) as _;
        this.cells.set_handler(handler);
        let weak = Rc::downgrade(&this);
        this.jobs.set_refocus(move || {
            if let Some(w) = weak.upgrade() {
                w.focus_view();
            }
        });
        this.connect();
        this.add_actions();
        this.connect_files();
        this.add_file_actions();
        this.load(&folder);
        this
    }

    fn connect(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.grid.connect_activate(move |_, pos| {
            if let Some(w) = weak.upgrade() {
                if let Some(info) = w.grid_sel.item(pos).and_then(|o| views::info_of(&o)) {
                    w.open(&info);
                }
            }
        });
        let weak = Rc::downgrade(self);
        self.list.connect_activate(move |_, pos| {
            if let Some(w) = weak.upgrade() {
                if let Some(info) = w.list_sel.item(pos).and_then(|o| views::info_of(&o)) {
                    w.open(&info);
                }
            }
        });
        let weak = Rc::downgrade(self);
        self.dir.connect_error_notify(move |dir| {
            let (Some(w), Some(err)) = (weak.upgrade(), dir.error()) else {
                return;
            };
            w.message.set_label(&format!(
                "{}\n{}",
                tr("This folder can't be opened."),
                err.message()
            ));
            w.stack.set_visible_child_name("message");
        });
        let weak = Rc::downgrade(self);
        self.search.connect_search_changed(move |entry| {
            if let Some(w) = weak.upgrade() {
                w.query.replace(entry.text().to_lowercase());
                w.filter.changed(gtk::FilterChange::Different);
            }
        });
        let toggle = self.search_toggle.clone();
        self.search
            .connect_stop_search(move |_| toggle.set_active(false));
        let weak = Rc::downgrade(self);
        self.search_toggle.connect_active_notify(move |t| {
            let Some(w) = weak.upgrade() else {
                return;
            };
            if t.is_active() {
                w.search.grab_focus();
            } else {
                w.search.set_text("");
                w.focus_view();
            }
        });
    }

    fn add_actions(self: &Rc<Self>) {
        let add = |name: &str, f: Handler| {
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
        add("back", Box::new(|w| w.step(true)));
        add("forward", Box::new(|w| w.step(false)));
        add(
            "up",
            Box::new(|w| {
                let current = gio::File::for_uri(w.history.borrow().current());
                if let Some(parent) = current.parent() {
                    w.go(parent);
                }
            }),
        );
        add(
            "home",
            Box::new(|w| w.go(gio::File::for_path(glib::home_dir()))),
        );
        add("close", Box::new(|w| w.window.close()));
        add("reload", Box::new(|w| w.reload()));
        add("new-folder", Box::new(|w| w.new_folder()));
        add("zoom-in", Box::new(|w| w.zoom(Some(true))));
        add("zoom-out", Box::new(|w| w.zoom(Some(false))));
        add("zoom-reset", Box::new(|w| w.zoom(None)));
        add(
            "search",
            Box::new(|w| {
                w.search_toggle.set_active(true);
                w.search.grab_focus();
            }),
        );

        let st = self.state.get();
        let view = gio::SimpleAction::new_stateful(
            "view",
            Some(glib::VariantTy::STRING),
            &st.mode.id().to_variant(),
        );
        let weak = Rc::downgrade(self);
        view.connect_change_state(move |action, value| {
            let (Some(w), Some(value)) = (weak.upgrade(), value) else {
                return;
            };
            let mode = match value.str() {
                Some("list") => Mode::List,
                _ => Mode::Icons,
            };
            action.set_state(value);
            w.update(|s| s.mode = mode);
            w.show_view();
        });
        self.window.add_action(&view);

        let hidden =
            gio::SimpleAction::new_stateful("show-hidden", None, &st.show_hidden.to_variant());
        let weak = Rc::downgrade(self);
        hidden.connect_activate(move |action, _| {
            let Some(w) = weak.upgrade() else {
                return;
            };
            let on = !action
                .state()
                .and_then(|v| v.get::<bool>())
                .unwrap_or(false);
            action.set_state(&on.to_variant());
            w.show_hidden.set(on);
            w.update(|s| s.show_hidden = on);
            w.filter.changed(gtk::FilterChange::Different);
        });
        self.window.add_action(&hidden);
    }

    fn rc(&self) -> Option<Rc<Self>> {
        self.me.borrow().upgrade()
    }

    fn update(&self, f: impl FnOnce(&mut State)) {
        let mut st = self.state.get();
        f(&mut st);
        self.state.set(st);
        state::write(&st);
    }

    /// Save the window size with the rest of the state, on close.
    pub fn remember(&self) {
        let (width, height) = (self.window.width(), self.window.height());
        if width > 0 && height > 0 {
            self.update(|s| {
                s.width = width;
                s.height = height;
            });
        }
    }

    /// Ctrl+plus, Ctrl+minus, Ctrl+0 (None): the icon size of the view in use.
    fn zoom(&self, larger: Option<bool>) {
        let st = self.state.get();
        match st.mode {
            Mode::Icons => {
                let size = match larger {
                    Some(true) => zoom::larger(&zoom::ICON_SIZES, st.icon_size),
                    Some(false) => zoom::smaller(&zoom::ICON_SIZES, st.icon_size),
                    None => zoom::DEFAULT_ICON,
                };
                if size != st.icon_size {
                    self.update(|s| s.icon_size = size);
                    self.grid
                        .set_factory(Some(&views::icons_factory(size, &self.cells)));
                }
            }
            Mode::List => {
                let size = match larger {
                    Some(true) => zoom::larger(&zoom::LIST_SIZES, st.list_icon_size),
                    Some(false) => zoom::smaller(&zoom::LIST_SIZES, st.list_icon_size),
                    None => zoom::DEFAULT_LIST,
                };
                if size != st.list_icon_size {
                    self.update(|s| s.list_icon_size = size);
                    self.name_column
                        .set_factory(Some(&views::name_factory(size, &self.cells)));
                }
            }
        }
    }

    fn focus_view(&self) {
        match self.state.get().mode {
            Mode::Icons => self.grid.grab_focus(),
            Mode::List => self.list.grab_focus(),
        };
    }

    fn show_view(&self) {
        self.stack
            .set_visible_child_name(self.state.get().mode.id());
        self.focus_view();
    }

    /// Open a folder in this window, adding it to Back.
    pub fn go(&self, file: gio::File) {
        let changed = self.history.borrow_mut().visit(file.uri().to_string());
        if changed {
            self.load(&file);
        }
    }

    fn step(&self, back: bool) {
        let uri = {
            let mut h = self.history.borrow_mut();
            let next = if back { h.back() } else { h.forward() };
            next.cloned()
        };
        if let Some(uri) = uri {
            self.load(&gio::File::for_uri(&uri));
        }
    }

    fn reload(&self) {
        let file = gio::File::for_uri(self.history.borrow().current());
        self.dir.set_file(None::<&gio::File>);
        self.load(&file);
    }

    fn load(&self, file: &gio::File) {
        log(&format!("showing {}", file.uri()));
        if self.search_toggle.is_active() {
            self.search_toggle.set_active(false);
        }
        let uri = file.uri().to_string();
        let in_trash = trash::is_trash_uri(&uri);
        if in_trash && !trash::gvfs() {
            // No GVfs: list the home trash itself.
            let files = trash::home().join("files");
            let _ = std::fs::create_dir_all(&files);
            self.dir.set_file(Some(&gio::File::for_path(files)));
        } else {
            self.dir.set_file(Some(file));
        }
        self.trash_bar.set_reveal_child(in_trash);
        let title = sidebar::place_title(&uri)
            .or_else(|| self.sidebar.mount_title(&uri))
            .unwrap_or_else(|| display_name(file));
        let icon = file
            .query_info(
                "standard::icon",
                gio::FileQueryInfoFlags::NONE,
                gio::Cancellable::NONE,
            )
            .ok()
            .and_then(|i| i.icon());
        self.title.set(&title, file, icon);
        self.window.set_title(Some(&title));
        self.sidebar.select_uri(&uri);
        let h = self.history.borrow();
        self.set_enabled("back", h.can_go_back());
        self.set_enabled("forward", h.can_go_forward());
        self.set_enabled("up", file.parent().is_some());
        drop(h);
        self.grid_scroll.vadjustment().set_value(0.0);
        self.list_scroll.vadjustment().set_value(0.0);
        self.show_view();
        self.update_actions();
    }

    fn set_enabled(&self, name: &str, on: bool) {
        if let Some(action) = self
            .window
            .lookup_action(name)
            .and_downcast::<gio::SimpleAction>()
        {
            action.set_enabled(on);
        }
    }

    /// Folders open in this window; anything else opens in its default
    /// application.
    fn open(&self, info: &gio::FileInfo) {
        let Some(target) = entry::target(info) else {
            return;
        };
        if eapp::is_marked(info) {
            if let Some(path) = target.path() {
                if let Err(why) = bundles::launch(&self.window, &path) {
                    gtk::AlertDialog::builder()
                        .message(format!(
                            "{} \u{201c}{}\u{201d}",
                            tr("Couldn't open"),
                            entry::name(info)
                        ))
                        .detail(why)
                        .build()
                        .show(Some(&self.window));
                }
            }
            return;
        }
        if entry::is_dir(info) {
            self.go(target);
            return;
        }
        let context = gtk::prelude::WidgetExt::display(&self.window).app_launch_context();
        let window = self.window.clone();
        let name = entry::name(info);
        gio::AppInfo::launch_default_for_uri_async(
            &target.uri(),
            Some(&context),
            gio::Cancellable::NONE,
            move |result| {
                if let Err(err) = result {
                    gtk::AlertDialog::builder()
                        .message(format!("{} \u{201c}{name}\u{201d}", tr("Couldn't open")))
                        .detail(err.message())
                        .build()
                        .show(Some(&window));
                }
            },
        );
    }

    fn new_folder(self: &Rc<Self>) {
        let Some(current) = self.dir.file() else {
            return;
        };
        let mut names = Vec::new();
        for i in 0..self.dir.n_items() {
            if let Some(info) = self.dir.item(i).and_downcast::<gio::FileInfo>() {
                names.push(info.name().to_string_lossy().into_owned());
            }
        }
        let folder = current.child(format::new_folder_name(&names));
        let weak = Rc::downgrade(self);
        let made = folder.clone();
        folder.make_directory_async(
            glib::Priority::DEFAULT,
            gio::Cancellable::NONE,
            move |result| {
                let Some(w) = weak.upgrade() else {
                    return;
                };
                match result {
                    Ok(()) => {
                        w.set_undo(fileops::Undo::Created(vec![made.clone()]));
                        // Finder names a new folder right away.
                        w.select_when_listed(&[made], true);
                    }
                    Err(err) => crate::dialogs::error(
                        &w.window,
                        &tr("Couldn't make a new folder here"),
                        err.message(),
                    ),
                }
            },
        );
    }
}
