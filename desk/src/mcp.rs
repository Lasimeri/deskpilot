//! `desk mcp`: the desktop as an MCP server, newline-delimited JSON-RPC on
//! stdin and stdout. One Wayland connection for the life of the server, so
//! the fake_input warm-up (each process's first key event is dropped) is
//! spent once at the start, not on every action. Coordinates are global
//! compositor coordinates, the same as `desk outputs`; a screenshot says how
//! its pixels map to them.

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::io::{BufRead, Write};
use std::thread::sleep;
use std::time::Duration;

use crate::{keys, shot, wl};

const INSTRUCTIONS: &str = "The person's desktop (KDE Plasma, Wayland): screenshots of its monitors and a virtual mouse and keyboard. Coordinates are global compositor coordinates (outputs lists each monitor's rectangle). Look before acting: take a screenshot, act, then take another to check. A screenshot is scaled down by default; its text gives the global origin and scale (global = origin + pixel / scale); for small text, take a region at full size (max_width 0). type types US-layout text into the focused window.";

pub fn run() -> Result<()> {
    let mut d = wl::Desktop::connect()?;
    // The first key event of a fake_input client is dropped: spend it here.
    d.key(keys::KEY_LEFTSHIFT, true)?;
    sleep(Duration::from_millis(10));
    d.key(keys::KEY_LEFTSHIFT, false)?;

    let stdin = std::io::stdin();
    let mut out = std::io::stdout();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let Ok(msg) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let id = msg.get("id").cloned();
        let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
        let reply = match method {
            "initialize" => Some(Ok(json!({
                "protocolVersion": msg["params"]["protocolVersion"].as_str().unwrap_or("2025-06-18"),
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "desk", "version": env!("CARGO_PKG_VERSION")},
                "instructions": INSTRUCTIONS,
            }))),
            "ping" => Some(Ok(json!({}))),
            "tools/list" => Some(Ok(json!({ "tools": tools() }))),
            "tools/call" => Some(Ok(call(&mut d, &msg["params"]))),
            _ if id.is_some() => Some(Err(format!("unknown method {method}"))),
            _ => None, // a notification
        };
        let (Some(id), Some(reply)) = (id, reply) else {
            continue;
        };
        let resp = match reply {
            Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
            Err(e) => json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": e}}),
        };
        writeln!(out, "{resp}")?;
        out.flush()?;
    }
    Ok(())
}

fn tool(name: &str, description: &str, properties: Value, required: &[&str]) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": {"type": "object", "properties": properties, "required": required},
    })
}

fn tools() -> Vec<Value> {
    let xy = |what: &str| json!({"type": "number", "description": format!("{what}, global compositor coordinates")});
    let button = json!({"type": "string", "enum": ["left", "right", "middle"], "description": "default left"});
    vec![
        tool(
            "screenshot",
            "Capture the desktop as a PNG: every monitor (default), one monitor by name, or a region. Scaled to max_width (default 1568; 0 = full size); the text says the global origin and scale: global = origin + pixel / scale.",
            json!({
                "screen": {"type": "string", "description": "a monitor name from outputs, e.g. DP-1"},
                "region": {"type": "array", "items": {"type": "integer"}, "minItems": 4, "maxItems": 4, "description": "[x, y, w, h] in global coordinates"},
                "max_width": {"type": "integer", "description": "scale down to this width; 0 keeps full size"},
                "cursor": {"type": "boolean", "description": "include the pointer (default true)"},
            }),
            &[],
        ),
        tool("outputs", "The monitors: name, rectangle in global coordinates, scale.", json!({}), &[]),
        tool(
            "click",
            "Move the pointer to x, y and click.",
            json!({"x": xy("x"), "y": xy("y"), "button": button, "double": {"type": "boolean"}}),
            &["x", "y"],
        ),
        tool("move", "Move the pointer to x, y.", json!({"x": xy("x"), "y": xy("y")}), &["x", "y"]),
        tool(
            "mouse_button",
            "Press or release a mouse button where the pointer is.",
            json!({"button": button, "down": {"type": "boolean", "description": "true presses, false releases"}}),
            &["down"],
        ),
        tool(
            "scroll",
            "Scroll by wheel notches where the pointer is; positive scrolls down (or right).",
            json!({"notches": {"type": "number"}, "horizontal": {"type": "boolean"}}),
            &["notches"],
        ),
        tool(
            "drag",
            "Press at x1, y1, move to x2, y2 over ms milliseconds, release.",
            json!({"x1": xy("start x"), "y1": xy("start y"), "x2": xy("end x"), "y2": xy("end y"), "ms": {"type": "integer"}, "button": button}),
            &["x1", "y1", "x2", "y2"],
        ),
        tool(
            "key",
            "Tap key combos in order, e.g. [\"ctrl+l\", \"enter\"] or [\"super\"].",
            json!({"combos": {"type": "array", "items": {"type": "string"}}}),
            &["combos"],
        ),
        tool(
            "type",
            "Type text (US layout) into the focused window; enter presses Enter after.",
            json!({"text": {"type": "string"}, "enter": {"type": "boolean"}}),
            &["text"],
        ),
        tool(
            "idle",
            "Whether the person has been away from the input devices for threshold_ms (default 60000).",
            json!({"threshold_ms": {"type": "integer"}}),
            &[],
        ),
    ]
}

fn text(s: String) -> Value {
    json!({"content": [{"type": "text", "text": s}]})
}

fn call(d: &mut wl::Desktop, params: &Value) -> Value {
    let name = params["name"].as_str().unwrap_or("");
    let a = &params["arguments"];
    match act(d, name, a) {
        Ok(v) => v,
        Err(e) => json!({"content": [{"type": "text", "text": format!("{name}: {e:#}")}], "isError": true}),
    }
}

fn num(a: &Value, k: &str) -> Result<f64> {
    a[k].as_f64().with_context(|| format!("{k} must be a number"))
}

fn btn(a: &Value) -> Result<u32> {
    wl::button_code(a["button"].as_str().unwrap_or("left"))
}

fn act(d: &mut wl::Desktop, name: &str, a: &Value) -> Result<Value> {
    match name {
        "screenshot" => screenshot(d, a),
        "outputs" => {
            let mut s = String::new();
            for o in d.outputs() {
                s += &format!("{}: {}x{} at {},{} (scale {})\n", o.name, o.lw(), o.lh(), o.x, o.y, o.scale);
            }
            Ok(text(s))
        }
        "click" => {
            let (x, y) = (num(a, "x")?, num(a, "y")?);
            d.mouse_move(x, y)?;
            sleep(Duration::from_millis(60));
            d.click(btn(a)?, a["double"].as_bool().unwrap_or(false))?;
            Ok(text(format!("clicked at {x},{y}")))
        }
        "move" => {
            let (x, y) = (num(a, "x")?, num(a, "y")?);
            d.mouse_move(x, y)?;
            Ok(text(format!("pointer at {x},{y}")))
        }
        "mouse_button" => {
            let down = a["down"].as_bool().context("down must be true or false")?;
            d.button(btn(a)?, down)?;
            Ok(text(if down { "pressed" } else { "released" }.into()))
        }
        "scroll" => {
            let n = num(a, "notches")?;
            d.scroll(n, a["horizontal"].as_bool().unwrap_or(false))?;
            Ok(text(format!("scrolled {n}")))
        }
        "drag" => {
            let (x1, y1, x2, y2) = (num(a, "x1")?, num(a, "y1")?, num(a, "x2")?, num(a, "y2")?);
            let ms = a["ms"].as_u64().unwrap_or(400);
            d.drag(x1, y1, x2, y2, btn(a)?, ms)?;
            Ok(text(format!("dragged {x1},{y1} -> {x2},{y2}")))
        }
        "key" => {
            let combos: Vec<String> = a["combos"]
                .as_array()
                .context("combos must be a list")?
                .iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect();
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
                sleep(Duration::from_millis(30));
            }
            Ok(text(format!("sent {}", combos.join(" "))))
        }
        "type" => {
            let t = a["text"].as_str().context("text must be a string")?;
            for (code, shift) in keys::text_events(t)? {
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
                sleep(Duration::from_millis(8));
            }
            if a["enter"].as_bool().unwrap_or(false) {
                sleep(Duration::from_millis(30));
                d.key(28, true)?;
                sleep(Duration::from_millis(20));
                d.key(28, false)?;
            }
            Ok(text(format!("typed {} characters", t.chars().count())))
        }
        "idle" => {
            let ms = a["threshold_ms"].as_u64().unwrap_or(60_000) as u32;
            Ok(text(if d.idle_check(ms)? { "idle" } else { "active" }.into()))
        }
        other => bail!("there is no tool {other}"),
    }
}

fn screenshot(d: &mut wl::Desktop, a: &Value) -> Result<Value> {
    let cursor = a["cursor"].as_bool().unwrap_or(true);
    let (s, ox, oy) = if let Some(r) = a["region"].as_array() {
        let v: Vec<i64> = r.iter().filter_map(Value::as_i64).collect();
        if v.len() != 4 {
            bail!("region is [x, y, w, h]");
        }
        (shot::capture_area(v[0] as i32, v[1] as i32, v[2] as u32, v[3] as u32, cursor)?, v[0], v[1])
    } else if let Some(name) = a["screen"].as_str() {
        let o = d.outputs().into_iter().find(|o| o.name == name).with_context(|| format!("no monitor named {name}"))?;
        (shot::capture_screen(name, cursor)?, o.x as i64, o.y as i64)
    } else {
        let outs = d.outputs();
        let ox = outs.iter().map(|o| o.x).min().unwrap_or(0) as i64;
        let oy = outs.iter().map(|o| o.y).min().unwrap_or(0) as i64;
        (shot::capture_workspace(cursor)?, ox, oy)
    };
    let max_w = a["max_width"].as_u64().unwrap_or(1568) as u32;
    let (png, w, h) = encode(&s, max_w)?;
    let scale = w as f64 / s.width as f64;
    Ok(json!({"content": [
        {"type": "image", "data": base64(&png), "mimeType": "image/png"},
        {"type": "text", "text": format!(
            "{w}x{h} image of {}x{} at global {ox},{oy}; scale {scale:.4}: global x = {ox} + px / {scale:.4}, global y = {oy} + py / {scale:.4}",
            s.width, s.height
        )},
    ]}))
}

/// PNG bytes, scaled down to max_w wide (0: as captured).
fn encode(s: &shot::Shot, max_w: u32) -> Result<(Vec<u8>, u32, u32)> {
    use image::codecs::png::{CompressionType, FilterType, PngEncoder};
    use image::{ExtendedColorType, ImageEncoder, RgbImage};
    let img = RgbImage::from_raw(s.width, s.height, s.rgb.clone()).context("screenshot buffer")?;
    let img = if max_w > 0 && s.width > max_w {
        let h = ((s.height as u64 * max_w as u64) / s.width as u64).max(1) as u32;
        image::imageops::resize(&img, max_w, h, image::imageops::FilterType::Triangle)
    } else {
        img
    };
    let mut png = Vec::new();
    PngEncoder::new_with_quality(&mut png, CompressionType::Fast, FilterType::Adaptive).write_image(
        img.as_raw(),
        img.width(),
        img.height(),
        ExtendedColorType::Rgb8,
    )?;
    Ok((png, img.width(), img.height()))
}

fn base64(b: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::with_capacity(b.len().div_ceil(3) * 4);
    for c in b.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        s.push(T[(n >> 18) as usize & 63] as char);
        s.push(T[(n >> 12) as usize & 63] as char);
        s.push(if c.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        s.push(if c.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    s
}
