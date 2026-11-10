//! Ordering the way Finder does: names compare without case and with
//! numbers by value, so `file2` comes before `file10`.

use std::cmp::Ordering;

/// What a view is sorted by.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Key {
    #[default]
    Name,
    Modified,
    Size,
    Kind,
}

enum Chunk<'a> {
    Digits(&'a str),
    Text(&'a str),
}

fn chunks(s: &str) -> impl Iterator<Item = Chunk<'_>> {
    let mut rest = s;
    std::iter::from_fn(move || {
        let first = rest.chars().next()?;
        let digit = first.is_ascii_digit();
        let end = rest
            .char_indices()
            .find(|(_, c)| c.is_ascii_digit() != digit)
            .map_or(rest.len(), |(i, _)| i);
        let (head, tail) = rest.split_at(end);
        rest = tail;
        Some(if digit {
            Chunk::Digits(head)
        } else {
            Chunk::Text(head)
        })
    })
}

fn compare_digits(a: &str, b: &str) -> Ordering {
    let a_trim = a.trim_start_matches('0');
    let b_trim = b.trim_start_matches('0');
    a_trim
        .len()
        .cmp(&b_trim.len())
        .then_with(|| a_trim.cmp(b_trim))
        // `07` after `7`, so equal values still have a stable order.
        .then_with(|| a.len().cmp(&b.len()))
}

/// Natural, case-insensitive name order. Falls back to the exact bytes so
/// two different names never compare equal.
pub fn names(a: &str, b: &str) -> Ordering {
    let la = a.to_lowercase();
    let lb = b.to_lowercase();
    let mut ia = chunks(&la);
    let mut ib = chunks(&lb);
    loop {
        let order = match (ia.next(), ib.next()) {
            (None, None) => break,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(Chunk::Digits(x)), Some(Chunk::Digits(y))) => compare_digits(x, y),
            (Some(Chunk::Text(x)), Some(Chunk::Text(y))) => x.cmp(y),
            (Some(Chunk::Digits(_)), Some(Chunk::Text(_))) => Ordering::Less,
            (Some(Chunk::Text(_)), Some(Chunk::Digits(_))) => Ordering::Greater,
        };
        if order != Ordering::Equal {
            return order;
        }
    }
    a.cmp(b)
}

/// One row's sort fields.
#[derive(Clone, Copy, Debug)]
pub struct Fields<'a> {
    pub name: &'a str,
    pub is_dir: bool,
    pub size: u64,
    pub modified: i64,
    pub kind: &'a str,
}

/// Ascending order by `key`. Ties fall back to the name. Folders have no
/// size, so by size they come before every file, as in Finder.
pub fn compare(key: Key, a: &Fields<'_>, b: &Fields<'_>) -> Ordering {
    let primary = match key {
        Key::Name => Ordering::Equal,
        Key::Modified => a.modified.cmp(&b.modified),
        Key::Size => match (a.is_dir, b.is_dir) {
            (true, true) => Ordering::Equal,
            (true, false) => Ordering::Less,
            (false, true) => Ordering::Greater,
            (false, false) => a.size.cmp(&b.size),
        },
        Key::Kind => names(a.kind, b.kind),
    };
    primary.then_with(|| names(a.name, b.name))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sorted(mut v: Vec<&str>) -> Vec<&str> {
        v.sort_by(|a, b| names(a, b));
        v
    }

    #[test]
    fn numbers_sort_by_value() {
        assert_eq!(
            sorted(vec!["file10", "file2", "file1", "File3"]),
            vec!["file1", "file2", "File3", "file10"]
        );
        assert_eq!(
            sorted(vec!["IMG_0010.jpg", "IMG_9.jpg", "IMG_0100.jpg"]),
            vec!["IMG_9.jpg", "IMG_0010.jpg", "IMG_0100.jpg"]
        );
    }

    #[test]
    fn case_is_ignored_but_order_is_total() {
        assert_eq!(names("apple", "Banana"), Ordering::Less);
        assert_eq!(names("Zed", "alpha"), Ordering::Greater);
        assert_ne!(names("Readme", "readme"), Ordering::Equal);
        assert_eq!(names("a", "a"), Ordering::Equal);
        assert_eq!(names("a", "ab"), Ordering::Less);
        assert_eq!(names("7", "07"), Ordering::Less);
    }

    fn f<'a>(name: &'a str, is_dir: bool, size: u64, modified: i64, kind: &'a str) -> Fields<'a> {
        Fields {
            name,
            is_dir,
            size,
            modified,
            kind,
        }
    }

    #[test]
    fn keys() {
        let dir = f("Projects", true, 4096, 50, "Folder");
        let small = f("notes.txt", false, 10, 100, "Plain text document");
        let big = f("movie.mp4", false, 9_000, 10, "MPEG-4 video");
        assert_eq!(compare(Key::Size, &dir, &small), Ordering::Less);
        assert_eq!(compare(Key::Size, &small, &big), Ordering::Less);
        assert_eq!(compare(Key::Modified, &big, &dir), Ordering::Less);
        assert_eq!(compare(Key::Kind, &dir, &big), Ordering::Less);
        assert_eq!(compare(Key::Name, &big, &small), Ordering::Less);
        let twin = f("a", false, 10, 100, "x");
        let twin2 = f("b", false, 10, 100, "x");
        assert_eq!(compare(Key::Size, &twin, &twin2), Ordering::Less);
    }
}
