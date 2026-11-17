//! The one Undo and Redo history of Files: every Files window and the
//! desktop (through [`crate::service`]) record what they do here, and Undo
//! or Redo from any of them takes the most recent operation, wherever it
//! happened. It lives as long as the Files process; Files keeps running
//! while there is something to undo or redo.
//!
//! Undo and Redo are off while any operation runs, in a window or for the
//! desktop, so the operation that is still running can't be passed over.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;

use gtk::gio;
use gtk::prelude::*;

use crate::i18n::tr;
use crate::window::fileops::{Redo, Undo};

/// How many operations Undo can go back.
pub const LIMIT: usize = 50;

/// What an operation was, for the menus: “Undo Move to Trash”.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Copy,
    Move,
    Trash,
    Rename,
    NewFolder,
    Duplicate,
    Alias,
    Compress,
    Extract,
    PutBack,
}

impl Action {
    pub fn label(self) -> String {
        tr(match self {
            Action::Copy => "Copy",
            Action::Move => "Move",
            Action::Trash => "Move to Trash",
            Action::Rename => "Rename",
            Action::NewFolder => "New Folder",
            Action::Duplicate => "Duplicate",
            Action::Alias => "Make Alias",
            Action::Compress => "Compress",
            Action::Extract => "Extract",
            Action::PutBack => "Put Back",
        })
    }
}

/// The two stacks, apart from what they hold.
pub struct Stacks<U, R> {
    undo: VecDeque<(Action, U)>,
    redo: Vec<(Action, R)>,
    limit: usize,
}

impl<U, R> Stacks<U, R> {
    pub fn new(limit: usize) -> Self {
        Self {
            undo: VecDeque::new(),
            redo: Vec::new(),
            limit,
        }
    }

    /// A new operation: what was undone before can't be redone.
    pub fn record(&mut self, action: Action, undo: U) {
        self.redo.clear();
        self.push_undo(action, undo);
    }

    /// Redo did an operation again: the rest stays redoable.
    pub fn redone(&mut self, action: Action, undo: U) {
        self.push_undo(action, undo);
    }

    fn push_undo(&mut self, action: Action, undo: U) {
        self.undo.push_back((action, undo));
        while self.undo.len() > self.limit {
            self.undo.pop_front();
        }
    }

    pub fn undone(&mut self, action: Action, redo: R) {
        self.redo.push((action, redo));
    }

    pub fn take_undo(&mut self) -> Option<(Action, U)> {
        self.undo.pop_back()
    }

    pub fn take_redo(&mut self) -> Option<(Action, R)> {
        self.redo.pop()
    }

    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
    }

    pub fn next_undo(&self) -> Option<Action> {
        self.undo.back().map(|(a, _)| *a)
    }

    pub fn next_redo(&self) -> Option<Action> {
        self.redo.last().map(|(a, _)| *a)
    }

    pub fn is_empty(&self) -> bool {
        self.undo.is_empty() && self.redo.is_empty()
    }
}

/// Told when what Undo and Redo can do changes. Returns false once its
/// owner is gone, and is then dropped.
type Watcher = Box<dyn Fn() -> bool>;

thread_local! {
    static STACKS: RefCell<Stacks<Undo, Redo>> = RefCell::new(Stacks::new(LIMIT));
    /// Operations running, in windows or for the desktop, and Undos or
    /// Redos under way.
    static BUSY: Cell<u32> = const { Cell::new(0) };
    static WATCHERS: RefCell<Vec<Watcher>> = const { RefCell::new(Vec::new()) };
    static HOLD: RefCell<Option<gio::ApplicationHoldGuard>> = const { RefCell::new(None) };
}

/// Held while an operation, an Undo or a Redo runs. Undo and Redo come
/// back on when the last one is dropped.
pub struct Busy(());

impl Busy {
    pub fn new() -> Self {
        BUSY.with(|b| b.set(b.get() + 1));
        changed();
        Busy(())
    }
}

impl Drop for Busy {
    fn drop(&mut self) {
        if BUSY.try_with(|b| b.set(b.get().saturating_sub(1))).is_ok() {
            changed();
        }
    }
}

fn busy() -> bool {
    BUSY.with(|b| b.get() > 0)
}

pub fn record(action: Action, undo: Undo) {
    STACKS.with(|s| s.borrow_mut().record(action, undo));
    changed();
}

pub fn redone(action: Action, undo: Undo) {
    STACKS.with(|s| s.borrow_mut().redone(action, undo));
    changed();
}

pub fn undone(action: Action, redo: Redo) {
    STACKS.with(|s| s.borrow_mut().undone(action, redo));
    changed();
}

/// Forget everything, after something that can't be undone.
pub fn clear() {
    STACKS.with(|s| s.borrow_mut().clear());
    changed();
}

/// The most recent operation, to undo, with Undo and Redo off until the
/// guard goes.
pub fn take_undo() -> Option<(Action, Undo, Busy)> {
    if busy() {
        return None;
    }
    let (action, undo) = STACKS.with(|s| s.borrow_mut().take_undo())?;
    Some((action, undo, Busy::new()))
}

pub fn take_redo() -> Option<(Action, Redo, Busy)> {
    if busy() {
        return None;
    }
    let (action, redo) = STACKS.with(|s| s.borrow_mut().take_redo())?;
    Some((action, redo, Busy::new()))
}

pub fn can_undo() -> bool {
    !busy() && STACKS.with(|s| s.borrow().next_undo().is_some())
}

pub fn can_redo() -> bool {
    !busy() && STACKS.with(|s| s.borrow().next_redo().is_some())
}

/// “Undo Move to Trash”, or “Undo” when there is nothing to undo.
pub fn undo_label() -> String {
    label(tr("Undo"), STACKS.with(|s| s.borrow().next_undo()))
}

pub fn redo_label() -> String {
    label(tr("Redo"), STACKS.with(|s| s.borrow().next_redo()))
}

fn label(verb: String, action: Option<Action>) -> String {
    match action {
        Some(action) => format!("{verb} {}", action.label()),
        None => verb,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Undo,
    Redo,
}

/// A menu section of Undo and Redo items whose labels name the operation
/// they act on, kept up to date for as long as the section exists.
pub fn labelled(items: &[(Step, &'static str, Option<&'static str>)]) -> gio::Menu {
    let section = gio::Menu::new();
    let items = items.to_vec();
    let fill = move |section: &gio::Menu| {
        let labels: Vec<String> = items
            .iter()
            .map(|(step, _, _)| match step {
                Step::Undo => undo_label(),
                Step::Redo => redo_label(),
            })
            .collect();
        // Only when a label changes, so an open menu isn't rebuilt for
        // nothing.
        let same = section.n_items() as usize == labels.len()
            && labels.iter().enumerate().all(|(i, l)| {
                section
                    .item_attribute_value(i as i32, "label", None)
                    .and_then(|v| v.get::<String>())
                    .is_some_and(|old| old == *l)
            });
        if same {
            return;
        }
        section.remove_all();
        for ((_, action, accel), label) in items.iter().zip(labels) {
            let item = gio::MenuItem::new(Some(&label), Some(action));
            if let Some(accel) = accel {
                item.set_attribute_value("accel", Some(&accel.to_variant()));
            }
            section.append_item(&item);
        }
    };
    fill(&section);
    let weak = section.downgrade();
    watch(move || match weak.upgrade() {
        Some(section) => {
            fill(&section);
            true
        }
        None => false,
    });
    section
}

/// Call `f` whenever Undo or Redo changes; it returns false to stop.
pub fn watch(f: impl Fn() -> bool + 'static) {
    WATCHERS.with(|w| w.borrow_mut().push(Box::new(f)));
}

fn changed() {
    // Also reached from a guard dropped as Files exits, when these may be
    // gone already.
    let Ok(empty) = STACKS.try_with(|s| s.borrow().is_empty()) else {
        return;
    };
    // Files stays running while there is something to undo or redo, so the
    // desktop can still undo what a window that has since closed did.
    let _ = HOLD.try_with(|h| {
        let mut hold = h.borrow_mut();
        if empty {
            *hold = None;
        } else if hold.is_none() {
            *hold = gio::Application::default().map(|a| a.hold());
        }
    });
    // A watcher may ask for the labels, or add a watcher of its own.
    let Ok(watchers) = WATCHERS.try_with(|w| std::mem::take(&mut *w.borrow_mut())) else {
        return;
    };
    let kept: Vec<Watcher> = watchers.into_iter().filter(|f| f()).collect();
    let _ = WATCHERS.try_with(|w| {
        let mut w = w.borrow_mut();
        let added = std::mem::take(&mut *w);
        *w = kept;
        w.extend(added);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn undo_takes_the_most_recent_and_redo_follows() {
        let mut s: Stacks<u32, u32> = Stacks::new(LIMIT);
        s.record(Action::Trash, 1);
        s.record(Action::Rename, 2);
        assert_eq!(s.next_undo(), Some(Action::Rename));
        assert_eq!(s.take_undo(), Some((Action::Rename, 2)));
        s.undone(Action::Rename, 20);
        assert_eq!(s.take_undo(), Some((Action::Trash, 1)));
        s.undone(Action::Trash, 10);
        // Redo goes forward again in order, and keeps the rest.
        assert_eq!(s.take_redo(), Some((Action::Trash, 10)));
        s.redone(Action::Trash, 1);
        assert_eq!(s.next_redo(), Some(Action::Rename));
        assert_eq!(s.take_redo(), Some((Action::Rename, 20)));
        s.redone(Action::Rename, 2);
        assert_eq!(s.next_redo(), None);
        assert_eq!(s.next_undo(), Some(Action::Rename));
    }

    #[test]
    fn a_new_operation_ends_redo() {
        let mut s: Stacks<u32, u32> = Stacks::new(LIMIT);
        s.record(Action::Copy, 1);
        let (a, _) = s.take_undo().unwrap();
        s.undone(a, 1);
        assert_eq!(s.next_redo(), Some(Action::Copy));
        s.record(Action::NewFolder, 2);
        assert_eq!(s.next_redo(), None);
        assert_eq!(s.next_undo(), Some(Action::NewFolder));
    }

    #[test]
    fn history_keeps_the_last_limit() {
        let mut s: Stacks<usize, usize> = Stacks::new(3);
        for i in 0..5 {
            s.record(Action::Move, i);
        }
        let mut left = Vec::new();
        while let Some((_, i)) = s.take_undo() {
            left.push(i);
        }
        assert_eq!(left, [4, 3, 2]);
        assert!(s.is_empty());
    }

    #[test]
    fn clear_forgets_both_ways() {
        let mut s: Stacks<u32, u32> = Stacks::new(LIMIT);
        s.record(Action::Move, 1);
        s.undone(Action::Copy, 2);
        s.clear();
        assert!(s.is_empty());
    }

    #[test]
    fn labels_name_the_action() {
        assert_eq!(
            label("Undo".into(), Some(Action::Trash)),
            "Undo Move to Trash"
        );
        assert_eq!(label("Redo".into(), Some(Action::Rename)), "Redo Rename");
        assert_eq!(label("Undo".into(), None), "Undo");
    }

    #[test]
    fn shared_history_is_off_while_busy() {
        record(Action::Copy, Undo::Created(Vec::new()));
        assert!(can_undo());
        let guard = Busy::new();
        assert!(!can_undo());
        assert!(take_undo().is_none());
        drop(guard);
        let (action, _, _busy) = take_undo().unwrap();
        assert_eq!(action, Action::Copy);
        assert!(!can_undo());
        clear();
    }
}
