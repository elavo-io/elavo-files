//! What goes over the wire, and the rules for names received.

use gio::glib;
use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

/// A device describing itself: in announcements, `register`, `info` and
/// as the `info` of `prepare-upload`. Version 1 peers leave most of it out.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Info {
    pub alias: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fingerprint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub download: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub announce: Option<bool>,
    /// Version 1's name for `announce`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub announcement: Option<bool>,
}

impl Info {
    pub fn wants_answer(&self) -> bool {
        self.announce.or(self.announcement).unwrap_or(false)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DeviceType {
    Mobile,
    #[default]
    Desktop,
    Web,
    Headless,
    Server,
}

impl DeviceType {
    /// Unknown kinds read as desktop, as in the official app.
    pub fn parse(s: Option<&str>) -> Self {
        match s.unwrap_or("") {
            "mobile" => Self::Mobile,
            "web" => Self::Web,
            "headless" => Self::Headless,
            "server" => Self::Server,
            _ => Self::Desktop,
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Self::Mobile => "mobile",
            Self::Desktop => "desktop",
            Self::Web => "web",
            Self::Headless => "headless",
            Self::Server => "server",
        }
    }
}

/// A device nearby, as Files knows it.
#[derive(Clone, Debug, PartialEq)]
pub struct Device {
    pub alias: String,
    pub model: Option<String>,
    pub kind: DeviceType,
    pub fingerprint: String,
    pub ip: String,
    pub port: u16,
    pub https: bool,
    pub version: String,
}

impl Device {
    /// `info` as heard from `ip`. In HTTPS the fingerprint is the one of
    /// the certificate it showed, when there was one.
    pub fn from_info(info: &Info, ip: &str, cert_fingerprint: Option<&str>) -> Option<Self> {
        let fingerprint = cert_fingerprint
            .map(str::to_owned)
            .or_else(|| info.fingerprint.clone())
            .filter(|f| !f.is_empty())?;
        let version = info.version.clone().unwrap_or_else(|| "1.0".to_owned());
        let https = match info.protocol.as_deref() {
            Some(p) => p == "https",
            // Version 1 had no choice: always HTTPS.
            None => true,
        };
        Some(Self {
            alias: if info.alias.trim().is_empty() {
                ip.to_owned()
            } else {
                info.alias.trim().to_owned()
            },
            model: info.device_model.clone().filter(|m| !m.is_empty()),
            kind: DeviceType::parse(info.device_type.as_deref()),
            fingerprint,
            ip: ip.to_owned(),
            port: info.port.unwrap_or(super::DEFAULT_PORT),
            https,
            version,
        })
    }

    pub fn is_v1(&self) -> bool {
        self.version.starts_with('1')
    }

    /// The path of `route` in the API this device speaks.
    pub fn api(&self, route: &str) -> String {
        let v = if self.is_v1() { "v1" } else { "v2" };
        format!("/api/localsend/{v}/{route}")
    }

    /// The same fingerprint, whatever the case it was written in.
    pub fn same_fingerprint(&self, other: &str) -> bool {
        self.fingerprint.eq_ignore_ascii_case(other)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Metadata {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modified: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accessed: Option<String>,
}

/// A file offered in `prepare-upload`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileMeta {
    pub id: String,
    pub file_name: String,
    pub size: u64,
    /// A MIME type in version 2; image, video, pdf, text, apk or other in 1.
    #[serde(default)]
    pub file_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Metadata>,
}

impl FileMeta {
    /// A text message rather than a file: sent with its text as preview.
    pub fn is_message(&self) -> bool {
        self.preview.is_some() && (self.file_type == "text/plain" || self.file_type == "text")
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PrepareUpload {
    pub info: Info,
    pub files: BTreeMap<String, FileMeta>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrepareResponse {
    pub session_id: String,
    pub files: BTreeMap<String, String>,
}

/// Version 1's file kinds, from a MIME type.
pub fn v1_file_type(mime: &str) -> &'static str {
    match mime {
        m if m.starts_with("image/") => "image",
        m if m.starts_with("video/") => "video",
        "application/pdf" => "pdf",
        "application/vnd.android.package-archive" => "apk",
        m if m.starts_with("text/") => "text",
        _ => "other",
    }
}

/// Where a received `file_name` may go under the save folder: its folders
/// kept, but nothing absolute, no `..`, and no empty or hidden-dot tricks.
pub fn safe_relative(name: &str) -> Option<PathBuf> {
    let mut out = PathBuf::new();
    for part in name.split(['/', '\\']) {
        let part = part.trim_matches(|c: char| c.is_control());
        if part.is_empty() || part == "." {
            continue;
        }
        if part == ".." || part.contains('\0') {
            return None;
        }
        match Path::new(part).components().next() {
            Some(Component::Normal(_)) => out.push(part),
            _ => return None,
        }
    }
    (!out.as_os_str().is_empty()).then_some(out)
}

const ADJECTIVES: [&str; 24] = [
    "Adorable",
    "Beautiful",
    "Big",
    "Bright",
    "Clean",
    "Clever",
    "Cool",
    "Cute",
    "Cunning",
    "Determined",
    "Energetic",
    "Efficient",
    "Fantastic",
    "Fast",
    "Fine",
    "Fresh",
    "Good",
    "Gorgeous",
    "Great",
    "Handsome",
    "Hot",
    "Kind",
    "Lovely",
    "Nice",
];
const FRUITS: [&str; 20] = [
    "Apple",
    "Avocado",
    "Banana",
    "Blackberry",
    "Blueberry",
    "Broccoli",
    "Carrot",
    "Cherry",
    "Coconut",
    "Grape",
    "Lemon",
    "Lettuce",
    "Mango",
    "Melon",
    "Mushroom",
    "Onion",
    "Orange",
    "Papaya",
    "Peach",
    "Pear",
];

/// A name like the official app's first one: an adjective and a fruit.
pub fn random_alias() -> String {
    let a = ADJECTIVES[glib::random_int_range(0, ADJECTIVES.len() as i32) as usize];
    let f = FRUITS[glib::random_int_range(0, FRUITS.len() as i32) as usize];
    format!("{a} {f}")
}

/// A name `random_alias` made: what Files picked before LocalSend used the
/// computer's name, so it can be dropped.
pub fn is_random_alias(name: &str) -> bool {
    let mut words = name.split(' ');
    matches!(
        (words.next(), words.next(), words.next()),
        (Some(a), Some(f), None) if ADJECTIVES.contains(&a) && FRUITS.contains(&f)
    )
}

/// A random id or token.
pub fn random_id() -> String {
    glib::uuid_string_random().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_v2_and_v1_announcements() {
        let v2: Info = serde_json::from_str(
            r#"{"alias":"Nice Orange","version":"2.0","deviceModel":"Samsung",
            "deviceType":"mobile","fingerprint":"AB","port":53318,"protocol":"https",
            "download":true,"announce":true}"#,
        )
        .unwrap();
        assert!(v2.wants_answer());
        let d = Device::from_info(&v2, "192.168.1.5", None).unwrap();
        assert_eq!(d.kind, DeviceType::Mobile);
        assert_eq!((d.port, d.https, d.is_v1()), (53318, true, false));
        assert_eq!(d.api("register"), "/api/localsend/v2/register");
        let v1: Info = serde_json::from_str(
            r#"{"alias":"Old","deviceModel":null,"deviceType":"tablet",
            "fingerprint":"x","announcement":true}"#,
        )
        .unwrap();
        assert!(v1.wants_answer());
        let d = Device::from_info(&v1, "10.0.0.2", Some("CERT")).unwrap();
        assert_eq!(d.fingerprint, "CERT");
        assert_eq!(d.kind, DeviceType::Desktop);
        assert_eq!(d.api("send-request"), "/api/localsend/v1/send-request");
        // Optional fields stay out of what is sent.
        let ours = Info {
            alias: "A".into(),
            ..Default::default()
        };
        assert_eq!(serde_json::to_string(&ours).unwrap(), r#"{"alias":"A"}"#);
    }

    #[test]
    fn keeps_received_names_inside_the_folder() {
        assert_eq!(safe_relative("a.txt"), Some("a.txt".into()));
        assert_eq!(safe_relative("dir/sub/a.txt"), Some("dir/sub/a.txt".into()));
        assert_eq!(safe_relative("/etc/passwd"), Some("etc/passwd".into()));
        assert_eq!(safe_relative("..\\x"), None);
        assert_eq!(safe_relative("a/../../x"), None);
        assert_eq!(safe_relative("./"), None);
        assert_eq!(v1_file_type("image/png"), "image");
        assert_eq!(v1_file_type("application/zip"), "other");
        assert!(random_alias().contains(' '));
        assert!(is_random_alias(&random_alias()));
        assert!(is_random_alias("Efficient Avocado"));
        assert!(!is_random_alias("Kevin's Dell"));
        assert!(!is_random_alias("Efficient Avocado 2"));
        assert!(!is_random_alias("efficient avocado"));
    }
}
