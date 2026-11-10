//! gettext through glibc. The textdomain is the application id, as
//! planning/development/translations.md requires. English is the source
//! language; `po/` holds catalogs when they exist.

use std::ffi::{c_char, CString};

use gtk::glib;

pub const DOMAIN: &str = crate::APP_ID;

const LOCALEDIR: &str = match option_env!("LOCALEDIR") {
    Some(dir) => dir,
    None => "/usr/share/locale",
};

extern "C" {
    fn bindtextdomain(domain: *const c_char, dir: *const c_char) -> *mut c_char;
    fn bind_textdomain_codeset(domain: *const c_char, codeset: *const c_char) -> *mut c_char;
}

pub fn init() {
    let (Ok(domain), Ok(dir), Ok(utf8)) = (
        CString::new(DOMAIN),
        CString::new(LOCALEDIR),
        CString::new("UTF-8"),
    ) else {
        return;
    };
    unsafe {
        libc::setlocale(libc::LC_ALL, c"".as_ptr());
        bindtextdomain(domain.as_ptr(), dir.as_ptr());
        bind_textdomain_codeset(domain.as_ptr(), utf8.as_ptr());
    }
}

/// Translate a UI string. `xgettext --keyword=tr` collects these.
pub fn tr(msgid: &str) -> String {
    glib::dgettext(Some(DOMAIN), msgid).into()
}
