//! `desk cursor`: the pointer's global position as KWin knows it, printed as
//! "X Y" lines whenever it changes. Wayland tells a client where the pointer
//! is only over the client's own surfaces; KWin's scripting knows it
//! everywhere (workspace.cursorPos). So a small KWin script is loaded for
//! the duration of this command; on a timer it hands the position to this
//! process over D-Bus (service org.kde.desk, object /cursor, as a string:
//! KWin's callDBus types numbers at its own discretion, a string is
//! unambiguous), and the script is unloaded when this process ends.

use anyhow::{Context, Result};
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use zbus::blocking::{connection::Builder, Connection};

const KWIN: &str = "org.kde.KWin";
const SCRIPTING: &str = "/Scripting";
const SCRIPTING_IFACE: &str = "org.kde.kwin.Scripting";
const PLUGIN: &str = "desk-cursor";

struct Cursor {
    last: Mutex<(i64, i64)>,
}

#[zbus::interface(name = "org.kde.desk.cursor")]
impl Cursor {
    /// "X Y", from the KWin script (named in lower case on the bus too).
    #[zbus(name = "pos")]
    fn pos(&self, xy: &str) {
        let mut it = xy.split_whitespace();
        let x = it.next().and_then(|v| v.parse::<f64>().ok());
        let y = it.next().and_then(|v| v.parse::<f64>().ok());
        if let (Some(x), Some(y)) = (x, y) {
            let p = (x.round() as i64, y.round() as i64);
            let mut last = self.last.lock().unwrap();
            if *last != p {
                *last = p;
                let out = std::io::stdout();
                let mut o = out.lock();
                let _ = writeln!(o, "{} {}", p.0, p.1);
                let _ = o.flush();
            }
        }
    }
}

fn script_text(interval_ms: u32) -> String {
    format!(
        "const t = new QTimer();\n\
         t.interval = {interval_ms};\n\
         t.timeout.connect(function () {{\n\
             const p = workspace.cursorPos;\n\
             callDBus(\"org.kde.desk\", \"/cursor\", \"org.kde.desk.cursor\", \"pos\", p.x + \" \" + p.y);\n\
         }});\n\
         t.start();\n"
    )
}

fn unload(conn: &Connection) {
    let _ = conn.call_method(Some(KWIN), SCRIPTING, Some(SCRIPTING_IFACE), "unloadScript", &(PLUGIN,));
}

pub fn run(interval_ms: u32) -> Result<()> {
    let conn = Builder::session()
        .context("session bus")?
        .name("org.kde.desk")
        .context("the name org.kde.desk is taken (another desk cursor running?)")?
        .serve_at("/cursor", Cursor { last: Mutex::new((i64::MIN, i64::MIN)) })
        .context("serve /cursor")?
        .build()
        .context("D-Bus service")?;

    let dir = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".into());
    let dir = std::path::PathBuf::from(dir).join("desk");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("cursor.js");
    std::fs::write(&path, script_text(interval_ms))?;

    unload(&conn); // a leftover from an earlier run
    let reply = conn
        .call_method(Some(KWIN), SCRIPTING, Some(SCRIPTING_IFACE), "loadScript", &(path.to_string_lossy().to_string(), PLUGIN))
        .context("KWin loadScript")?;
    let id: i32 = reply.body().deserialize().context("script id")?;
    if id < 0 {
        anyhow::bail!("KWin refused the cursor script");
    }
    let script_path = format!("/Scripting/Script{id}");
    conn.call_method(Some(KWIN), script_path.as_str(), Some("org.kde.kwin.Script"), "run", &())
        .context("KWin run script")?;
    eprintln!("desk cursor: KWin script {id} running every {interval_ms} ms; Ctrl-C stops");

    // Until a signal: the interface runs on the connection's own thread.
    let stop = Arc::new(AtomicBool::new(false));
    for sig in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP, libc::SIGPIPE] {
        let s = stop.clone();
        // A tiny handler through a static flag: libc signal with a plain fn.
        unsafe { libc::signal(sig, handler as libc::sighandler_t) };
        let _ = s;
    }
    while !STOP.load(Ordering::SeqCst) {
        std::thread::sleep(std::time::Duration::from_millis(100));
        // A reader that went away (the pipe closed) ends this too.
        if unsafe { libc::fcntl(1, libc::F_GETFD) } < 0 {
            break;
        }
    }
    unload(&conn);
    Ok(())
}

static STOP: AtomicBool = AtomicBool::new(false);
extern "C" fn handler(_: libc::c_int) {
    STOP.store(true, Ordering::SeqCst);
}
