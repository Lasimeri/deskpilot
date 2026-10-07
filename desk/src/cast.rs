//! `desk cast`: a live screencast of one monitor through the desktop
//! portal (org.freedesktop.portal.ScreenCast), handed to a child process
//! as a PipeWire stream. The portal is the compositor's own streaming
//! path: every frame KWin composes, as a PipeWire video node, no
//! screenshots. The child (normally gst-launch-1.0 with pipewiresrc) gets
//! the PipeWire connection as an inherited fd and the node id, by
//! substituting @FD@ and @NODE@ in its arguments.
//!
//! The first run shows the portal's share dialog (pick the monitor); the
//! answer is kept as a restore token in ~/.cache/desk/cast.token so later
//! runs start without a dialog. The session lives as long as the child.

use anyhow::{bail, Context, Result};
use std::collections::HashMap;
use std::os::fd::{AsRawFd, OwnedFd};
use std::path::PathBuf;
use std::process::Command;
use zbus::blocking::{Connection, Proxy};
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

const DEST: &str = "org.freedesktop.portal.Desktop";
const PATH: &str = "/org/freedesktop/portal/desktop";
const IFACE: &str = "org.freedesktop.portal.ScreenCast";
const REQUEST: &str = "org.freedesktop.portal.Request";

fn token_file() -> Result<PathBuf> {
    let home = std::env::var("HOME").context("HOME not set")?;
    Ok(PathBuf::from(home).join(".cache/desk/cast.token"))
}

/// The request object path the portal will use for our call: the sender's
/// unique name with the ':' dropped and '.' as '_', then our token.
fn request_path(conn: &Connection, token: &str) -> Result<String> {
    let name = conn.unique_name().context("no unique bus name")?.to_string();
    let sender = name.trim_start_matches(':').replace('.', "_");
    Ok(format!("/org/freedesktop/portal/desktop/request/{sender}/{token}"))
}

/// One portal call: subscribe to its Request.Response first, call, then
/// wait for the response and return its results map.
fn portal_call(conn: &Connection, counter: &mut u32, method: &str, call: impl FnOnce(&Connection, HashMap<&str, Value<'static>>) -> zbus::Result<zbus::Message>) -> Result<HashMap<String, OwnedValue>> {
    *counter += 1;
    let token = format!("desk{}", *counter);
    let mut options: HashMap<&str, Value<'static>> = HashMap::new();
    options.insert("handle_token", Value::from(token.clone()));
    let path = request_path(conn, &token)?;
    let req = Proxy::new(conn, DEST, path.as_str(), REQUEST).context("request proxy")?;
    let mut responses = req.receive_signal("Response").context("subscribe to Response")?;

    let reply = call(conn, options).with_context(|| format!("{method} failed"))?;
    // The portal may answer with another request path than the predicted one
    // (older portals); then listen there instead.
    let actual: OwnedObjectPath = reply.body().deserialize().context("request path in reply")?;
    if actual.as_str() != path {
        let req = Proxy::new(conn, DEST, actual.as_str(), REQUEST).context("request proxy")?;
        responses = req.receive_signal("Response").context("subscribe to Response")?;
    }
    let msg = responses.next().context("portal closed without a Response")?;
    let (code, results): (u32, HashMap<String, OwnedValue>) = msg.body().deserialize().context("Response body")?;
    match code {
        0 => Ok(results),
        1 => bail!("{method}: cancelled in the portal dialog"),
        _ => bail!("{method}: portal answered {code}"),
    }
}

/// Clear FD_CLOEXEC so the child inherits the PipeWire connection.
fn inheritable(fd: &OwnedFd) -> Result<()> {
    let raw = fd.as_raw_fd();
    let flags = unsafe { libc::fcntl(raw, libc::F_GETFD) };
    if flags < 0 || unsafe { libc::fcntl(raw, libc::F_SETFD, flags & !libc::FD_CLOEXEC) } < 0 {
        bail!("fcntl on the PipeWire fd: {}", std::io::Error::last_os_error());
    }
    Ok(())
}

pub fn run(cursor: bool, forget: bool, child: Vec<String>) -> Result<i32> {
    if child.is_empty() {
        bail!("desk cast [--no-cursor] [--forget] -- COMMAND [ARGS...]  (@FD@ and @NODE@ are substituted)");
    }
    let tf = token_file()?;
    if forget {
        let _ = std::fs::remove_file(&tf);
    }
    let restore = std::fs::read_to_string(&tf).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty());

    let conn = Connection::session().context("connect to session bus")?;
    let mut counter = 0u32;

    // CreateSession
    let r = portal_call(&conn, &mut counter, "CreateSession", |c, mut o| {
        o.insert("session_handle_token", Value::from("deskcast"));
        c.call_method(Some(DEST), PATH, Some(IFACE), "CreateSession", &(o,))
    })?;
    let session: String = match r.get("session_handle") {
        Some(v) => String::try_from(v.clone()).context("session_handle is not a string")?,
        None => bail!("CreateSession: no session_handle"),
    };
    let session_path = OwnedObjectPath::try_from(session.as_str()).context("session path")?;

    // SelectSources: one monitor, the cursor embedded or hidden, permission kept.
    let sp = session_path.clone();
    portal_call(&conn, &mut counter, "SelectSources", |c, mut o| {
        o.insert("types", Value::from(1u32));
        o.insert("multiple", Value::from(false));
        o.insert("cursor_mode", Value::from(if cursor { 2u32 } else { 1u32 }));
        o.insert("persist_mode", Value::from(2u32));
        if let Some(t) = &restore {
            o.insert("restore_token", Value::from(t.clone()));
        }
        c.call_method(Some(DEST), PATH, Some(IFACE), "SelectSources", &(sp, o))
    })?;

    // Start: the dialog on the first run, then the stream list.
    let sp = session_path.clone();
    let r = portal_call(&conn, &mut counter, "Start", |c, o| {
        c.call_method(Some(DEST), PATH, Some(IFACE), "Start", &(sp, "", o))
    })?;
    if let Some(t) = r.get("restore_token") {
        if let Ok(t) = String::try_from(t.clone()) {
            if let Some(dir) = tf.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let _ = std::fs::write(&tf, format!("{t}\n"));
        }
    }
    let streams = r.get("streams").context("Start: no streams")?;
    // a(ua{sv}): node id and properties per stream.
    let streams: Vec<(u32, HashMap<String, OwnedValue>)> = Vec::try_from(streams.clone()).context("streams is not a(ua{sv})")?;
    let (node, props) = streams.first().context("Start: empty stream list")?;
    let size = props.get("size").map(|v| format!("{v:?}")).unwrap_or_default();
    eprintln!("desk cast: PipeWire node {node} {size}");

    // The PipeWire connection the node is visible on.
    let reply = conn
        .call_method(Some(DEST), PATH, Some(IFACE), "OpenPipeWireRemote", &(session_path.clone(), HashMap::<&str, Value<'_>>::new()))
        .context("OpenPipeWireRemote failed")?;
    let fd: zbus::zvariant::OwnedFd = reply.body().deserialize().context("OpenPipeWireRemote fd")?;
    let fd: OwnedFd = fd.into();
    inheritable(&fd)?;
    let fd_s = fd.as_raw_fd().to_string();
    let node_s = node.to_string();

    let args: Vec<String> = child.iter().map(|a| a.replace("@FD@", &fd_s).replace("@NODE@", &node_s)).collect();
    eprintln!("desk cast: {}", args.join(" "));
    let status = Command::new(&args[0]).args(&args[1..]).status().with_context(|| format!("run {}", args[0]))?;
    // The session closes with the connection.
    let _ = conn.call_method(Some(DEST), session_path.as_str(), Some("org.freedesktop.portal.Session"), "Close", &());
    Ok(status.code().unwrap_or(1))
}
