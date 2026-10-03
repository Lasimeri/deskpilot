//! Installs the desktop file that authorizes this binary for KWin's
//! restricted interfaces. KWin resolves the caller's /proc/<pid>/exe and
//! matches it against Exec of desktop entries carrying the relevant keys:
//! X-KDE-DBUS-Restricted-Interfaces gates the ScreenShot2 D-Bus interface,
//! X-KDE-Wayland-Interfaces gates restricted Wayland globals (fake input).
//! Exec must be this binary's absolute path; sycoca rebuild required after.

use anyhow::{Context, Result};
use std::process::Command;

pub fn run() -> Result<()> {
    let exe = std::env::current_exe()?
        .canonicalize()
        .context("resolve current executable path")?;
    let home = std::env::var("HOME").context("HOME not set")?;
    let dir = format!("{home}/.local/share/applications");
    std::fs::create_dir_all(&dir)?;
    let path = format!("{dir}/desk.desktop");
    let body = format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=desk\n\
         Comment=Claude Code desktop control (screenshot authorization shim)\n\
         Exec={}\n\
         Icon=utilities-terminal\n\
         NoDisplay=true\n\
         X-KDE-DBUS-Restricted-Interfaces=org.kde.KWin.ScreenShot2\n\
         X-KDE-Wayland-Interfaces=org_kde_kwin_fake_input\n",
        exe.display()
    );
    std::fs::write(&path, body).with_context(|| format!("write {path}"))?;
    println!("wrote {path} (Exec={})", exe.display());
    match Command::new("kbuildsycoca6").output() {
        Ok(o) if o.status.success() => println!("kbuildsycoca6 ok"),
        Ok(o) => println!(
            "kbuildsycoca6 exited {}: {}",
            o.status,
            String::from_utf8_lossy(&o.stderr).trim()
        ),
        Err(e) => println!("kbuildsycoca6 not run ({e}); run it manually"),
    }
    println!("note: re-run `desk setup` if the binary moves (debug vs release path matters)");
    Ok(())
}
