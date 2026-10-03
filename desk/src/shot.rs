//! Screenshots via KWin's org.kde.KWin.ScreenShot2 D-Bus interface.
//! Pixel data is streamed by KWin into a pipe we pass as a unix fd.
//! Caller authorization comes from a desktop file listing
//! X-KDE-DBUS-Restricted-Interfaces=org.kde.KWin.ScreenShot2 whose Exec
//! matches this binary's path (see setup.rs).

use anyhow::{bail, Context, Result};
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufWriter, Read};
use std::os::fd::{AsFd, FromRawFd, OwnedFd};
use std::path::Path;
use zbus::blocking::Connection;
use zbus::zvariant::{Fd, Value};

const DEST: &str = "org.kde.KWin";
const PATH: &str = "/org/kde/KWin/ScreenShot2";
const IFACE: &str = "org.kde.KWin.ScreenShot2";

pub struct Shot {
    pub width: u32,
    pub height: u32,
    pub rgb: Vec<u8>, // tightly packed RGB8
}

fn options(cursor: bool) -> HashMap<&'static str, Value<'static>> {
    let mut m = HashMap::new();
    m.insert("include-cursor", Value::from(cursor));
    m.insert("native-resolution", Value::from(false));
    m
}

fn make_pipe() -> Result<(OwnedFd, OwnedFd)> {
    let mut fds = [0i32; 2];
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } != 0 {
        bail!("pipe2 failed: {}", std::io::Error::last_os_error());
    }
    Ok(unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) })
}

fn vu32(map: &HashMap<String, Value>, key: &str) -> Result<u32> {
    match map.get(key) {
        Some(Value::U32(x)) => Ok(*x),
        other => bail!("ScreenShot2 reply missing/odd '{key}': {other:?}"),
    }
}

fn run_capture(method: &str, mut call: impl FnMut(&Connection, Fd) -> zbus::Result<zbus::Message>) -> Result<Shot> {
    let (r, w) = make_pipe()?;
    let reader = std::thread::spawn(move || -> std::io::Result<Vec<u8>> {
        let mut buf = Vec::new();
        File::from(r).read_to_end(&mut buf)?;
        Ok(buf)
    });

    let conn = Connection::session().context("connect to session bus")?;
    let reply = call(&conn, Fd::from(w.as_fd())).map_err(|e| {
        let msg = e.to_string();
        if msg.to_ascii_lowercase().contains("authorized") {
            anyhow::anyhow!("{msg}\nhint: run `desk setup` once to install the KWin authorization desktop file")
        } else {
            anyhow::anyhow!("{method} failed: {msg}")
        }
    })?;
    drop(w); // our copy; KWin holds its dup until it finishes streaming

    let body = reply.body();
    let results: HashMap<String, Value> = body.deserialize()?;
    let data = reader
        .join()
        .map_err(|_| anyhow::anyhow!("pipe reader thread panicked"))?
        .context("reading pixel stream")?;

    let width = vu32(&results, "width")?;
    let height = vu32(&results, "height")?;
    let stride = vu32(&results, "stride")?;
    let format = vu32(&results, "format")?;
    to_rgb(width, height, stride, format, &data)
}

pub fn capture_workspace(cursor: bool) -> Result<Shot> {
    run_capture("CaptureWorkspace", |conn, fd| {
        conn.call_method(Some(DEST), PATH, Some(IFACE), "CaptureWorkspace", &(options(cursor), fd))
    })
}

pub fn capture_screen(name: &str, cursor: bool) -> Result<Shot> {
    run_capture("CaptureScreen", |conn, fd| {
        conn.call_method(Some(DEST), PATH, Some(IFACE), "CaptureScreen", &(name, options(cursor), fd))
    })
}

pub fn capture_area(x: i32, y: i32, w: u32, h: u32, cursor: bool) -> Result<Shot> {
    run_capture("CaptureArea", |conn, fd| {
        conn.call_method(Some(DEST), PATH, Some(IFACE), "CaptureArea", &(x, y, w, h, options(cursor), fd))
    })
}

/// QImage::Format -> packed RGB8. 4=RGB32, 5=ARGB32, 6=ARGB32_Premultiplied
/// (little-endian BGRA in memory); 16/17/18 = RGBX/RGBA8888 byte order; 13 = RGB888.
fn to_rgb(width: u32, height: u32, stride: u32, format: u32, data: &[u8]) -> Result<Shot> {
    let (w, h, stride) = (width as usize, height as usize, stride as usize);
    let need = stride * h;
    if data.len() < need {
        bail!("short pixel stream: got {} bytes, need {need} ({w}x{h} stride {stride} fmt {format})", data.len());
    }
    let mut rgb = vec![0u8; w * h * 3];
    let (bpp, order) = match format {
        4 | 5 | 6 => (4usize, [2usize, 1, 0]), // BGRA in memory
        16 | 17 | 18 => (4, [0, 1, 2]),        // RGBA in memory
        13 => (3, [0, 1, 2]),                  // RGB888
        other => bail!("unhandled QImage format {other}"),
    };
    for row in 0..h {
        let src = &data[row * stride..row * stride + w * bpp];
        let dst = &mut rgb[row * w * 3..(row + 1) * w * 3];
        for col in 0..w {
            let s = &src[col * bpp..];
            let d = &mut dst[col * 3..col * 3 + 3];
            d[0] = s[order[0]];
            d[1] = s[order[1]];
            d[2] = s[order[2]];
        }
    }
    Ok(Shot {
        width: width,
        height: height,
        rgb,
    })
}

impl Shot {
    pub fn crop(&self, x: u32, y: u32, w: u32, h: u32) -> Option<Shot> {
        if x + w > self.width || y + h > self.height || w == 0 || h == 0 {
            return None;
        }
        let (sw, x, y, w, h) = (self.width as usize, x as usize, y as usize, w as usize, h as usize);
        let mut rgb = vec![0u8; w * h * 3];
        for row in 0..h {
            let src = &self.rgb[((y + row) * sw + x) * 3..((y + row) * sw + x + w) * 3];
            rgb[row * w * 3..(row + 1) * w * 3].copy_from_slice(src);
        }
        Some(Shot {
            width: w as u32,
            height: h as u32,
            rgb,
        })
    }

    pub fn save_png(&self, path: &Path) -> Result<()> {
        use image::codecs::png::{CompressionType, FilterType, PngEncoder};
        use image::{ExtendedColorType, ImageEncoder};
        let f = BufWriter::new(File::create(path).with_context(|| format!("create {}", path.display()))?);
        PngEncoder::new_with_quality(f, CompressionType::Fast, FilterType::Adaptive)
            .write_image(&self.rgb, self.width, self.height, ExtendedColorType::Rgb8)?;
        Ok(())
    }
}
