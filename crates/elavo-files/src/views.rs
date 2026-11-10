//! Cell factories and sorters for the Icons view (a `GridView`) and the
//! List view (a `ColumnView` over a `TreeListModel`).

use std::cmp::Ordering;

use elavo_files_core::entry;
use elavo_files_core::sort::{self, Key};
use gtk::prelude::*;
use gtk::{gdk, gio, glib};

/// Where a loaded thumbnail texture is kept on its `FileInfo`.
const TEXTURE: &str = "elavo-files::texture";

/// The `FileInfo` behind a row: the item itself in Icons, or the item of a
/// `TreeListRow` in List.
pub fn info_of(object: &glib::Object) -> Option<gio::FileInfo> {
    if let Some(row) = object.downcast_ref::<gtk::TreeListRow>() {
        return row.item().and_then(|i| i.downcast::<gio::FileInfo>().ok());
    }
    object.clone().downcast::<gio::FileInfo>().ok()
}

fn thumbnail(info: &gio::FileInfo) -> Option<gdk::Texture> {
    if let Some(obj) = info.attribute_object(TEXTURE) {
        return obj.downcast::<gdk::Texture>().ok();
    }
    let path = entry::thumbnail(info)?;
    let texture = gdk::Texture::from_filename(path).ok()?;
    info.set_attribute_object(TEXTURE, &texture);
    Some(texture)
}

/// Images without a cached thumbnail are scaled down here, off the main
/// loop's critical path, up to this size of file.
const PREVIEW_MAX_BYTES: u64 = 40_000_000;
const PREVIEW_SIZE: i32 = 256;

/// Load a small preview of an image file and show it once it is ready,
/// if `image` still shows the same file.
fn load_preview(image: &gtk::Image, info: &gio::FileInfo, size: i32) {
    let ct = entry::content_type(info);
    if !ct.starts_with("image/") || entry::size(info) > PREVIEW_MAX_BYTES {
        return;
    }
    let Some(file) = entry::target(info) else {
        return;
    };
    let uri = file.uri().to_string();
    image.set_widget_name(&uri);
    let image = image.clone();
    let info = info.clone();
    glib::spawn_future_local(async move {
        let Ok(stream) = file.read_future(glib::Priority::LOW).await else {
            return;
        };
        let Ok(pixbuf) = gtk::gdk_pixbuf::Pixbuf::from_stream_at_scale_future(
            &stream,
            PREVIEW_SIZE,
            PREVIEW_SIZE,
            true,
        )
        .await
        else {
            return;
        };
        #[allow(deprecated)]
        let texture = gdk::Texture::for_pixbuf(&pixbuf);
        info.set_attribute_object(TEXTURE, &texture);
        if image.widget_name() == uri {
            image.set_paintable(Some(&texture));
            image.set_pixel_size(size);
        }
    });
}

/// A cached thumbnail when there is one and the icon is big enough to show
/// it, otherwise the themed icon for the file (folders get the theme's
/// folder icon).
pub fn set_icon(image: &gtk::Image, info: &gio::FileInfo, size: i32) {
    image.set_pixel_size(size);
    if size >= 24 {
        if let Some(texture) = thumbnail(info) {
            image.set_paintable(Some(&texture));
            return;
        }
    }
    let icon = info
        .icon()
        .unwrap_or_else(|| gio::content_type_get_icon(&entry::content_type(info)));
    image.set_widget_name("");
    image.set_from_gicon(&icon);
    if size >= 48 {
        load_preview(image, info, size);
    }
}

/// Icons view cells: the icon over a name of up to two lines.
pub fn icons_factory(size: i32) -> gtk::SignalListItemFactory {
    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup(move |_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().expect("list item");
        let image = gtk::Image::builder()
            .pixel_size(size)
            .halign(gtk::Align::Center)
            .css_classes(["files-icon-picture"])
            .build();
        let name = gtk::Label::builder()
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .lines(2)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .justify(gtk::Justification::Center)
            .max_width_chars(((size + 40) / 7).max(11))
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Start)
            .css_classes(["files-icon-name"])
            .build();
        // Break long names between characters without adding hyphens.
        let attrs = gtk::pango::AttrList::new();
        attrs.insert(gtk::pango::AttrInt::new_insert_hyphens(false));
        name.set_attributes(Some(&attrs));
        let cell = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .width_request(size + 44)
            .valign(gtk::Align::Start)
            .build();
        cell.append(&image);
        cell.append(&name);
        item.set_child(Some(&cell));
    });
    factory.connect_bind(move |_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().expect("list item");
        let (Some(info), Some(cell)) = (item.item().and_then(|o| info_of(&o)), item.child()) else {
            return;
        };
        let image = cell
            .first_child()
            .and_downcast::<gtk::Image>()
            .expect("image");
        let name = image
            .next_sibling()
            .and_downcast::<gtk::Label>()
            .expect("label");
        set_icon(&image, &info, size);
        let text = entry::name(&info);
        name.set_label(&text);
        cell.set_tooltip_text(Some(&text));
    });
    factory
}

/// Load a folder's children only when its row is first expanded, so a
/// list of many folders does not read them all.
fn watch_expansion(row: &gtk::TreeListRow) {
    // SAFETY: the key holds a plain bool and is only read with that type.
    unsafe {
        if row.data::<bool>("elavo-files-watched").is_some() {
            return;
        }
        row.set_data("elavo-files-watched", true);
    }
    row.connect_expanded_notify(|row| {
        if !row.is_expanded() {
            return;
        }
        let Some(children) = row.children() else {
            return;
        };
        let dir = children
            .downcast_ref::<gtk::FilterListModel>()
            .and_then(|f| f.model())
            .and_downcast::<gtk::DirectoryList>();
        let info = row.item().and_downcast::<gio::FileInfo>();
        if let (Some(dir), Some(info)) = (dir, info) {
            if dir.file().is_none() {
                dir.set_file(entry::target(&info).as_ref());
            }
        }
    });
}

/// List view Name cells: a disclosure triangle for folders, the icon and
/// the name.
pub fn name_factory(size: i32) -> gtk::SignalListItemFactory {
    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup(move |_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().expect("list item");
        let content = gtk::Box::builder().spacing(6).build();
        content.append(&gtk::Image::builder().pixel_size(size).build());
        content.append(
            &gtk::Label::builder()
                .xalign(0.0)
                .ellipsize(gtk::pango::EllipsizeMode::Middle)
                .build(),
        );
        let expander = gtk::TreeExpander::builder()
            .child(&content)
            .indent_for_icon(true)
            .build();
        item.set_child(Some(&expander));
    });
    factory.connect_bind(move |_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().expect("list item");
        let Some(row) = item.item().and_downcast::<gtk::TreeListRow>() else {
            return;
        };
        let Some(expander) = item.child().and_downcast::<gtk::TreeExpander>() else {
            return;
        };
        expander.set_list_row(Some(&row));
        watch_expansion(&row);
        let Some(info) = row.item().and_downcast::<gio::FileInfo>() else {
            return;
        };
        let content = expander.child().expect("content");
        let image = content
            .first_child()
            .and_downcast::<gtk::Image>()
            .expect("image");
        let label = image
            .next_sibling()
            .and_downcast::<gtk::Label>()
            .expect("label");
        set_icon(&image, &info, size);
        label.set_label(&entry::name(&info));
    });
    factory.connect_unbind(|_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().expect("list item");
        if let Some(expander) = item.child().and_downcast::<gtk::TreeExpander>() {
            expander.set_list_row(None);
        }
    });
    factory
}

/// A one-label column: Date Modified, Size or Kind.
pub fn text_factory(text: fn(&gio::FileInfo) -> String, xalign: f32) -> gtk::SignalListItemFactory {
    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup(move |_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().expect("list item");
        item.set_child(Some(
            &gtk::Label::builder()
                .xalign(xalign)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .css_classes(["files-dim"])
                .build(),
        ));
    });
    factory.connect_bind(move |_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().expect("list item");
        let (Some(info), Some(label)) = (
            item.item().and_then(|o| info_of(&o)),
            item.child().and_downcast::<gtk::Label>(),
        ) else {
            return;
        };
        label.set_label(&text(&info));
    });
    factory
}

fn ordering(o: Ordering) -> gtk::Ordering {
    match o {
        Ordering::Less => gtk::Ordering::Smaller,
        Ordering::Equal => gtk::Ordering::Equal,
        Ordering::Greater => gtk::Ordering::Larger,
    }
}

/// Ascending order by `key` over `FileInfo` items.
pub fn sorter(key: Key) -> gtk::CustomSorter {
    gtk::CustomSorter::new(move |a, b| {
        let (Some(a), Some(b)) = (info_of(a), info_of(b)) else {
            return gtk::Ordering::Equal;
        };
        let mut sa = Default::default();
        let mut sb = Default::default();
        let fa = entry::fields(&a, &mut sa);
        let fb = entry::fields(&b, &mut sb);
        ordering(sort::compare(key, &fa, &fb))
    })
}
