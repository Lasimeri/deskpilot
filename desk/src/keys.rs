//! evdev keycode tables, combo parsing ("ctrl+shift+t"), and US-layout
//! text-to-keystroke mapping. Codes go through org_kde_kwin_fake_input's
//! keyboard_key; the compositor applies the active xkb keymap (assumed US).

use anyhow::{bail, Result};

pub const KEY_LEFTSHIFT: u32 = 42;

pub fn modifier_code(name: &str) -> Option<u32> {
    Some(match name {
        "ctrl" | "control" => 29,
        "shift" => 42,
        "alt" => 56,
        "altgr" => 100,
        "meta" | "super" | "win" | "cmd" => 125,
        _ => return None,
    })
}

pub fn named_key(name: &str) -> Option<u32> {
    Some(match name {
        "esc" | "escape" => 1,
        "backspace" => 14,
        "tab" => 15,
        "enter" | "return" => 28,
        "space" => 57,
        "capslock" => 58,
        "numlock" => 69,
        "scrolllock" => 70,
        "home" => 102,
        "up" => 103,
        "pgup" | "pageup" => 104,
        "left" => 105,
        "right" => 106,
        "end" => 107,
        "down" => 108,
        "pgdn" | "pagedown" => 109,
        "insert" | "ins" => 110,
        "delete" | "del" => 111,
        "menu" => 127,
        "print" | "printscreen" => 99,
        "pause" => 119,
        "f1" => 59,
        "f2" => 60,
        "f3" => 61,
        "f4" => 62,
        "f5" => 63,
        "f6" => 64,
        "f7" => 65,
        "f8" => 66,
        "f9" => 67,
        "f10" => 68,
        "f11" => 87,
        "f12" => 88,
        _ => return None,
    })
}

/// US layout: char -> (evdev code, needs shift).
pub fn char_key(c: char) -> Option<(u32, bool)> {
    let unshifted = |code| Some((code, false));
    let shifted = |code| Some((code, true));
    match c {
        'a' => unshifted(30), 'b' => unshifted(48), 'c' => unshifted(46), 'd' => unshifted(32),
        'e' => unshifted(18), 'f' => unshifted(33), 'g' => unshifted(34), 'h' => unshifted(35),
        'i' => unshifted(23), 'j' => unshifted(36), 'k' => unshifted(37), 'l' => unshifted(38),
        'm' => unshifted(50), 'n' => unshifted(49), 'o' => unshifted(24), 'p' => unshifted(25),
        'q' => unshifted(16), 'r' => unshifted(19), 's' => unshifted(31), 't' => unshifted(20),
        'u' => unshifted(22), 'v' => unshifted(47), 'w' => unshifted(17), 'x' => unshifted(45),
        'y' => unshifted(21), 'z' => unshifted(44),
        'A' => shifted(30), 'B' => shifted(48), 'C' => shifted(46), 'D' => shifted(32),
        'E' => shifted(18), 'F' => shifted(33), 'G' => shifted(34), 'H' => shifted(35),
        'I' => shifted(23), 'J' => shifted(36), 'K' => shifted(37), 'L' => shifted(38),
        'M' => shifted(50), 'N' => shifted(49), 'O' => shifted(24), 'P' => shifted(25),
        'Q' => shifted(16), 'R' => shifted(19), 'S' => shifted(31), 'T' => shifted(20),
        'U' => shifted(22), 'V' => shifted(47), 'W' => shifted(17), 'X' => shifted(45),
        'Y' => shifted(21), 'Z' => shifted(44),
        '1' => unshifted(2), '2' => unshifted(3), '3' => unshifted(4), '4' => unshifted(5),
        '5' => unshifted(6), '6' => unshifted(7), '7' => unshifted(8), '8' => unshifted(9),
        '9' => unshifted(10), '0' => unshifted(11),
        '!' => shifted(2), '@' => shifted(3), '#' => shifted(4), '$' => shifted(5),
        '%' => shifted(6), '^' => shifted(7), '&' => shifted(8), '*' => shifted(9),
        '(' => shifted(10), ')' => shifted(11),
        '-' => unshifted(12), '_' => shifted(12),
        '=' => unshifted(13), '+' => shifted(13),
        '[' => unshifted(26), '{' => shifted(26),
        ']' => unshifted(27), '}' => shifted(27),
        '\\' => unshifted(43), '|' => shifted(43),
        ';' => unshifted(39), ':' => shifted(39),
        '\'' => unshifted(40), '"' => shifted(40),
        '`' => unshifted(41), '~' => shifted(41),
        ',' => unshifted(51), '<' => shifted(51),
        '.' => unshifted(52), '>' => shifted(52),
        '/' => unshifted(53), '?' => shifted(53),
        ' ' => unshifted(57),
        '\n' => unshifted(28),
        '\t' => unshifted(15),
        _ => None,
    }
}

/// "ctrl+shift+t" -> (mod codes in press order, key code).
pub fn parse_combo(combo: &str) -> Result<(Vec<u32>, u32)> {
    let parts: Vec<&str> = combo.split('+').collect();
    if parts.is_empty() || parts.iter().any(|p| p.is_empty()) {
        bail!("bad combo '{combo}'");
    }
    let (key_part, mod_parts) = parts.split_last().unwrap();
    let mut mods = Vec::new();
    for m in mod_parts {
        let lm = m.to_ascii_lowercase();
        match modifier_code(&lm) {
            Some(c) => mods.push(c),
            None => bail!("unknown modifier '{m}' in '{combo}'"),
        }
    }
    let kl = key_part.to_ascii_lowercase();
    let code = if let Some(c) = named_key(&kl) {
        c
    } else if let Some(c) = modifier_code(&kl) {
        c // allow tapping a bare modifier, e.g. `desk key meta`
    } else if key_part.chars().count() == 1 {
        let ch = key_part.chars().next().unwrap();
        let (c, shift) = match char_key(ch) {
            Some(v) => v,
            None => bail!("no keycode for character '{ch}'"),
        };
        if shift {
            mods.push(KEY_LEFTSHIFT);
        }
        c
    } else {
        bail!("unknown key '{key_part}' in '{combo}'");
    };
    Ok((mods, code))
}

/// Text -> per-char (code, shift) list; errors on unmappable chars.
pub fn text_events(text: &str) -> Result<Vec<(u32, bool)>> {
    let mut out = Vec::with_capacity(text.len());
    for c in text.chars() {
        match char_key(c) {
            Some(v) => out.push(v),
            None => bail!("cannot type character {c:?} with the US-layout table (use clipboard instead)"),
        }
    }
    Ok(out)
}
