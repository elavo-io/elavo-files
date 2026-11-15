//! Extract Here: which archives Files can open, what their contents are
//! called once out, and the program that unpacks each with the tools an
//! Elavo system has (`tar` with gzip, xz, bzip2 and zstd, `unzip`, `7z`,
//! and `bsdtar` where it is installed).
//!
//! As Archive Utility does, an archive holding one item puts that item
//! beside it; one holding several puts them in a folder named after the
//! archive. A name already taken gets a number (`Photos 2`).

use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    /// A tar archive, compressed or not; GNU tar finds the compression.
    Tar,
    Zip,
    /// 7-Zip and RAR, through `7z`.
    SevenZip,
    /// One compressed file, not an archive: `notes.txt.gz`. The program
    /// decompresses to standard output.
    Single(&'static str),
}

/// Suffixes, longest first so `.tar.gz` wins over `.gz`.
const SUFFIXES: &[(&str, Format)] = &[
    (".tar.gz", Format::Tar),
    (".tar.xz", Format::Tar),
    (".tar.bz2", Format::Tar),
    (".tar.zst", Format::Tar),
    (".tar.lz", Format::Tar),
    (".tar.lzma", Format::Tar),
    (".tgz", Format::Tar),
    (".txz", Format::Tar),
    (".tbz", Format::Tar),
    (".tbz2", Format::Tar),
    (".tzst", Format::Tar),
    (".tar", Format::Tar),
    (".zip", Format::Zip),
    (".7z", Format::SevenZip),
    (".rar", Format::SevenZip),
    (".gz", Format::Single("gzip")),
    (".xz", Format::Single("xz")),
    (".bz2", Format::Single("bzip2")),
    (".zst", Format::Single("zstd")),
];

/// The format of `name` and the name without its archive suffix.
pub fn detect(name: &str) -> Option<(Format, &str)> {
    let lower = name.to_ascii_lowercase();
    SUFFIXES.iter().find_map(|(suffix, format)| {
        (lower.len() > suffix.len() && lower.ends_with(suffix))
            .then(|| (*format, &name[..name.len() - suffix.len()]))
    })
}

pub fn is_archive(name: &str) -> bool {
    detect(name).is_some()
}

/// Whether one of the programs for `format` is installed, given a lookup
/// such as `glib::find_program_in_path`.
pub fn can_extract(format: Format, has: &dyn Fn(&str) -> bool) -> bool {
    command(format, Path::new("a"), Path::new("b"), has).is_some()
}

/// The program and its arguments that unpack `archive` into the folder
/// `into` (for [`Format::Single`], what to run with standard output going
/// to the new file). None when nothing that reads it is installed.
pub fn command(
    format: Format,
    archive: &Path,
    into: &Path,
    has: &dyn Fn(&str) -> bool,
) -> Option<(String, Vec<String>)> {
    let a = archive.to_string_lossy().into_owned();
    let d = into.to_string_lossy().into_owned();
    let s = |v: &[&str]| v.iter().map(|x| (*x).to_owned()).collect::<Vec<_>>();
    let seven = || {
        has("7z").then(|| {
            (
                "7z".to_owned(),
                s(&[
                    "x",
                    "-y",
                    "-bd",
                    "-bso0",
                    "-bsp0",
                    &format!("-o{d}"),
                    "--",
                    &a,
                ]),
            )
        })
    };
    let bsdtar = || has("bsdtar").then(|| ("bsdtar".to_owned(), s(&["-x", "-f", &a, "-C", &d])));
    match format {
        Format::Tar => has("tar")
            .then(|| {
                (
                    "tar".to_owned(),
                    s(&["-x", "--no-same-owner", "-f", &a, "-C", &d]),
                )
            })
            .or_else(bsdtar)
            .or_else(seven),
        Format::Zip => has("unzip")
            .then(|| ("unzip".to_owned(), s(&["-q", "-o", &a, "-d", &d])))
            .or_else(seven)
            .or_else(bsdtar),
        Format::SevenZip => seven().or_else(bsdtar),
        Format::Single(tool) => has(tool).then(|| (tool.to_owned(), s(&["-d", "-c", "--", &a]))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_formats_and_stems() {
        assert_eq!(detect("logs.tar.gz"), Some((Format::Tar, "logs")));
        assert_eq!(detect("Logs.TGZ"), Some((Format::Tar, "Logs")));
        assert_eq!(detect("src.tar.zst"), Some((Format::Tar, "src")));
        assert_eq!(detect("Photos.zip"), Some((Format::Zip, "Photos")));
        assert_eq!(detect("a.7z"), Some((Format::SevenZip, "a")));
        assert_eq!(
            detect("notes.txt.gz"),
            Some((Format::Single("gzip"), "notes.txt"))
        );
        assert_eq!(detect("notes.txt"), None);
        assert_eq!(detect(".zip"), None);
    }

    #[test]
    fn picks_an_installed_program() {
        let only = |names: &'static [&'static str]| move |p: &str| names.contains(&p);
        let a = Path::new("/t/x.zip");
        let d = Path::new("/t/out");
        let (p, args) = command(Format::Zip, a, d, &only(&["unzip", "7z"])).unwrap();
        assert_eq!(p, "unzip");
        assert_eq!(args.last().unwrap(), "/t/out");
        let (p, args) = command(Format::Zip, a, d, &only(&["7z"])).unwrap();
        assert_eq!(p, "7z");
        assert!(args.contains(&"-o/t/out".to_owned()));
        assert!(command(Format::SevenZip, a, d, &only(&["unzip"])).is_none());
        assert!(can_extract(Format::Tar, &only(&["tar"])));
        assert!(!can_extract(Format::Single("zstd"), &only(&["gzip"])));
    }
}
