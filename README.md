# Elavo Files

A Finder-style file manager for Elavo OS, written in Rust with GTK 4.

Phases 1 to 3 are in: the window, the sidebar, the Icons, List, Columns,
and Gallery views, file operations, previews, and thumbnails.
Nautilus is still the default file manager on Elavo; installing or running
Elavo Files does not change that.

## What works

- A window in the Elavo Settings style, with the window buttons at the top
  left (following `gtk-decoration-layout`) and a dark sidebar.
- Sidebar sections: Recents; Favorites (Applications, Desktop, Documents,
  Downloads by default); Locations (home, mounted drives from the volume monitor,
  Network, Trash). Applications is the shared `/Applications` folder
  (`~/Applications` if there is none).
- Favorites are the person's, as in Finder: drop a folder (from any view,
  the title's proxy icon, or another app) between two favorites to add it
  there, shown by a line; drop on a favorite to move or copy into it. Drag a
  favorite to reorder it, or out of the window and let go to remove it (it
  shows "Remove" once outside; letting go inside or pressing Escape keeps
  it). Right click: Open, Remove from Sidebar, Restore Default Favorites;
  Add to Sidebar (Ctrl+T) on folders. Removing never touches the folder.
  The list is `~/.config/elavo/favorites` (GTK bookmarks format), shared
  live by every window; the first run brings over the folders bookmarked in
  `~/.config/gtk-3.0/bookmarks` that are not standard ones, and never writes
  that file.
- Toolbar: back and forward, the folder title, the view switcher (Icons,
  List, Columns, Gallery; the choice is remembered), the more menu (New
  Folder, Show Hidden Files, icon size, Show Preview), and search, which
  filters the current folder by name.
- The folder title, as in Finder: a click slides in the folder's icon (the
  proxy icon), which drags the folder like any file; it slides away again
  when the pointer leaves. A right click lists the folder and the folders
  it is in, then the drive and the computer, and goes to the one picked.
- Elavo applications (`.eapp` bundles, from elavo-applications) show as
  the app, with its name and icon (in the previews too; Columns previews a
  bundle instead of opening it as a folder), and open the app on double-click or
  Open; Show Package Contents opens the folder. As with the Nautilus
  extension, installs in an Applications folder are never moved or copied:
  dragged or pasted elsewhere they become shortcuts. Move to Trash refuses
  a running app, and in `/Applications` asks elavo-applications to
  uninstall. A copy outside an Applications folder opens through
  `elavo-applications-ctl open`.
- macOS applications (`.app` folders with `Contents/Info.plist`) keep their
  name but show the generic application icon and "macOS Application"; Columns
  previews them, and opening one shows its contents (Files can't run it).
- Icons view: a grid with image previews and thumbnails, two-line names.
  Only an icon and its name are the item: a click anywhere else clears the
  selection, as in Finder.
- List view: Name (with disclosure triangles), Date Modified, Size, and Kind,
  sortable by any column header.
- Columns view: Finder's columns, scrolling sideways. Selecting a folder
  opens it in the next column; selecting a file shows its preview there
  (picture, name, kind and size, dates, image dimensions, and the quick
  actions). Columns are resized by their right edge; Left and Right move
  between them.
- Gallery view: the selected file large in the middle, a filmstrip along
  the bottom (names on hover), and its details on the right, with Rotate
  Left, Markup, and More… under them. Left and Right step through the
  filmstrip.
- Show Preview (Ctrl+Shift+P): a preview pane beside Icons and List for the
  selected item, or the folder when nothing is selected.
- Previews and thumbnails load in the background, a few at a time: images
  are decoded at the size shown (up to 60 MB files), other files use the
  shared thumbnail cache (`~/.cache/thumbnails`) or the system's
  thumbnailers, whose results go into that cache as the spec describes.
  Plain text files preview their first lines.
- Rotate Left turns a PNG, JPEG, BMP, or TIFF image a quarter turn and saves
  it in place.
- Double-click or Enter opens a folder, or opens a file in its default app.
  Folders update live when files change.

- Context menus on items, in Finder's order: Open, Open With (apps for the
  type, then Other…), Move to Trash, Get Info, Rename, Compress, Duplicate,
  Make Alias, Quick Look, Copy, Share. On empty space: New Folder, Paste
  Item, Move Item Here, Show Hidden Files, and View. In the Trash: Put Back
  and Delete Immediately, and Empty Trash on empty space.
- Copy, cut, and paste, shared with other apps as `text/uri-list` and
  `x-special/gnome-copied-files`.
- Copy, move, Move to Trash, Delete Immediately, Empty Trash, and Put Back,
  run off the main thread. When a name is taken, a dialog asks to Keep Both,
  Replace, Skip, or Stop, optionally for all remaining items. A ring in the
  toolbar shows running operations; click it for progress and to cancel.
- Rename in place (F2 or the menu), Duplicate ("Report copy.pdf"), Make
  Alias (a symbolic link, "Report.pdf alias"), and Compress (to a .zip).
- Undo (Ctrl+Z) for the last move, rename, trash, or new folder.
- Drag and drop: onto folders and sidebar places, moving on the same drive
  and copying across drives (Ctrl or Alt always copies). Files can be
  dragged out to other apps and dropped in from them.
- Selecting with a rectangle, as in Finder: drag from any empty space (in
  the list, anywhere in a row but its icon and name). The selection follows
  the rectangle, the view scrolls near the top and bottom edges, and Ctrl
  or Shift adds to the selection.
- If a drive has no Trash (such as a RAM disk), Move to Trash offers to
  delete the items immediately.

Markup, group-by, Get Info, Quick Look, and Share are shown but not active
yet.

## Keyboard shortcuts

| Keys | Action |
| --- | --- |
| Ctrl+1, Ctrl+2, Ctrl+3, Ctrl+4 | Icons, List, Columns, Gallery |
| Ctrl+Shift+P | Show or hide the preview pane |
| Ctrl+plus (or Ctrl+equals), Ctrl+minus | Larger or smaller icons (48, 64, 96, 128 px; list rows 16, 24, 32 px) |
| Ctrl+0 | Default icon size |
| Alt+Left, Alt+Right (or Ctrl+[, Ctrl+]) | Back, forward |
| Alt+Up | Enclosing folder |
| Alt+Home, Ctrl+Shift+H | Home |
| Ctrl+H, Ctrl+Shift+. | Show hidden files |
| Ctrl+Shift+N | New folder |
| Ctrl+T | Add the selected folders (or this folder) to the sidebar |
| Ctrl+C, Ctrl+X, Ctrl+V | Copy, cut, paste |
| Ctrl+Shift+V (or Ctrl+Alt+V) | Move the copied items here |
| Delete | Move to Trash (in the Trash: delete immediately) |
| Shift+Delete | Delete immediately, after asking |
| F2 | Rename |
| Ctrl+D | Duplicate |
| Ctrl+A | Select all |
| Ctrl+Z | Undo |
| Ctrl+O, Ctrl+Down | Open |
| Menu, Shift+F10 | Context menu |
| Ctrl+F | Search |
| F5, Ctrl+R | Reload |
| Ctrl+N, Ctrl+W | New window, close window |

The view, the icon sizes, hidden files, and the window size are saved in
`~/.config/elavo/files.conf`, the Favorites in `~/.config/elavo/favorites`.

## Layout

- `crates/elavo-files-core`: formatting, sorting, history, places, zoom
  steps, names for copies and aliases, the clipboard formats, the home
  Trash, and the file operations engine, using GIO only (no GTK), with unit
  tests.
- `crates/elavo-files`: the GTK app (`elavo-files`, application id
  `io.elavo.Files`).
- `data/resources`: the stylesheet and toolbar icons, compiled into a
  GResource.
- `scripts/nested-session.sh`: a headless labwc session for testing and
  screenshots. Set `ELAVO_SHARED_APPLICATIONS` to show another folder as
  Applications (the toolbox sees `/Applications` as
  `/run/host/Applications`).

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

The nested session uses its own D-Bus session bus, config folder, home, and
data folder, so it never talks to a running file manager and Move to Trash
goes to a private Trash under `/tmp`. Run it on a scratch folder under
`/tmp`. The toolbox has no GVfs, so there the Trash is read from that home
Trash directly.

## License

MIT. See [LICENSE.md](LICENSE.md).
