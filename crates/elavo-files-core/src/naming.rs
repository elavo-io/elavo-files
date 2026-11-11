//! Finder's names for new items: `Report copy.pdf`, `Report.pdf alias`,
//! `Report 2.pdf` for Keep Both, `Archive.zip`, and the checks a new name
//! has to pass before a rename.

/// Two-part extensions that stay together, so `logs.tar.gz` duplicates as
/// `logs copy.tar.gz`.
const DOUBLE_EXTENSIONS: &[&str] = &[".tar.gz", ".tar.bz2", ".tar.xz", ".tar.zst"];

/// `("Report", ".pdf")` for `Report.pdf`. Folders, dot files such as
/// `.bashrc`, and names without a dot have no extension.
pub fn split(name: &str, is_dir: bool) -> (&str, &str) {
    if is_dir {
        return (name, "");
    }
    let lower = name.to_ascii_lowercase();
    for ext in DOUBLE_EXTENSIONS {
        if lower.len() > ext.len() && lower.ends_with(ext) {
            let i = name.len() - ext.len();
            return (&name[..i], &name[i..]);
        }
    }
    match name.rfind('.') {
        Some(i) if i > 0 && i + 1 < name.len() => (&name[..i], &name[i..]),
        _ => (name, ""),
    }
}

/// `("Report", Some(3))` for `Report 3`: a trailing number after a space.
fn trailing_number(stem: &str) -> (&str, Option<u32>) {
    if let Some((base, n)) = stem.rsplit_once(' ') {
        if !base.is_empty() && !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()) {
            if let Ok(n) = n.parse() {
                return (base, Some(n));
            }
        }
    }
    (stem, None)
}

/// `base + ext`, or the first free `base N + ext` from `start` on.
pub fn unique(base: &str, ext: &str, start: u32, exists: impl Fn(&str) -> bool) -> String {
    let first = format!("{base}{ext}");
    if !exists(&first) {
        return first;
    }
    (start.max(2)..)
        .map(|n| format!("{base} {n}{ext}"))
        .find(|c| !exists(c))
        .expect("a free name")
}

/// Duplicate: `Report copy.pdf`, then `Report copy 2.pdf`. Duplicating a
/// copy counts on from it rather than adding a second `copy`.
pub fn duplicate(name: &str, is_dir: bool, exists: impl Fn(&str) -> bool) -> String {
    let (stem, ext) = split(name, is_dir);
    let (numbered, _) = trailing_number(stem);
    let base = stem
        .strip_suffix(" copy")
        .or_else(|| numbered.strip_suffix(" copy"))
        .filter(|b| !b.is_empty())
        .unwrap_or(stem);
    unique(&format!("{base} copy"), ext, 2, exists)
}

/// Make Alias: `Report.pdf alias`, then `Report.pdf alias 2`.
pub fn alias(name: &str, exists: impl Fn(&str) -> bool) -> String {
    unique(&format!("{name} alias"), "", 2, exists)
}

/// Keep Both in a name conflict: `Report 2.pdf`, or `Report 4.pdf` when
/// the incoming item is already `Report 3.pdf`.
pub fn keep_both(name: &str, is_dir: bool, exists: impl Fn(&str) -> bool) -> String {
    let (stem, ext) = split(name, is_dir);
    let (base, n) = trailing_number(stem);
    let start = n.map_or(2, |n| n.saturating_add(1));
    (start.max(2)..)
        .map(|n| format!("{base} {n}{ext}"))
        .find(|c| !exists(c))
        .expect("a free name")
}

/// Compress: `Report.pdf.zip` for one item, `Archive.zip` for several.
pub fn archive(names: &[&str], exists: impl Fn(&str) -> bool) -> String {
    match names {
        [one] => unique(one, ".zip", 2, exists),
        _ => unique("Archive", ".zip", 2, exists),
    }
}

/// Why a name can't be used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Invalid {
    Empty,
    Slash,
    Dots,
    TooLong,
}

impl Invalid {
    pub fn message(self) -> &'static str {
        match self {
            Invalid::Empty => "A name can't be empty.",
            Invalid::Slash => "A name can't contain \u{201c}/\u{201d}.",
            Invalid::Dots => "\u{201c}.\u{201d} and \u{201c}..\u{201d} are reserved names.",
            Invalid::TooLong => "The name is too long.",
        }
    }
}

/// Whether `name` can be a file name on a Linux filesystem.
pub fn check(name: &str) -> Result<(), Invalid> {
    if name.is_empty() {
        Err(Invalid::Empty)
    } else if name.contains('/') || name.contains('\0') {
        Err(Invalid::Slash)
    } else if name == "." || name == ".." {
        Err(Invalid::Dots)
    } else if name.len() > 255 {
        Err(Invalid::TooLong)
    } else {
        Ok(())
    }
}

/// How many characters of `name` the rename field selects at first: the
/// name without its extension, as in Finder.
pub fn stem_chars(name: &str, is_dir: bool) -> usize {
    split(name, is_dir).0.chars().count()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn taken<'a>(names: &'a [&'a str]) -> impl Fn(&str) -> bool + 'a {
        move |n| names.contains(&n)
    }

    #[test]
    fn splits_extensions() {
        assert_eq!(split("Report.pdf", false), ("Report", ".pdf"));
        assert_eq!(split("logs.TAR.GZ", false), ("logs", ".TAR.GZ"));
        assert_eq!(split(".bashrc", false), (".bashrc", ""));
        assert_eq!(split("Makefile", false), ("Makefile", ""));
        assert_eq!(split("trailing.", false), ("trailing.", ""));
        assert_eq!(split("My.Folder", true), ("My.Folder", ""));
    }

    #[test]
    fn duplicates_like_finder() {
        assert_eq!(
            duplicate("Report.pdf", false, taken(&[])),
            "Report copy.pdf"
        );
        assert_eq!(
            duplicate("Report.pdf", false, taken(&["Report copy.pdf"])),
            "Report copy 2.pdf"
        );
        assert_eq!(
            duplicate(
                "Report copy.pdf",
                false,
                taken(&["Report copy.pdf", "Report copy 2.pdf"])
            ),
            "Report copy 3.pdf"
        );
        assert_eq!(
            duplicate("Report copy 2.pdf", false, taken(&["Report copy.pdf"])),
            "Report copy 2.pdf"
        );
        assert_eq!(duplicate("Photos", true, taken(&[])), "Photos copy");
        assert_eq!(duplicate("v1.2", true, taken(&[])), "v1.2 copy");
        assert_eq!(duplicate(".hidden", false, taken(&[])), ".hidden copy");
        assert_eq!(duplicate(" copy", false, taken(&[])), " copy copy");
    }

    #[test]
    fn aliases_archives_and_keep_both() {
        assert_eq!(alias("Report.pdf", taken(&[])), "Report.pdf alias");
        assert_eq!(
            alias("Report.pdf", taken(&["Report.pdf alias"])),
            "Report.pdf alias 2"
        );
        assert_eq!(keep_both("Report.pdf", false, taken(&[])), "Report 2.pdf");
        assert_eq!(
            keep_both("Report.pdf", false, taken(&["Report 2.pdf"])),
            "Report 3.pdf"
        );
        assert_eq!(keep_both("Report 3.pdf", false, taken(&[])), "Report 4.pdf");
        assert_eq!(keep_both("2024", true, taken(&[])), "2024 2");
        assert_eq!(archive(&["Report.pdf"], taken(&[])), "Report.pdf.zip");
        assert_eq!(
            archive(&["a", "b"], taken(&["Archive.zip"])),
            "Archive 2.zip"
        );
    }

    #[test]
    fn checks_names() {
        assert_eq!(check("ok name.txt"), Ok(()));
        assert_eq!(check(""), Err(Invalid::Empty));
        assert_eq!(check("a/b"), Err(Invalid::Slash));
        assert_eq!(check(".."), Err(Invalid::Dots));
        assert_eq!(check(&"x".repeat(256)), Err(Invalid::TooLong));
        assert_eq!(stem_chars("Résumé.pdf", false), 6);
        assert_eq!(stem_chars("Folder.app", true), 10);
    }
}
