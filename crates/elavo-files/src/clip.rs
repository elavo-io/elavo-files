//! Copy, Cut and Paste of files, in the formats Nautilus and other GTK
//! apps use: `x-special/gnome-copied-files` (which says copy or cut),
//! `text/uri-list` (through `gdk::FileList`) and plain text paths for
//! pasting into a text field.

use elavo_files_core::clipboard::{self, Op};
use gtk::prelude::*;
use gtk::{gdk, gio, glib};

pub fn put(widget: &impl IsA<gtk::Widget>, files: &[gio::File], op: Op) {
    let uris: Vec<String> = files.iter().map(|f| f.uri().to_string()).collect();
    let text = files
        .iter()
        .map(|f| {
            f.path()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| f.uri().to_string())
        })
        .collect::<Vec<_>>()
        .join("\n");
    let bytes = |s: String| glib::Bytes::from_owned(s.into_bytes());
    let providers = [
        gdk::ContentProvider::for_bytes(clipboard::GNOME_MIME, &bytes(clipboard::gnome(op, &uris))),
        gdk::ContentProvider::for_value(&gdk::FileList::from_array(files).to_value()),
        gdk::ContentProvider::for_bytes("text/plain;charset=utf-8", &bytes(text.clone())),
        gdk::ContentProvider::for_bytes("text/plain", &bytes(text)),
    ];
    let _ = widget
        .clipboard()
        .set_content(Some(&gdk::ContentProvider::new_union(&providers)));
}

/// Whether the clipboard holds files to paste.
pub fn has_files(widget: &impl IsA<gtk::Widget>) -> bool {
    let formats = widget.clipboard().formats();
    formats.contain_mime_type(clipboard::GNOME_MIME)
        || formats.contain_mime_type(clipboard::URI_LIST_MIME)
        || formats.contains_type(gdk::FileList::static_type())
}

pub fn clear(widget: &impl IsA<gtk::Widget>) {
    let _ = widget
        .clipboard()
        .set_content(None::<&gdk::ContentProvider>);
}

pub async fn read(widget: &impl IsA<gtk::Widget>) -> Option<(Op, Vec<gio::File>)> {
    let (stream, mime) = widget
        .clipboard()
        .read_future(
            &[clipboard::GNOME_MIME, clipboard::URI_LIST_MIME],
            glib::Priority::DEFAULT,
        )
        .await
        .ok()?;
    let out = gio::MemoryOutputStream::new_resizable();
    out.splice_future(
        &stream,
        gio::OutputStreamSpliceFlags::CLOSE_SOURCE | gio::OutputStreamSpliceFlags::CLOSE_TARGET,
        glib::Priority::DEFAULT,
    )
    .await
    .ok()?;
    let bytes = out.steal_as_bytes();
    let (op, uris) = clipboard::parse(&mime, &String::from_utf8_lossy(&bytes))?;
    Some((op, uris.iter().map(|u| gio::File::for_uri(u)).collect()))
}
