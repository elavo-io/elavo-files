//! A secret in the person's keyring, through the Secret Service on the
//! session bus (GNOME Keyring in Elavo), the same store libsecret uses.
//!
//! Only the "plain" transport is used: the secret crosses the session bus,
//! which only this user can reach. A locked keyring is not unlocked here;
//! Elavo unlocks the login keyring at login.
//!
//! LocalSend's PIN is kept with the attributes `xdg:schema` and
//! `setting`, so `secret-tool lookup xdg:schema io.elavo.Sharing setting
//! localsend-pin` finds the same item.

use std::collections::HashMap;

use gio::glib;
use gio::prelude::*;
use glib::variant::ObjectPath;

const SECRETS: &str = "org.freedesktop.secrets";
const SERVICE_PATH: &str = "/org/freedesktop/secrets";
const SERVICE: &str = "org.freedesktop.Secret.Service";
const DEFAULT_COLLECTION: &str = "/org/freedesktop/secrets/aliases/default";
/// The schema every Elavo sharing secret is filed under.
pub const SCHEMA: &str = "io.elavo.Sharing";

type Result<T> = std::result::Result<T, String>;

fn call(
    conn: &gio::DBusConnection,
    path: &str,
    iface: &str,
    method: &str,
    args: glib::Variant,
) -> Result<glib::Variant> {
    conn.call_sync(
        Some(SECRETS),
        path,
        iface,
        method,
        Some(&args),
        None,
        gio::DBusCallFlags::NONE,
        5000,
        gio::Cancellable::NONE,
    )
    .map_err(|e| e.message().to_owned())
}

fn attributes(setting: &str) -> HashMap<String, String> {
    HashMap::from([
        ("xdg:schema".to_owned(), SCHEMA.to_owned()),
        ("setting".to_owned(), setting.to_owned()),
    ])
}

fn bus() -> Result<gio::DBusConnection> {
    gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE)
        .map_err(|e| e.message().to_owned())
}

fn open_session(conn: &gio::DBusConnection) -> Result<ObjectPath> {
    let reply = call(
        conn,
        SERVICE_PATH,
        SERVICE,
        "OpenSession",
        ("plain", "".to_variant()).to_variant(),
    )?;
    reply
        .child_value(1)
        .get::<ObjectPath>()
        .ok_or_else(|| "the keyring did not open a session".to_owned())
}

fn search(conn: &gio::DBusConnection, setting: &str) -> Result<(Vec<ObjectPath>, Vec<ObjectPath>)> {
    let reply = call(
        conn,
        SERVICE_PATH,
        SERVICE,
        "SearchItems",
        (attributes(setting),).to_variant(),
    )?;
    let unlocked = reply.child_value(0).get::<Vec<ObjectPath>>().unwrap_or_default();
    let locked = reply.child_value(1).get::<Vec<ObjectPath>>().unwrap_or_default();
    Ok((unlocked, locked))
}

/// The secret filed under `setting`, if there is one. An error means the
/// keyring could not be asked, or is locked.
pub fn lookup(setting: &str) -> Result<Option<String>> {
    let conn = bus()?;
    let (unlocked, locked) = search(&conn, setting)?;
    let Some(item) = unlocked.into_iter().next() else {
        if locked.is_empty() {
            return Ok(None);
        }
        return Err("the keyring is locked".to_owned());
    };
    let session = open_session(&conn)?;
    let reply = call(
        &conn,
        SERVICE_PATH,
        SERVICE,
        "GetSecrets",
        (vec![item.clone()], session.clone()).to_variant(),
    );
    close(&conn, &session);
    let secrets = reply?.child_value(0);
    let Some(entry) = (0..secrets.n_children())
        .map(|i| secrets.child_value(i))
        .find(|e| e.child_value(0).str() == Some(item.as_str()))
        .map(|e| e.child_value(1))
    else {
        return Ok(None);
    };
    let (_session, _params, value, _type) = entry
        .get::<(ObjectPath, Vec<u8>, Vec<u8>, String)>()
        .ok_or_else(|| "the keyring sent an unexpected secret".to_owned())?;
    String::from_utf8(value)
        .map(Some)
        .map_err(|_| "the secret is not text".to_owned())
}

/// File `secret` under `setting`, replacing what was there. `label` is what
/// keyring managers such as Seahorse show.
pub fn store(setting: &str, label: &str, secret: &str) -> Result<()> {
    let conn = bus()?;
    let session = open_session(&conn)?;
    let properties: HashMap<String, glib::Variant> = HashMap::from([
        (
            "org.freedesktop.Secret.Item.Label".to_owned(),
            label.to_variant(),
        ),
        (
            "org.freedesktop.Secret.Item.Attributes".to_owned(),
            attributes(setting).to_variant(),
        ),
    ]);
    let value = (
        session.clone(),
        Vec::<u8>::new(),
        secret.as_bytes().to_vec(),
        "text/plain".to_owned(),
    );
    let reply = call(
        &conn,
        DEFAULT_COLLECTION,
        "org.freedesktop.Secret.Collection",
        "CreateItem",
        (properties, value, true).to_variant(),
    );
    close(&conn, &session);
    let prompt = reply?.child_value(1).get::<ObjectPath>();
    match prompt {
        Some(p) if p.as_str() != "/" => {
            Err("the keyring asked to be unlocked; unlock it and try again".to_owned())
        }
        _ => Ok(()),
    }
}

/// Remove what is filed under `setting`.
pub fn clear(setting: &str) -> Result<()> {
    let conn = bus()?;
    let (unlocked, locked) = search(&conn, setting)?;
    if !locked.is_empty() {
        return Err("the keyring is locked".to_owned());
    }
    for item in unlocked {
        call(
            &conn,
            item.as_str(),
            "org.freedesktop.Secret.Item",
            "Delete",
            ().to_variant(),
        )?;
    }
    Ok(())
}

fn close(conn: &gio::DBusConnection, session: &ObjectPath) {
    let _ = call(
        conn,
        session.as_str(),
        "org.freedesktop.Secret.Session",
        "Close",
        ().to_variant(),
    );
}
