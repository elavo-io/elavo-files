# Elavo Files

A Finder-style file manager for Elavo OS, written in Rust with GTK 4.

This is phase 1: the window, the sidebar, and the Icons and List views.
Nautilus is still the default file manager on Elavo; installing or running
Elavo Files does not change that.

## What works

- A window in the Elavo Settings style, with the window buttons at the top
  left (following `gtk-decoration-layout`) and a dark sidebar.
- Sidebar sections: Recents; Favorites (Applications, Desktop, Documents,
  Downloads); Locations (home, mounted drives from the volume monitor,
  Network, Trash).
- Toolbar: back and forward, the folder title, the view switcher, the more
  menu (New Folder, Show Hidden Files, icon size), and search, which filters
  the current folder by name.
- Icons view: a grid with image previews and thumbnails, two-line names.
- List view: Name (with disclosure triangles), Date Modified, Size, and Kind,
  sortable by any column header.
- Double-click or Enter opens a folder, or opens a file in its default app.
  Folders update live when files change.

Columns, Gallery, group-by, and Share are shown but not active yet.

## Keyboard shortcuts

| Keys | Action |
| --- | --- |
| Ctrl+1, Ctrl+2 | Icons, List |
| Ctrl+plus (or Ctrl+equals), Ctrl+minus | Larger or smaller icons (48, 64, 96, 128 px; list rows 16, 24, 32 px) |
| Ctrl+0 | Default icon size |
| Alt+Left, Alt+Right (or Ctrl+[, Ctrl+]) | Back, forward |
| Alt+Up | Enclosing folder |
| Alt+Home, Ctrl+Shift+H | Home |
| Ctrl+H, Ctrl+Shift+. | Show hidden files |
| Ctrl+Shift+N | New folder |
| Ctrl+F | Search |
| F5, Ctrl+R | Reload |
| Ctrl+N, Ctrl+W | New window, close window |

The view, the icon sizes, hidden files, and the window size are saved in
`~/.config/elavo/files.conf`.

## Layout

- `crates/elavo-files-core`: formatting, sorting, history, places, and zoom
  steps, using GIO only (no GTK), with unit tests.
- `crates/elavo-files`: the GTK app (`elavo-files`, application id
  `io.elavo.Files`).
- `data/resources`: the stylesheet and toolbar icons, compiled into a
  GResource.
- `scripts/nested-session.sh`: a headless labwc session for testing and
  screenshots.

## Build and run

```sh
cargo build
cargo test
cargo run -p elavo-files -- ~/Documents
```

On Elavo, run these inside the development toolbox
(`toolbox run -c elavo-dev cargo ...`).

To test without touching the desktop session:

```sh
scripts/nested-session.sh start 1366x768
scripts/nested-session.sh run ~/Downloads
scripts/nested-session.sh keys -M ctrl 2 -m ctrl
scripts/nested-session.sh shot list
scripts/nested-session.sh stop
```

The nested session uses its own D-Bus session bus and config folder, so it
never talks to a running file manager.

## License

MIT. See [LICENSE.md](LICENSE.md).
