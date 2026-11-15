//! The computer's name, as LocalSend shows it to other devices.
//!
//! Elavo System Settings names the computer through systemd-hostnamed
//! (General → About → Name): `SetPrettyHostname` with what was typed, and
//! `SetStaticHostname` with a network-safe form of it. LocalSend shows the
//! pretty name, falling back to the static host name, then the current one.
//! The app reads them from `org.freedesktop.hostname1` and follows its
//! `PropertiesChanged`.

use gio::glib;

pub const HOSTNAME_BUS_NAME: &str = "org.freedesktop.hostname1";
pub const HOSTNAME_PATH: &str = "/org/freedesktop/hostname1";
pub const HOSTNAME_INTERFACE: &str = "org.freedesktop.hostname1";

/// The first of hostnamed's names that says something.
pub fn pick(pretty: &str, static_name: &str, hostname: &str) -> Option<String> {
    [pretty, static_name, hostname]
        .into_iter()
        .map(str::trim)
        .find(|n| !n.is_empty() && *n != "localhost" && *n != "localhost.localdomain")
        .map(str::to_owned)
}

/// Before hostnamed answers, or without it: the host name, or Elavo.
pub fn fallback() -> String {
    pick("", "", &glib::host_name()).unwrap_or_else(|| "Elavo".to_owned())
}

/// The name from hostnamed's `GetAll` reply, `(a{sv})`.
pub fn from_properties(reply: &glib::Variant) -> Option<String> {
    let props = reply.try_child_value(0)?;
    let dict = glib::VariantDict::new(Some(&props));
    let get = |k: &str| {
        dict.lookup_value(k, Some(glib::VariantTy::STRING))
            .and_then(|v| v.get::<String>())
            .unwrap_or_default()
    };
    pick(
        &get("PrettyHostname"),
        &get("StaticHostname"),
        &get("Hostname"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefers_the_pretty_name() {
        assert_eq!(
            pick("Kevin's Dell", "Kevins-Dell", "fedora").as_deref(),
            Some("Kevin's Dell")
        );
        assert_eq!(
            pick(" ", "Kevins-Dell", "fedora").as_deref(),
            Some("Kevins-Dell")
        );
        assert_eq!(pick("", "", "fedora").as_deref(), Some("fedora"));
        assert_eq!(pick("", "localhost", "localhost"), None);
        assert!(!fallback().is_empty());
    }

    #[test]
    fn reads_hostnamed_properties() {
        let dict = glib::VariantDict::new(None);
        dict.insert("PrettyHostname", "Studio Laptop");
        dict.insert("StaticHostname", "Studio-Laptop");
        dict.insert("Chassis", "laptop");
        let reply = glib::Variant::tuple_from_iter([dict.end()]);
        assert_eq!(from_properties(&reply).as_deref(), Some("Studio Laptop"));
        let dict = glib::VariantDict::new(None);
        dict.insert("PrettyHostname", "");
        dict.insert("StaticHostname", "Studio-Laptop");
        let reply = glib::Variant::tuple_from_iter([dict.end()]);
        assert_eq!(from_properties(&reply).as_deref(), Some("Studio-Laptop"));
    }
}
