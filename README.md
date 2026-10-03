# deskpilot

Desktop control for a KDE Plasma (Wayland) machine, built so an AI agent
such as Claude Code can see and drive the desktop: atomic screenshots of
every monitor, and a virtual mouse and keyboard. Everything uses KWin's own
interfaces, in global compositor coordinates, so the place a screenshot
shows is the place a click lands.

It comes as one binary, `desk`, which works two ways:

- **a command-line tool**: `desk shot`, `desk click 4600 300`, `desk type "hello"`;
- **an MCP server** (`desk mcp`): the same actions as tools an agent calls
  directly, with no shell round trip.

## Why

On Wayland the usual tools do not work: `xdotool` needs X11, `ydotool` and
uinput devices get pointer acceleration and device classification in the way,
and screenshot portals ask a person to approve each capture. KWin has
better paths, but they are restricted: it only lets a program it trusts use
them. deskpilot uses those paths and registers itself as trusted (`desk
setup`, below).

## Features

- **Screenshots** through KWin's `org.kde.KWin.ScreenShot2` D-Bus interface.
  The whole workspace is grabbed in one atomic frame and then cut per
  monitor, so "both monitors at once" is literal. A single monitor or a
  rectangle at native resolution works too. Pixels arrive over a pipe and
  are saved as PNG.
- **Mouse** through the `org_kde_kwin_fake_input` Wayland protocol (the one
  KDE Connect uses): absolute moves in global logical coordinates, with no
  acceleration or calibration; click, double click, press and release,
  scroll, drag.
- **Keyboard** through the same protocol: key combinations (`ctrl+shift+t`,
  `alt+space`, `f5`) and typed text. The compositor applies the active
  keyboard layout; `type` assumes US.
- **Outputs**: each monitor's name, position, size and scale (`wl_output` v4).
- **Idle state**: whether the person is active or idle (`ext-idle-notify-v1`).
- **MCP server**: newline-delimited JSON-RPC on stdin and stdout, one Wayland
  connection for the life of the server.

## Requirements

- KDE Plasma 6 on Wayland (KWin). Other compositors do not offer these
  interfaces.
- Rust (stable) to build.
- `kbuildsycoca6` (part of KDE Frameworks), used by `desk setup`.

## Build and install

```bash
git clone https://github.com/Lasimeri/deskpilot.git
cd deskpilot
cargo build --release
./target/release/desk setup     # once, and again whenever the binary moves
```

### Authorization: `desk setup`

Both KWin interfaces are restricted. KWin looks up the calling program's
path (`/proc/<pid>/exe`) and checks it against the `Exec=` line of
desktop entries that carry the right keys:

- `X-KDE-DBUS-Restricted-Interfaces=org.kde.KWin.ScreenShot2` for screenshots;
- `X-KDE-Wayland-Interfaces=org_kde_kwin_fake_input` for the mouse and keyboard.

`desk setup` writes `~/.local/share/applications/desk.desktop` with
`Exec=` set to the binary's absolute path, and rebuilds the KDE service
cache. The authorization belongs to that exact path: a debug build, a
moved checkout or a copied binary fails with `NotAuthorized` or a missing
Wayland global until `desk setup` is run from the new path.

## Command line

```
desk setup                            # authorize this binary (see above)
desk outputs                          # monitors: name, geometry, scale; the workspace's bounds
desk shot                             # the whole workspace + one PNG per monitor
desk shot --screen DP-1               # one monitor
desk shot --region 4400 100 400 400   # a rectangle in global coordinates, native resolution
desk shot --no-cursor --no-split
desk click 4600 300 [right] [--double]
desk mouse move X Y
desk mouse click|down|up [left|right|middle]
desk mouse scroll DY [DX]               # wheel notches, positive = down / right
desk mouse drag X1 Y1 X2 Y2
desk key ctrl+shift+t f5 enter        # combinations, in order
desk type "text" [--enter]            # US layout; a character it cannot type is an error
desk idle [--threshold-ms 600000]     # prints idle or active
desk mcp                              # run as an MCP server
```

Screenshots are written to `~/.cache/desk-shots/`, named with the epoch
milliseconds. `desk` sets `XDG_RUNTIME_DIR` and `WAYLAND_DISPLAY` defaults
itself, so it also works from SSH or other contexts without the session's
environment.

## MCP server (Claude Code)

```bash
claude mcp add desk -- /absolute/path/to/deskpilot/target/release/desk mcp
```

Tools: `screenshot` (all monitors, one monitor or a region; scaled to
`max_width`, default 1568, 0 for full size; the result says how its pixels
map to global coordinates), `outputs`, `click`, `move`, `mouse_button`,
`scroll`, `drag`, `key`, `type` and `idle`. The server's instructions tell
the agent to look before acting: take a screenshot, act, then take another
to check.

## Coordinates

Global logical pixels of the compositor's workspace, the rectangle that
`desk outputs` prints for each monitor. With every monitor at scale 1 they
equal screenshot pixels; a scaled-down MCP screenshot states its origin and
scale (`global = origin + pixel / scale`). Trust `desk outputs` over any
layout written down anywhere.

## Known quirks

- **The first key event of each process is dropped** by the fake-input
  protocol. The MCP server spends it once at startup (a Shift press and
  release); a one-off `desk key` or `desk type` from the command line loses
  its first key, so lead with a harmless key if it matters.
- `type` maps characters for a US layout; under another active layout the
  compositor turns the same key codes into other characters.
- The authorization is tied to the binary's path (see `desk setup`).

## Safety

This gives whatever runs `desk` full control of the desktop: it can see
every screen and press any key. Only the binary at the path `desk setup`
registered is trusted by KWin, and nothing listens on the network: the MCP
server talks over stdin and stdout to the agent that started it. Give it to
an agent you would let sit at your keyboard.

## How it was verified

On a two-monitor Plasma 6 desktop: workspace, single-monitor and region
captures against the live session; an absolute pointer move landing on the
exact commanded pixel (checked with a region shot); `alt+space` to KRunner,
then `type`, produced the exact string, and Escape dismissed it. Scroll and
drag use the same verified channel.

## Layout

| path | what |
| --- | --- |
| `desk/src/main.rs` | the command line |
| `desk/src/shot.rs` | screenshots through KWin's ScreenShot2 |
| `desk/src/wl.rs` | Wayland: outputs, fake input, idle notification |
| `desk/src/keys.rs` | key names and the US-layout table for `type` |
| `desk/src/mcp.rs` | the MCP server |
| `desk/src/setup.rs` | the desktop entry that authorizes the binary |
