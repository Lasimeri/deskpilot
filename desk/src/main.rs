mod cast;
mod keys;
mod mcp;
mod setup;
mod shot;
mod wl;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::thread::sleep;
use std::time::Duration;

#[derive(Parser)]
#[command(name = "desk", version, about = "KWin Wayland desktop control: atomic dual-monitor screenshots + virtual mouse/keyboard")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Install KWin ScreenShot2 authorization (desktop file + sycoca rebuild)
    Setup,
    /// List enabled outputs: name, logical geometry, scale
    Outputs,
    /// Screenshot. Default: one atomic workspace grab saved whole + split per monitor
    Shot {
        /// Output directory (default ~/.cache/desk-shots)
        #[arg(long)]
        dir: Option<PathBuf>,
        /// Omit the pointer from the capture
        #[arg(long)]
        no_cursor: bool,
        /// Capture a single output by name (e.g. DP-4) instead of the workspace
        #[arg(long)]
        screen: Option<String>,
        /// Capture a rectangle in global coordinates: X Y W H
        #[arg(long, num_args = 4, value_names = ["X", "Y", "W", "H"], allow_hyphen_values = true)]
        region: Option<Vec<i64>>,
        /// Skip the per-monitor crops, save only the combined image
        #[arg(long)]
        no_split: bool,
    },
    /// Virtual pointer
    Mouse {
        #[command(subcommand)]
        cmd: MouseCmd,
    },
    /// Move pointer to X Y and click: shorthand for the common case
    Click {
        x: f64,
        y: f64,
        #[arg(default_value = "left")]
        button: String,
        #[arg(long)]
        double: bool,
    },
    /// Tap key combos in sequence, e.g. `desk key ctrl+shift+t f5 enter`
    Key {
        #[arg(required = true)]
        combos: Vec<String>,
        #[arg(long, default_value_t = 30)]
        delay_ms: u64,
    },
    /// Print "idle" or "active" depending on seat idle time vs threshold
    Idle {
        #[arg(long, default_value_t = 600_000)]
        threshold_ms: u32,
    },
    /// The desktop as an MCP server on stdin/stdout (for Claude Code: claude mcp add desk -- desk mcp)
    Mcp,
    /// A live screencast of one monitor (the portal's PipeWire stream) handed to COMMAND,
    /// with @FD@ and @NODE@ substituted in its arguments; e.g.
    /// desk cast -- gst-launch-1.0 pipewiresrc fd=@FD@ path=@NODE@ ! ...
    Cast {
        /// Leave the pointer out of the stream
        #[arg(long)]
        no_cursor: bool,
        /// Drop the remembered permission and ask again
        #[arg(long)]
        forget: bool,
        /// The command that consumes the stream
        #[arg(last = true, required = true)]
        command: Vec<String>,
    },
    /// Type literal text (US layout)
    Type {
        text: String,
        #[arg(long, default_value_t = 8)]
        delay_ms: u64,
        /// Press Enter afterwards
        #[arg(long)]
        enter: bool,
    },
}

#[derive(Subcommand)]
enum MouseCmd {
    /// Absolute move in global compositor coordinates
    Move { x: f64, y: f64 },
    Click {
        #[arg(default_value = "left")]
        button: String,
        #[arg(long, num_args = 2, value_names = ["X", "Y"])]
        at: Option<Vec<f64>>,
        #[arg(long)]
        double: bool,
    },
    Down {
        #[arg(default_value = "left")]
        button: String,
    },
    Up {
        #[arg(default_value = "left")]
        button: String,
    },
    /// Scroll by wheel notches; positive = down/right
    Scroll {
        #[arg(allow_hyphen_values = true)]
        notches: f64,
        #[arg(long)]
        horizontal: bool,
    },
    /// Press-move-release from X1 Y1 to X2 Y2
    Drag {
        x1: f64,
        y1: f64,
        x2: f64,
        y2: f64,
        #[arg(long, default_value_t = 400)]
        ms: u64,
        #[arg(long, default_value = "left")]
        button: String,
    },
}

fn ensure_env() {
    if std::env::var_os("XDG_RUNTIME_DIR").is_none() {
        std::env::set_var("XDG_RUNTIME_DIR", format!("/run/user/{}", unsafe { libc::getuid() }));
    }
    if std::env::var_os("WAYLAND_DISPLAY").is_none() {
        std::env::set_var("WAYLAND_DISPLAY", "wayland-0");
    }
}

fn main() -> Result<()> {
    ensure_env();
    match Cli::parse().cmd {
        Cmd::Setup => setup::run(),
        Cmd::Outputs => outputs(),
        Cmd::Shot { dir, no_cursor, screen, region, no_split } => do_shot(dir, !no_cursor, screen, region, no_split),
        Cmd::Mouse { cmd } => mouse(cmd),
        Cmd::Click { x, y, button, double } => {
            let mut d = wl::Desktop::connect()?;
            d.mouse_move(x, y)?;
            sleep(Duration::from_millis(60));
            d.click(wl::button_code(&button)?, double)?;
            println!("clicked {button} at {x},{y}");
            Ok(())
        }
        Cmd::Key { combos, delay_ms } => key_combos(combos, delay_ms),
        Cmd::Idle { threshold_ms } => {
            let mut d = wl::Desktop::connect()?;
            println!("{}", if d.idle_check(threshold_ms)? { "idle" } else { "active" });
            Ok(())
        }
        Cmd::Type { text, delay_ms, enter } => type_text(&text, delay_ms, enter),
        Cmd::Mcp => mcp::run(),
        Cmd::Cast { no_cursor, forget, command } => {
            let code = cast::run(!no_cursor, forget, command)?;
            std::process::exit(code);
        }
    }
}

fn outputs() -> Result<()> {
    let d = wl::Desktop::connect()?;
    let outs = d.outputs();
    if outs.is_empty() {
        bail!("no enabled outputs reported");
    }
    for o in &outs {
        println!(
            "{}: {}x{} at {},{} (scale {})",
            o.name,
            o.lw(),
            o.lh(),
            o.x,
            o.y,
            o.scale
        );
    }
    let (bx, by, bw, bh) = bbox(&outs);
    println!("workspace: {bw}x{bh} origin {bx},{by}");
    Ok(())
}

fn bbox(outs: &[wl::Output]) -> (i32, i32, i32, i32) {
    let minx = outs.iter().map(|o| o.x).min().unwrap_or(0);
    let miny = outs.iter().map(|o| o.y).min().unwrap_or(0);
    let maxx = outs.iter().map(|o| o.x + o.lw()).max().unwrap_or(0);
    let maxy = outs.iter().map(|o| o.y + o.lh()).max().unwrap_or(0);
    (minx, miny, maxx - minx, maxy - miny)
}

fn shot_dir(dir: Option<PathBuf>) -> Result<PathBuf> {
    let dir = match dir {
        Some(d) => d,
        None => PathBuf::from(std::env::var("HOME").context("HOME not set")?).join(".cache/desk-shots"),
    };
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

fn stamp() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap();
    format!("{}{:03}", now.as_secs(), now.subsec_millis())
}

fn do_shot(dir: Option<PathBuf>, cursor: bool, screen: Option<String>, region: Option<Vec<i64>>, no_split: bool) -> Result<()> {
    let dir = shot_dir(dir)?;
    let ts = stamp();

    if let Some(r) = region {
        let (x, y, w, h) = (r[0] as i32, r[1] as i32, r[2] as u32, r[3] as u32);
        let s = shot::capture_area(x, y, w, h, cursor)?;
        let p = dir.join(format!("shot-{ts}-region.png"));
        s.save_png(&p)?;
        println!("{} ({}x{} at {x},{y})", p.display(), s.width, s.height);
        return Ok(());
    }

    if let Some(name) = screen {
        let s = shot::capture_screen(&name, cursor)?;
        let p = dir.join(format!("shot-{ts}-{name}.png"));
        s.save_png(&p)?;
        println!("{} ({}x{})", p.display(), s.width, s.height);
        return Ok(());
    }

    let d = wl::Desktop::connect()?;
    let outs = d.outputs();
    let ws = shot::capture_workspace(cursor)?;
    let (bx, by, bw, bh) = bbox(&outs);

    let all = dir.join(format!("shot-{ts}-all.png"));
    ws.save_png(&all)?;
    println!("{} ({}x{}, workspace origin {bx},{by})", all.display(), ws.width, ws.height);

    if !no_split && !outs.is_empty() {
        // map logical workspace rects onto the captured image (identical when scale is 1)
        let sx = ws.width as f64 / bw.max(1) as f64;
        let sy = ws.height as f64 / bh.max(1) as f64;
        for o in &outs {
            let cx = ((o.x - bx) as f64 * sx).round() as u32;
            let cy = ((o.y - by) as f64 * sy).round() as u32;
            let cw = (o.lw() as f64 * sx).round() as u32;
            let ch = (o.lh() as f64 * sy).round() as u32;
            match ws.crop(cx, cy, cw.min(ws.width - cx), ch.min(ws.height - cy)) {
                Some(c) => {
                    let p = dir.join(format!("shot-{ts}-{}.png", o.name));
                    c.save_png(&p)?;
                    println!("{} ({}x{}, {} at {},{})", p.display(), c.width, c.height, o.name, o.x, o.y);
                }
                None => println!("{}: crop out of bounds, skipped", o.name),
            }
        }
    }
    Ok(())
}

fn mouse(cmd: MouseCmd) -> Result<()> {
    let mut d = wl::Desktop::connect()?;
    match cmd {
        MouseCmd::Move { x, y } => {
            d.mouse_move(x, y)?;
            println!("pointer at {x},{y}");
        }
        MouseCmd::Click { button, at, double } => {
            if let Some(p) = &at {
                d.mouse_move(p[0], p[1])?;
                sleep(Duration::from_millis(60));
            }
            d.click(wl::button_code(&button)?, double)?;
            println!("clicked {button}{}", at.map(|p| format!(" at {},{}", p[0], p[1])).unwrap_or_default());
        }
        MouseCmd::Down { button } => {
            d.button(wl::button_code(&button)?, true)?;
            println!("{button} down");
        }
        MouseCmd::Up { button } => {
            d.button(wl::button_code(&button)?, false)?;
            println!("{button} up");
        }
        MouseCmd::Scroll { notches, horizontal } => {
            d.scroll(notches, horizontal)?;
            println!("scrolled {notches} notch(es){}", if horizontal { " horizontally" } else { "" });
        }
        MouseCmd::Drag { x1, y1, x2, y2, ms, button } => {
            d.drag(x1, y1, x2, y2, wl::button_code(&button)?, ms)?;
            println!("dragged {button} {x1},{y1} -> {x2},{y2} over {ms}ms");
        }
    }
    Ok(())
}

fn key_combos(combos: Vec<String>, delay_ms: u64) -> Result<()> {
    let mut d = wl::Desktop::connect()?;
    for combo in &combos {
        let (mods, code) = keys::parse_combo(combo)?;
        for m in &mods {
            d.key(*m, true)?;
            sleep(Duration::from_millis(8));
        }
        d.key(code, true)?;
        sleep(Duration::from_millis(20));
        d.key(code, false)?;
        for m in mods.iter().rev() {
            sleep(Duration::from_millis(8));
            d.key(*m, false)?;
        }
        sleep(Duration::from_millis(delay_ms));
    }
    println!("sent: {}", combos.join(" "));
    Ok(())
}

fn type_text(text: &str, delay_ms: u64, enter: bool) -> Result<()> {
    let events = keys::text_events(text)?;
    let mut d = wl::Desktop::connect()?;
    for (code, shift) in events {
        if shift {
            d.key(keys::KEY_LEFTSHIFT, true)?;
            sleep(Duration::from_millis(4));
        }
        d.key(code, true)?;
        sleep(Duration::from_millis(6));
        d.key(code, false)?;
        if shift {
            sleep(Duration::from_millis(4));
            d.key(keys::KEY_LEFTSHIFT, false)?;
        }
        sleep(Duration::from_millis(delay_ms));
    }
    if enter {
        sleep(Duration::from_millis(30));
        d.key(28, true)?;
        sleep(Duration::from_millis(20));
        d.key(28, false)?;
    }
    println!("typed {} char(s){}", text.chars().count(), if enter { " + enter" } else { "" });
    Ok(())
}
