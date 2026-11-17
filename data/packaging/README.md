# Making Files the default file manager (draft)

What the Elavo image needs so Files is its file manager, in place of Nautilus.
`scripts/install` puts Files' own files in place; the two patches here are
drafts for the other repositories and have not been applied there.

## What Files installs (`scripts/install`, honors DESTDIR and PREFIX)

| File | Why |
| --- | --- |
| `/usr/bin/elavo-files` | the app |
| `/usr/share/applications/io.elavo.Files.desktop` | `MimeType=inode/directory`, `DBusActivatable=true`, the New Window action |
| `/usr/share/dbus-1/services/io.elavo.Files.service` | D-Bus starts Files for the desktop file (`--gapplication-service`) |
| `/usr/share/dbus-1/services/org.freedesktop.FileManager1.service` | D-Bus starts Files for Show in Folder from other apps |
| `/etc/xdg/autostart/io.elavo.Files.LocalSend.desktop` | the LocalSend receiver at login (`--localsend-daemon`) |

Files is one instance per session (`io.elavo.Files`). A second
`elavo-files FOLDER|FILE|URI` hands its arguments to the first: a folder
opens in a window, a file shows selected in its folder, a network address
mounts (asking for a password if needed) and opens. The running instance
owns `org.freedesktop.FileManager1` (`ShowFolders`, `ShowItems`,
`ShowItemProperties` → Get Info), taking it over from a file manager that
allows replacement.

## The image (`elavo-image.patch`, for ~/GitHub/elavo)

1. A `files` build stage, like the other Elavo apps: clone elavo-files,
   `cargo build --release --locked`, `DESTDIR=/out scripts/install`, and
   `COPY --from=files /out /files_files`.
2. In `build.sh`, copy `/ctx/files_files` after the packages, check the
   files, and add `/usr/share/applications/elavo-mimeapps.list` with
   `inode/directory=io.elavo.Files.desktop;`.
   GLib reads `<desktop>-mimeapps.list` for `XDG_CURRENT_DESKTOP=elavo`,
   ahead of the distribution's defaults. A person's own
   `~/.config/mimeapps.list` still wins.
3. `system-applications`: `io.elavo.Files` replaces `org.gnome.Nautilus`, so
   the locked entry in /Applications is Files.
4. Packages Files uses that the image lacks today: `gvfs-smb` (Windows/Samba
   shares and browsing them in Network), `gvfs-nfs`, `gvfs-afc` (iPhone),
   `gvfs-mtp` (Android), `gvfs-gphoto2` (cameras), `gvfs-archive`, and
   `bsdtar` (an extract fallback). Nautilus used to bring these, so the
   image lists them itself: `localsearch` (search), `glycin-thumbnailer` and
   `gst-thumbnailers` (thumbnails), and `gvfs-fuse`. Already there: gvfs
   (dav, ftp, sftp, network, dns-sd, wsdd, trash, recent, computer), udisks2,
   glib-networking (LocalSend's TLS), openssl (LocalSend's certificate), zip,
   7zip, tar, the GStreamer plugins Quick Look plays video with, foot (Open
   in Terminal). Optional: `gvfs-afp` (old Apple file sharing), `gvfs-goa`.
5. Nautilus and Sushi are not installed. Folio's desktop sends its file
   operations, Undo and Quick Look to Files' own D-Bus interfaces in place
   of Nautilus' `FileOperations2` and Sushi, and the panel's Files menus
   drive Files windows. elavo-applications still builds a Nautilus module;
   with Nautilus gone nothing loads it.

## The dock (`elavo-folio-files-pin.patch`, for ~/GitHub/elavo-folio)

The dock's locked first pin becomes `io.elavo.Files.desktop`; a saved
`org.gnome.Nautilus.desktop` pin turns into it, so people keep one Files
icon. Files' windows have the app id `io.elavo.Files`, which matches it.
Folio has since taken this in, and its panel menus and desktop file
operations go to Files as well.

## Checking it on a running system

```sh
xdg-mime query default inode/directory     # io.elavo.Files.desktop
gio open ~/Documents                         # a Files window
gdbus call --session --dest org.freedesktop.FileManager1 \
  --object-path /org/freedesktop/FileManager1 \
  --method org.freedesktop.FileManager1.ShowItems "['file://$HOME/.bashrc']" ''
```

In a browser, a download's Show in Folder should open Files with the file
selected.
