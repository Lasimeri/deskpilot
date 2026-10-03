# deskpilot

KWin/Wayland desktop control CLI. Built for driving this machine from Claude Code:
atomic dual-monitor screenshots plus a virtual mouse and keyboard, all in
global compositor coordinates so what a screenshot shows is where a click lands.

Cargo workspace; single member crate `desk`. Binary: `target/release/desk`.

## Mechanisms

- Screenshots: KWin's `org.kde.KWin.ScreenShot2` D-Bus interface (v5). Pixels
  stream over a pipe fd; converted from QImage BGRA/RGBA to RGB PNG (fast
  compression). `CaptureWorkspace` grabs every monitor in one atomic frame,
  then crops per output, so "both monitors at once" is literal.
- Input: `org_kde_kwin_fake_input` Wayland protocol (the KDE Connect path).
  `pointer_motion_absolute` takes global logical coordinates: no libinput
  acceleration, no uinput device classification, no calibration.
  `keyboard_key` takes evdev codes; the compositor applies the active xkb
  layout (US assumed by the `type` mapping table).
- Output enumeration: `wl_output` v4 (name, position, mode, scale).

## Authorization (the part that will bite later)

Both KWin interfaces are restricted. KWin resolves the caller's
`/proc/<pid>/exe` and matches it against `Exec` of desktop entries carrying:

- `X-KDE-DBUS-Restricted-Interfaces=org.kde.KWin.ScreenShot2` (D-Bus, screenshots)
- `X-KDE-Wayland-Interfaces=org_kde_kwin_fake_input` (Wayland global, input)

`desk setup` writes `~/.local/share/applications/desk.desktop` with
`Exec=<current binary path>` and runs `kbuildsycoca6`. If the binary path
changes (debug build, moved workspace), re-run `desk setup`. A debug build at a
different path fails with NotAuthorized / missing-global errors by design.

## Usage

```
desk setup                          # once, and after the binary moves
desk outputs                        # monitor names + geometry + workspace bbox
desk shot                           # atomic grab -> all.png + one png per monitor
desk shot --region 4400 100 400 400 # native-res rectangle, global coords
desk shot --screen DP-4             # single output
desk shot --no-cursor --no-split
desk click 4600 300 [right] [--double]
desk mouse move|click|down|up|scroll|drag ...
desk key ctrl+shift+t f5 enter      # combos in sequence
desk type "text" [--enter]          # US layout; unmappable chars error out
desk idle [--threshold-ms 600000]   # prints idle|active via ext-idle-notify-v1
```

PNGs land in `~/.cache/desk-shots/` stamped with epoch millis.

Coordinates are global logical pixels (scale 1 on this box, so identical to
image pixels). Current layout: DP-5 2560x1440 at (0,720), DP-4 3840x2160 at
(2560,0); workspace 6400x2160. Trust `desk outputs` over this paragraph.

The tool sets `XDG_RUNTIME_DIR`/`WAYLAND_DISPLAY` defaults itself, so it works
from bare SSH/exec contexts without session env.

## Verified 2026-09-01

- Workspace + per-monitor + region captures against live session
- Pointer absolute move landed at exact commanded pixel (region-shot check)
- `alt+space` KRunner + `type` produced the exact string, Escape dismissed
- Scroll/drag share the same verified channel but were not separately exercised
