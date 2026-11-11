//! Thumbnails, loaded off the main loop's critical path.
//!
//! In order:
//! 1. GNOME's thumbnail cache (`~/.cache/thumbnails`), through GIO's
//!    `thumbnail::path` when it is still valid.
//! 2. Images up to [`IMAGE_MAX_BYTES`], decoded here at the size asked for.
//! 3. The system's thumbnailers (`/usr/share/thumbnailers`, as Nautilus and
//!    gnome-desktop use them) for video, office files and the like. Their
//!    result goes into the cache as the freedesktop spec says, so Nautilus
//!    and Files share it.
//!
//! Only a few decodes and thumbnailers run at once; a request that is no
//! longer wanted (its cell scrolled away) gives up while it waits.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use elavo_files_core::entry;
use gtk::prelude::*;
use gtk::{gdk, gio, glib};

/// Images larger than this are left to the thumbnailers.
pub const IMAGE_MAX_BYTES: u64 = 60_000_000;
/// The cache's "large" size, used for anything up to it.
const LARGE: i32 = 256;
const IMAGE_SLOTS: usize = 4;
const THUMBNAILER_SLOTS: usize = 2;
const THUMBNAILER_TIMEOUT: Duration = Duration::from_secs(20);

const QUERY: &str = "standard::content-type,standard::fast-content-type,standard::size,\
standard::type,time::modified,thumbnail::path,thumbnail::failed,thumbnail::is-valid";

thread_local! {
    static BUSY: RefCell<HashMap<&'static str, usize>> = RefCell::default();
    /// URIs a thumbnailer failed on this run, not to be tried again.
    static FAILED: RefCell<HashSet<String>> = RefCell::default();
    static THUMBNAILERS: Vec<Thumbnailer> = find_thumbnailers();
}

/// A place in a queue; freed when dropped.
struct Slot(&'static str);

impl Drop for Slot {
    fn drop(&mut self) {
        BUSY.with(|b| {
            if let Some(n) = b.borrow_mut().get_mut(self.0) {
                *n = n.saturating_sub(1);
            }
        });
    }
}

async fn slot(kind: &'static str, max: usize, wanted: &dyn Fn() -> bool) -> Option<Slot> {
    loop {
        if !wanted() {
            return None;
        }
        let free = BUSY.with(|b| {
            let mut b = b.borrow_mut();
            let n = b.entry(kind).or_insert(0);
            if *n < max {
                *n += 1;
                true
            } else {
                false
            }
        });
        if free {
            return Some(Slot(kind));
        }
        glib::timeout_future(Duration::from_millis(25)).await;
    }
}

#[allow(deprecated)]
fn texture_of(pixbuf: &gtk::gdk_pixbuf::Pixbuf) -> gdk::Texture {
    gdk::Texture::for_pixbuf(pixbuf)
}

/// Decode an image at most `size` pixels on its longer side, turned the
/// way its EXIF orientation says.
pub async fn image(file: &gio::File, size: i32) -> Option<gdk::Texture> {
    let stream = file.read_future(glib::Priority::LOW).await.ok()?;
    let pixbuf = gtk::gdk_pixbuf::Pixbuf::from_stream_at_scale_future(&stream, size, size, true)
        .await
        .ok()?;
    let pixbuf = pixbuf.apply_embedded_orientation().unwrap_or(pixbuf);
    Some(texture_of(&pixbuf))
}

async fn cached(path: &Path) -> Option<gdk::Texture> {
    let (bytes, _) = gio::File::for_path(path)
        .load_contents_future()
        .await
        .ok()?;
    gdk::Texture::from_bytes(&glib::Bytes::from_owned(bytes)).ok()
}

/// The best picture of `file` up to `size` pixels, or None for its icon.
/// `info` saves a query when the caller has one; `wanted` is asked while
/// waiting for a free slot.
pub async fn texture(
    file: &gio::File,
    info: Option<&gio::FileInfo>,
    size: i32,
    wanted: &dyn Fn() -> bool,
) -> Option<gdk::Texture> {
    let queried;
    let info = match info {
        Some(i) => i,
        None => {
            queried = file
                .query_info_future(QUERY, gio::FileQueryInfoFlags::NONE, glib::Priority::LOW)
                .await
                .ok()?;
            &queried
        }
    };
    if entry::is_dir(info) {
        return None;
    }
    let ct = entry::content_type(info);
    let is_image = ct.starts_with("image/") && entry::size(info) <= IMAGE_MAX_BYTES;
    // A small picture comes from the cache when there is one.
    if size <= LARGE {
        if let Some(path) = entry::thumbnail(info) {
            if let Some(t) = cached(&path).await {
                return Some(t);
            }
        }
    }
    if is_image {
        let _slot = slot("image", IMAGE_SLOTS, wanted).await?;
        if let Some(t) = image(file, size).await {
            return Some(t);
        }
    }
    let uri = file.uri().to_string();
    if info.boolean("thumbnail::failed") || FAILED.with(|f| f.borrow().contains(&uri)) {
        return None;
    }
    let exec =
        THUMBNAILERS.with(|all| all.iter().find(|t| t.handles(&ct)).map(|t| t.exec.clone()))?;
    let _slot = slot("thumbnailer", THUMBNAILER_SLOTS, wanted).await?;
    let mtime = entry::modified(info);
    match run_thumbnailer(&exec, file, mtime).await {
        Some(path) => cached(&path).await,
        None => {
            FAILED.with(|f| f.borrow_mut().insert(uri));
            None
        }
    }
}

struct Thumbnailer {
    exec: String,
    mime: Vec<String>,
}

impl Thumbnailer {
    fn handles(&self, content_type: &str) -> bool {
        self.mime
            .iter()
            .any(|m| m == content_type || gio::content_type_is_a(content_type, m))
    }
}

fn find_thumbnailers() -> Vec<Thumbnailer> {
    let mut dirs = vec![glib::user_data_dir()];
    dirs.extend(glib::system_data_dirs());
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for dir in dirs {
        let Ok(read) = std::fs::read_dir(dir.join("thumbnailers")) else {
            continue;
        };
        let mut files: Vec<_> = read.flatten().map(|e| e.path()).collect();
        files.sort();
        for path in files {
            let Some(name) = path.file_name().map(|n| n.to_owned()) else {
                continue;
            };
            if !seen.insert(name) {
                continue;
            }
            let key = glib::KeyFile::new();
            if key.load_from_file(&path, glib::KeyFileFlags::NONE).is_err() {
                continue;
            }
            let group = "Thumbnailer Entry";
            if let Ok(try_exec) = key.string(group, "TryExec") {
                if glib::find_program_in_path(try_exec.as_str()).is_none() {
                    continue;
                }
            }
            let (Ok(exec), Ok(mime)) = (key.string(group, "Exec"), key.string(group, "MimeType"))
            else {
                continue;
            };
            out.push(Thumbnailer {
                exec: exec.to_string(),
                mime: mime
                    .split(';')
                    .filter(|m| !m.is_empty())
                    .map(str::to_owned)
                    .collect(),
            });
        }
    }
    out
}

/// Where the spec keeps the large thumbnail of `uri`.
pub fn cache_path(uri: &str) -> PathBuf {
    let md5 = glib::compute_checksum_for_string(glib::ChecksumType::Md5, uri).unwrap_or_default();
    glib::user_cache_dir()
        .join("thumbnails")
        .join("large")
        .join(format!("{md5}.png"))
}

/// The thumbnailer's arguments: `%i` the path, `%u` the URI, `%o` the
/// output, `%s` the size, `%%` a percent sign.
fn arguments(exec: &str, input: &str, uri: &str, output: &str, size: i32) -> Option<Vec<String>> {
    let words = glib::shell_parse_argv(exec).ok()?;
    Some(
        words
            .iter()
            .map(|w| {
                let mut out = String::new();
                let w = w.to_string_lossy();
                let mut chars = w.chars();
                while let Some(c) = chars.next() {
                    if c != '%' {
                        out.push(c);
                        continue;
                    }
                    match chars.next() {
                        Some('i') => out.push_str(input),
                        Some('u') => out.push_str(uri),
                        Some('o') => out.push_str(output),
                        Some('s') => out.push_str(&size.to_string()),
                        Some('%') => out.push('%'),
                        Some(other) => {
                            out.push('%');
                            out.push(other);
                        }
                        None => out.push('%'),
                    }
                }
                out
            })
            .collect(),
    )
}

/// Run a thumbnailer and store its picture in the cache, with the URI and
/// modification time the spec asks for.
async fn run_thumbnailer(exec: &str, file: &gio::File, mtime: i64) -> Option<PathBuf> {
    let input = file.path()?;
    let uri = file.uri().to_string();
    let dest = cache_path(&uri);
    let dir = dest.parent()?.to_owned();
    std::fs::create_dir_all(&dir).ok()?;
    // The spec keeps thumbnails private: folders 700, files 600.
    let private = |path: &std::path::Path, mode: u32| {
        let _ = std::fs::set_permissions(path, std::os::unix::fs::PermissionsExt::from_mode(mode));
    };
    private(dir.parent()?, 0o700);
    private(&dir, 0o700);
    let out = glib::user_runtime_dir().join(format!(
        "elavo-files-thumb-{}-{}.png",
        std::process::id(),
        glib::random_int()
    ));
    let args = arguments(
        exec,
        &input.to_string_lossy(),
        &uri,
        &out.to_string_lossy(),
        LARGE,
    )?;
    let argv: Vec<&std::ffi::OsStr> = args.iter().map(std::ffi::OsStr::new).collect();
    let process = gio::Subprocess::newv(
        &argv,
        gio::SubprocessFlags::STDOUT_SILENCE | gio::SubprocessFlags::STDERR_SILENCE,
    )
    .ok()?;
    let fired = std::rc::Rc::new(std::cell::Cell::new(false));
    let (p, f) = (process.clone(), fired.clone());
    let timer = glib::timeout_add_local_once(THUMBNAILER_TIMEOUT, move || {
        f.set(true);
        p.force_exit();
    });
    let ok = process.wait_check_future().await.is_ok();
    if !fired.get() {
        timer.remove();
    }
    let result = (|| {
        if !ok {
            return None;
        }
        let pixbuf = gtk::gdk_pixbuf::Pixbuf::from_file(&out).ok()?;
        let pixbuf = if pixbuf.width().max(pixbuf.height()) > LARGE {
            let (w, h) = (pixbuf.width() as f64, pixbuf.height() as f64);
            let scale = LARGE as f64 / w.max(h);
            pixbuf.scale_simple(
                ((w * scale).round() as i32).max(1),
                ((h * scale).round() as i32).max(1),
                gtk::gdk_pixbuf::InterpType::Bilinear,
            )?
        } else {
            pixbuf
        };
        let tmp = dir.join(format!(".elavo-files-{}.png", glib::random_int()));
        pixbuf
            .savev(
                &tmp,
                "png",
                &[
                    ("tEXt::Thumb::URI", uri.as_str()),
                    ("tEXt::Thumb::MTime", &mtime.to_string()),
                    ("tEXt::Software", "Elavo Files"),
                ],
            )
            .ok()?;
        private(&tmp, 0o600);
        std::fs::rename(&tmp, &dest).ok()?;
        Some(dest.clone())
    })();
    let _ = std::fs::remove_file(&out);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thumbnailer_arguments() {
        let args = arguments(
            "thumb -s %s %u '%o' 100%%",
            "/a b.mp4",
            "file:///a%20b.mp4",
            "/tmp/o.png",
            256,
        )
        .unwrap();
        assert_eq!(
            args,
            [
                "thumb",
                "-s",
                "256",
                "file:///a%20b.mp4",
                "/tmp/o.png",
                "100%"
            ]
        );
        let args = arguments("t %i %o", "/x y", "file:///x%20y", "/o", 256).unwrap();
        assert_eq!(args, ["t", "/x y", "/o"]);
    }

    #[test]
    fn cache_path_follows_the_spec() {
        // md5("file:///home/jens/photos/me.png"), the spec's example.
        let p = cache_path("file:///home/jens/photos/me.png");
        assert!(p.ends_with("thumbnails/large/c6ee772d9e49320e97ec29a7eb5b1697.png"));
    }
}
