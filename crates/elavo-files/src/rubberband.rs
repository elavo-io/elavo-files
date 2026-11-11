//! Selecting by dragging a rectangle over the files, as in Finder.
//!
//! The rectangle starts on any empty space, between and below items too,
//! and is a translucent accent tint with a thin border. The selection follows
//! it while dragging. Near the top or bottom edge the view scrolls. Holding
//! Ctrl or Shift adds to what was already selected.
//!
//! GTK's own rubber band is off: it only reacts to whole rows and columns
//! and updates in steps. Items are hit-tested against the cells on screen;
//! their places are kept in content coordinates, so the ones scrolled away
//! during the drag still count.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeSet, HashMap};
use std::rc::Rc;

use elavo_files_core::entry;
use gtk::prelude::*;
use gtk::{gdk, glib, graphene};

use crate::views::{self, Cells};

/// Movement before a press on empty space becomes a rectangle.
const THRESHOLD: f64 = 4.0;
/// How near an edge the pointer starts the view scrolling, and how fast it
/// can go, in pixels per frame.
const EDGE: f64 = 36.0;
const MAX_STEP: f64 = 22.0;

struct Band {
    view: gtk::Widget,
    scroll: gtk::ScrolledWindow,
    sel: gtk::MultiSelection,
    cells: Rc<Cells>,
    layer: gtk::Fixed,
    rect: gtk::Box,
    active: Cell<bool>,
    /// Where the press was: x in the view, y in the content.
    anchor: Cell<(f64, f64)>,
    /// The pointer now, in the view.
    pointer: Cell<(f64, f64)>,
    base: RefCell<gtk::Bitset>,
    positions: RefCell<HashMap<String, u32>>,
    /// Item places in content coordinates, as seen during the drag.
    places: RefCell<HashMap<u32, graphene::Rect>>,
    hits: RefCell<BTreeSet<u32>>,
    tick: RefCell<Option<gtk::TickCallbackId>>,
}

/// Give `view` (inside `scroll`) the rubber band. Returns what goes in the
/// view's place: `scroll` with the rectangle drawn over it.
pub fn attach(
    view: &impl IsA<gtk::Widget>,
    scroll: &gtk::ScrolledWindow,
    sel: &gtk::MultiSelection,
    cells: &Rc<Cells>,
) -> gtk::Overlay {
    let rect = gtk::Box::builder()
        .css_classes(["files-band"])
        .can_target(false)
        .visible(false)
        .build();
    let layer = gtk::Fixed::builder().can_target(false).build();
    layer.put(&rect, 0.0, 0.0);
    let overlay = gtk::Overlay::builder().child(scroll).build();
    overlay.add_overlay(&layer);
    overlay.set_clip_overlay(&layer, true);

    let band = Rc::new(Band {
        view: view.clone().upcast(),
        scroll: scroll.clone(),
        sel: sel.clone(),
        cells: cells.clone(),
        layer,
        rect,
        active: Cell::new(false),
        anchor: Cell::new((0.0, 0.0)),
        pointer: Cell::new((0.0, 0.0)),
        base: RefCell::new(gtk::Bitset::new_empty()),
        positions: RefCell::default(),
        places: RefCell::default(),
        hits: RefCell::default(),
        tick: RefCell::default(),
    });

    let drag = gtk::GestureDrag::builder()
        .button(gdk::BUTTON_PRIMARY)
        .build();
    let b = band.clone();
    drag.connect_drag_begin(move |g, x, y| {
        if !b.on_empty_space(x, y) {
            g.set_state(gtk::EventSequenceState::Denied);
            return;
        }
        let adds = g
            .current_event_state()
            .intersects(gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::SHIFT_MASK);
        b.anchor.set((x, y + b.offset()));
        b.pointer.set((x, y));
        b.base.replace(if adds {
            b.sel.selection()
        } else {
            gtk::Bitset::new_empty()
        });
    });
    let b = band.clone();
    drag.connect_drag_update(move |g, dx, dy| {
        let Some((sx, sy)) = g.start_point() else {
            return;
        };
        if !b.active.get() {
            if dx.hypot(dy) < THRESHOLD {
                return;
            }
            g.set_state(gtk::EventSequenceState::Claimed);
            b.start();
        }
        b.pointer.set((sx + dx, sy + dy));
        b.update();
    });
    let b = band.clone();
    drag.connect_drag_end(move |_, _, _| b.finish());
    let b = band.clone();
    drag.connect_cancel(move |_, _| b.finish());
    band.view.add_controller(drag);
    overlay
}

impl Band {
    fn offset(&self) -> f64 {
        self.scroll.vadjustment().value()
    }

    /// Empty space: not an item (in the list, not its icon or name, as
    /// the rest of a row starts a rectangle too) and not the column titles.
    fn on_empty_space(&self, x: f64, y: f64) -> bool {
        let item = if self.view.is::<gtk::ColumnView>() {
            views::GRAB
        } else {
            "files-cell"
        };
        let mut w = self.view.pick(x, y, gtk::PickFlags::DEFAULT);
        while let Some(x) = w {
            if x == self.view {
                return true;
            }
            if x.has_css_class(item) || x.css_name() == "header" {
                return false;
            }
            w = x.parent();
        }
        true
    }

    fn start(self: &Rc<Self>) {
        self.active.set(true);
        self.places.borrow_mut().clear();
        self.hits.borrow_mut().clear();
        // Where each file is in the model, by uri, as the cells know them.
        let mut positions = HashMap::new();
        for i in 0..self.sel.n_items() {
            let file = self
                .sel
                .item(i)
                .and_then(|o| views::info_of(&o))
                .and_then(|info| entry::file(&info));
            if let Some(file) = file {
                positions.insert(file.uri().to_string(), i);
            }
        }
        self.positions.replace(positions);
        self.rect.set_visible(true);
        let weak = Rc::downgrade(self);
        let id = self
            .view
            .add_tick_callback(move |_, _| match weak.upgrade() {
                Some(b) => {
                    b.autoscroll();
                    glib::ControlFlow::Continue
                }
                None => glib::ControlFlow::Break,
            });
        self.tick.replace(Some(id));
    }

    fn finish(&self) {
        if let Some(id) = self.tick.take() {
            id.remove();
        }
        if self.active.replace(false) {
            self.rect.set_visible(false);
            self.positions.borrow_mut().clear();
            self.places.borrow_mut().clear();
        }
    }

    fn autoscroll(&self) {
        let (_, y) = self.pointer.get();
        let height = f64::from(self.view.height());
        // Faster the nearer the edge, and faster still past it.
        let step = if y < EDGE {
            -((EDGE - y) / EDGE).min(3.0) * MAX_STEP / 3.0
        } else if y > height - EDGE {
            ((y - (height - EDGE)) / EDGE).min(3.0) * MAX_STEP / 3.0
        } else {
            return;
        };
        let adj = self.scroll.vadjustment();
        let top = adj.upper() - adj.page_size();
        let value = (adj.value() + step.clamp(-MAX_STEP, MAX_STEP)).clamp(adj.lower(), top);
        if (value - adj.value()).abs() > f64::EPSILON {
            adj.set_value(value);
            self.update();
        }
    }

    /// Redraw the rectangle and select what it touches.
    fn update(&self) {
        let offset = self.offset();
        let (ax, ay) = self.anchor.get();
        let (px, py) = self.pointer.get();
        let width = f64::from(self.view.width());
        let px = px.clamp(0.0, width - 1.0);
        let (left, right) = (ax.min(px), ax.max(px));
        let (top, bottom) = (ay.min(py + offset), ay.max(py + offset));

        // Draw it where the view shows that part of the content.
        if let Some(at) = self.view.compute_point(
            &self.layer,
            &graphene::Point::new(left as f32, (top - offset) as f32),
        ) {
            self.layer
                .move_(&self.rect, f64::from(at.x()), f64::from(at.y()));
        }
        self.rect
            .set_size_request((right - left).round() as i32, (bottom - top).round() as i32);

        self.learn_places(offset);
        let band = graphene::Rect::new(
            left as f32,
            top as f32,
            (right - left).max(1.0) as f32,
            (bottom - top).max(1.0) as f32,
        );
        let hits: BTreeSet<u32> = self
            .places
            .borrow()
            .iter()
            .filter(|(_, r)| r.intersection(&band).is_some())
            .map(|(&i, _)| i)
            .collect();
        if *self.hits.borrow() == hits {
            return;
        }
        let selected = self.base.borrow().copy();
        for &i in &hits {
            selected.add(i);
        }
        let n = self.sel.n_items();
        self.sel
            .set_selection(&selected, &gtk::Bitset::new_range(0, n));
        self.hits.replace(hits);
    }

    /// Note where the cells on screen are, in content coordinates.
    fn learn_places(&self, offset: f64) {
        let positions = self.positions.borrow();
        let mut places = self.places.borrow_mut();
        // In the list a row is the whole width, as in Finder; in icons an
        // item is its icon and name.
        let rows = self.view.is::<gtk::ColumnView>();
        let width = self.view.width() as f32;
        for (uri, widget) in self.cells.mapped() {
            if !widget.is_ancestor(&self.view) {
                continue;
            }
            let (Some(&i), Some(b)) = (positions.get(&uri), widget.compute_bounds(&self.view))
            else {
                continue;
            };
            let y = b.y() + offset as f32;
            let r = if rows {
                graphene::Rect::new(0.0, y, width, b.height())
            } else {
                graphene::Rect::new(b.x(), y, b.width(), b.height())
            };
            places.insert(i, r);
        }
    }
}
