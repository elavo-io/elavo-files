//! The clipboard formats file managers share: GNOME's
//! `x-special/gnome-copied-files` (`copy` or `cut`, then one URI a line),
//! which Nautilus reads and writes, and the plain `text/uri-list`.

pub const GNOME_MIME: &str = "x-special/gnome-copied-files";
pub const URI_LIST_MIME: &str = "text/uri-list";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Copy,
    Cut,
}

pub fn gnome(op: Op, uris: &[String]) -> String {
    let mut text = String::from(match op {
        Op::Copy => "copy",
        Op::Cut => "cut",
    });
    for uri in uris {
        text.push('\n');
        text.push_str(uri);
    }
    text
}

pub fn uri_list(uris: &[String]) -> String {
    uris.iter().map(|u| format!("{u}\r\n")).collect()
}

pub fn parse_gnome(text: &str) -> Option<(Op, Vec<String>)> {
    let mut lines = text.lines().map(|l| l.trim_end_matches('\r'));
    let op = match lines.next()?.trim() {
        "copy" => Op::Copy,
        "cut" => Op::Cut,
        _ => return None,
    };
    let uris: Vec<String> = lines
        .filter(|l| !l.trim().is_empty())
        .map(str::to_owned)
        .collect();
    (!uris.is_empty()).then_some((op, uris))
}

/// URIs in a `text/uri-list`, skipping comments and blank lines.
pub fn parse_uri_list(text: &str) -> Vec<String> {
    text.lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(str::to_owned)
        .collect()
}

/// Whatever the clipboard held, as an operation and URIs.
pub fn parse(mime: &str, text: &str) -> Option<(Op, Vec<String>)> {
    if mime.starts_with(GNOME_MIME) {
        return parse_gnome(text);
    }
    let uris = parse_uri_list(text);
    (!uris.is_empty()).then_some((Op::Copy, uris))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_gnome_and_uri_lists() {
        let uris = vec![
            "file:///tmp/a%20b.txt".to_owned(),
            "file:///tmp/c".to_owned(),
        ];
        let g = gnome(Op::Cut, &uris);
        assert_eq!(g, "cut\nfile:///tmp/a%20b.txt\nfile:///tmp/c");
        assert_eq!(parse(GNOME_MIME, &g), Some((Op::Cut, uris.clone())));
        let l = uri_list(&uris);
        assert_eq!(parse(URI_LIST_MIME, &l), Some((Op::Copy, uris.clone())));
        assert_eq!(
            parse(URI_LIST_MIME, "# comment\r\nfile:///x\r\n\r\n"),
            Some((Op::Copy, vec!["file:///x".to_owned()]))
        );
        assert_eq!(parse(GNOME_MIME, "paste\nfile:///x"), None);
        assert_eq!(parse(GNOME_MIME, "copy\n"), None);
    }
}
