//! Type-to-select, as in Finder: typing the start of a name in a folder
//! selects the first item whose name begins with it.

use std::cmp::Ordering;
use std::time::Duration;

use crate::sort;

/// A pause this long starts a new name.
pub const RESET: Duration = Duration::from_millis(1000);

/// The item `query` selects among `names`, in the order shown: the first
/// whose name starts with it, ignoring case. With none, the one sharing
/// the longest start with it (the first shown on a tie), so "Graphics"
/// finds "GitHub". When no name shares even a first letter, the name that
/// would come next in name order, or the last name when every one comes
/// before it.
pub fn index<S: AsRef<str>>(names: &[S], query: &str) -> Option<usize> {
    if names.is_empty() || query.is_empty() {
        return None;
    }
    let q = query.to_lowercase();
    if let Some(i) = names
        .iter()
        .position(|n| n.as_ref().to_lowercase().starts_with(&q))
    {
        return Some(i);
    }
    let mut best = (0, 0);
    for (i, n) in names.iter().enumerate() {
        let shared = common_prefix(&n.as_ref().to_lowercase(), &q);
        if shared > best.1 {
            best = (i, shared);
        }
    }
    if best.1 > 0 {
        return Some(best.0);
    }
    let by_name = |a: &(usize, &S), b: &(usize, &S)| sort::names(a.1.as_ref(), b.1.as_ref());
    names
        .iter()
        .enumerate()
        .filter(|(_, n)| sort::names(n.as_ref(), query) == Ordering::Greater)
        .min_by(by_name)
        .or_else(|| names.iter().enumerate().max_by(by_name))
        .map(|(i, _)| i)
}

/// How many characters `a` and `b` start with in common.
fn common_prefix(a: &str, b: &str) -> usize {
    a.chars().zip(b.chars()).take_while(|(x, y)| x == y).count()
}

#[cfg(test)]
mod tests {
    use super::*;

    const NAMES: &[&str] = &[
        "Applications",
        "Archive",
        "build",
        "Desktop",
        "file 2",
        "file 10",
        "notes.txt",
    ];

    #[test]
    fn prefix_without_case() {
        assert_eq!(index(NAMES, "a"), Some(0));
        assert_eq!(index(NAMES, "Ar"), Some(1));
        assert_eq!(index(NAMES, "ARC"), Some(1));
        assert_eq!(index(NAMES, "d"), Some(3));
        assert_eq!(index(NAMES, "file 1"), Some(5));
        assert_eq!(index(NAMES, "file "), Some(4));
    }

    #[test]
    fn no_match_goes_to_the_longest_shared_start() {
        let docs = ["Desktop", "GitHub", "identify-power-button.py", "Notes"];
        assert_eq!(index(&docs, "Graphics"), Some(1));
        assert_eq!(index(&docs, "gRAPHICS"), Some(1));
        // "Ar" is shared by both A items: the first shown wins.
        assert_eq!(index(NAMES, "Arz"), Some(1));
        assert_eq!(index(NAMES, "Apz"), Some(0));
        assert_eq!(index(NAMES, "az"), Some(0));
        // "file " is shared by both; the first shown wins.
        assert_eq!(index(NAMES, "file 3"), Some(4));
        assert_eq!(index(NAMES, "notes.md"), Some(6));
    }

    #[test]
    fn nothing_shared_goes_to_the_next_name() {
        // Between "build" and "Desktop".
        assert_eq!(index(NAMES, "c"), Some(3));
        assert_eq!(index(NAMES, "cat"), Some(3));
        // After every name: the last.
        assert_eq!(index(NAMES, "zz"), Some(6));
    }

    #[test]
    fn next_name_ignores_the_order_shown() {
        // Sorted by date, say: the next name is still found by name.
        let shown = ["notes.txt", "Desktop", "build", "Archive"];
        assert_eq!(index(&shown, "c"), Some(1));
        assert_eq!(index(&shown, "zz"), Some(0));
        assert_eq!(index(&shown, "b"), Some(2));
    }

    #[test]
    fn nothing_to_select() {
        assert_eq!(index::<&str>(&[], "a"), None);
        assert_eq!(index(NAMES, ""), None);
    }
}
