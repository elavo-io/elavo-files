//! Connect to Server: what an address typed in it means, and the recent
//! servers, kept in `~/.config/elavo/servers` (one address per line, the
//! newest first). Passwords are never kept: GVfs and the keyring hold
//! those, and an address with one has it taken out before it is stored.

use std::path::PathBuf;

use gio::glib;

/// How many recent servers are kept.
pub const RECENT_LIMIT: usize = 10;

/// What Connect to Server can open, by its URI scheme, with the name shown
/// for each. `ssh` reads as `sftp`, `http(s)` as WebDAV, `\\server\share`
/// and a bare name as SMB, as in Finder.
pub const SCHEMES: &[(&str, &str)] = &[
    ("smb", "Windows / Samba (SMB)"),
    ("sftp", "SFTP (SSH)"),
    ("ftp", "FTP"),
    ("ftps", "FTP over TLS"),
    ("dav", "WebDAV"),
    ("davs", "WebDAV over HTTPS"),
    ("nfs", "NFS"),
    ("afp", "Apple Filing (AFP)"),
];

#[derive(Debug, PartialEq, Eq)]
pub enum Invalid {
    Empty,
    /// A scheme Connect to Server does not open, such as `file` or `mailto`.
    Scheme(String),
    NoServer,
}

impl std::fmt::Display for Invalid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Invalid::Empty => write!(f, "Enter a server address."),
            Invalid::Scheme(s) => write!(
                f,
                "\u{201c}{s}://\u{201d} isn\u{2019}t a kind of server Files can connect to."
            ),
            Invalid::NoServer => write!(f, "The address has no server name."),
        }
    }
}

/// The address as a URI GVfs mounts: a scheme it knows and a server name.
pub fn normalize(input: &str) -> Result<String, Invalid> {
    let s = input.trim();
    if s.is_empty() {
        return Err(Invalid::Empty);
    }
    let (scheme, rest) = if let Some(unc) = s.strip_prefix("\\\\") {
        ("smb".to_owned(), unc.replace('\\', "/"))
    } else if let Some((scheme, rest)) = s.split_once("://") {
        (scheme.to_ascii_lowercase(), rest.to_owned())
    } else {
        ("smb".to_owned(), s.to_owned())
    };
    let scheme = match scheme.as_str() {
        "ssh" => "sftp".to_owned(),
        "http" | "webdav" => "dav".to_owned(),
        "https" | "webdavs" => "davs".to_owned(),
        "cifs" => "smb".to_owned(),
        _ => scheme,
    };
    if !SCHEMES.iter().any(|(k, _)| *k == scheme) {
        return Err(Invalid::Scheme(scheme));
    }
    let host = rest.split('/').next().unwrap_or("");
    let host = host.rsplit('@').next().unwrap_or("");
    if host.is_empty() || host.starts_with(':') {
        return Err(Invalid::NoServer);
    }
    Ok(format!("{scheme}://{rest}"))
}

/// The address with any password taken out (`user:secret@` becomes
/// `user@`), for storing, logging and showing.
pub fn without_password(uri: &str) -> String {
    let Some((scheme, rest)) = uri.split_once("://") else {
        return uri.to_owned();
    };
    let end = rest.find('/').unwrap_or(rest.len());
    let (authority, path) = rest.split_at(end);
    let Some((userinfo, host)) = authority.rsplit_once('@') else {
        return uri.to_owned();
    };
    let user = userinfo.split(':').next().unwrap_or("");
    if user.is_empty() {
        format!("{scheme}://{host}{path}")
    } else {
        format!("{scheme}://{user}@{host}{path}")
    }
}

/// What kind of server an address is, for the recent list.
pub fn kind(uri: &str) -> &'static str {
    let scheme = uri.split("://").next().unwrap_or("");
    SCHEMES
        .iter()
        .find(|(k, _)| *k == scheme)
        .map(|(_, name)| *name)
        .unwrap_or("")
}

fn path() -> PathBuf {
    glib::user_config_dir().join("elavo").join("servers")
}

pub fn recent() -> Vec<String> {
    read_from(&path())
}

fn read_from(path: &std::path::Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(without_password)
        .take(RECENT_LIMIT)
        .collect()
}

fn write_to(path: &std::path::Path, list: &[String]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut text = list.join("\n");
    text.push('\n');
    std::fs::write(path, text)
}

/// Put `uri` first in the recent servers.
pub fn remember(uri: &str) -> std::io::Result<()> {
    let p = path();
    write_to(&p, &with_first(read_from(&p), uri))
}

pub fn forget(uri: &str) -> std::io::Result<()> {
    let p = path();
    let list: Vec<String> = read_from(&p).into_iter().filter(|u| u != uri).collect();
    write_to(&p, &list)
}

fn with_first(list: Vec<String>, uri: &str) -> Vec<String> {
    let uri = without_password(uri);
    let trimmed = |u: &str| u.trim_end_matches('/').to_owned();
    let mut out = vec![uri.clone()];
    out.extend(list.into_iter().filter(|u| trimmed(u) != trimmed(&uri)));
    out.truncate(RECENT_LIMIT);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_addresses_as_finder_does() {
        assert_eq!(normalize(" nas.local ").unwrap(), "smb://nas.local");
        assert_eq!(normalize("\\\\nas\\Media").unwrap(), "smb://nas/Media");
        assert_eq!(normalize("SMB://nas/share").unwrap(), "smb://nas/share");
        assert_eq!(
            normalize("ssh://me@box:2222/srv").unwrap(),
            "sftp://me@box:2222/srv"
        );
        assert_eq!(
            normalize("https://cloud.example/dav").unwrap(),
            "davs://cloud.example/dav"
        );
        assert_eq!(normalize("ftp://ftp.example").unwrap(), "ftp://ftp.example");
        assert_eq!(
            normalize("nfs://server/export").unwrap(),
            "nfs://server/export"
        );
        assert_eq!(normalize(""), Err(Invalid::Empty));
        assert_eq!(
            normalize("file:///etc"),
            Err(Invalid::Scheme("file".to_owned()))
        );
        assert_eq!(normalize("sftp://"), Err(Invalid::NoServer));
        assert_eq!(normalize("sftp://me@/x"), Err(Invalid::NoServer));
    }

    #[test]
    fn never_keeps_a_password() {
        assert_eq!(
            without_password("smb://me:secret@nas/share"),
            "smb://me@nas/share"
        );
        assert_eq!(without_password("smb://DOM;me:pw@nas"), "smb://DOM;me@nas");
        assert_eq!(without_password("ftp://:pw@host/x"), "ftp://host/x");
        assert_eq!(
            without_password("sftp://me@box:22/srv"),
            "sftp://me@box:22/srv"
        );
        assert_eq!(without_password("dav://host/a@b"), "dav://host/a@b");
    }

    #[test]
    fn keeps_the_newest_first_once_each() {
        let dir = std::env::temp_dir().join(format!("elavo-servers-{}", std::process::id()));
        let file = dir.join("servers");
        let mut list = vec![];
        for i in 0..12 {
            list = with_first(list, &format!("smb://host{i}"));
        }
        list = with_first(list, "smb://host5/");
        list = with_first(list, "sftp://me:pw@box");
        write_to(&file, &list).unwrap();
        let back = read_from(&file);
        assert_eq!(back.len(), RECENT_LIMIT);
        assert_eq!(back[0], "sftp://me@box");
        assert_eq!(back[1], "smb://host5/");
        assert_eq!(
            back.iter().filter(|u| u.starts_with("smb://host5")).count(),
            1
        );
        assert!(!std::fs::read_to_string(&file).unwrap().contains("pw"));
        assert_eq!(kind("davs://x"), "WebDAV over HTTPS");
        let _ = std::fs::remove_dir_all(dir);
    }
}
