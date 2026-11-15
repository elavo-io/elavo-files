//! Open in Terminal: the person's terminal in a folder. `xdg-terminal-exec`
//! picks the default terminal where it is installed; otherwise Foot, which
//! Elavo ships and labwc's own menu opens, then the other common ones.

use std::path::Path;

use gtk::{gio, glib};

/// Terminals and how each is told where to start. All of them also start
/// in the folder they are run in.
const TERMINALS: &[(&str, &[&str])] = &[
    ("xdg-terminal-exec", &[]),
    ("foot", &["--working-directory"]),
    ("ptyxis", &["--new-window", "--working-directory"]),
    ("kgx", &["--working-directory"]),
    ("gnome-terminal", &["--working-directory"]),
    ("konsole", &["--workdir"]),
    ("alacritty", &["--working-directory"]),
    ("kitty", &["--directory"]),
];

/// The command line for the first terminal `has` finds.
fn command(dir: &Path, has: &dyn Fn(&str) -> bool) -> Option<Vec<String>> {
    let (program, args) = TERMINALS.iter().find(|(p, _)| has(p))?;
    let mut argv = vec![(*program).to_owned()];
    argv.extend(args.iter().map(|a| (*a).to_owned()));
    if !args.is_empty() {
        argv.push(dir.to_string_lossy().into_owned());
    }
    Some(argv)
}

pub fn open(dir: &Path) -> Result<(), String> {
    let has = |p: &str| glib::find_program_in_path(p).is_some();
    let argv = command(dir, &has).ok_or_else(|| "No terminal is installed.".to_owned())?;
    let launcher = gio::SubprocessLauncher::new(gio::SubprocessFlags::NONE);
    launcher.set_cwd(dir);
    let argv: Vec<&std::ffi::OsStr> = argv.iter().map(|a| a.as_ref()).collect();
    // GSubprocess reaps the child when it exits.
    launcher
        .spawn(&argv)
        .map(|_| ())
        .map_err(|e| e.message().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefers_the_default_then_foot() {
        let dir = Path::new("/home/me/Code");
        let only = |names: &'static [&'static str]| move |p: &str| names.contains(&p);
        assert_eq!(
            command(dir, &only(&["foot", "xdg-terminal-exec"])).unwrap(),
            ["xdg-terminal-exec"]
        );
        assert_eq!(
            command(dir, &only(&["foot", "kgx"])).unwrap(),
            ["foot", "--working-directory", "/home/me/Code"]
        );
        assert!(command(dir, &only(&[])).is_none());
    }
}
