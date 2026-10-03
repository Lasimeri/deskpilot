//! Wayland side: output enumeration (wl_output) and virtual input via
//! org_kde_kwin_fake_input. Absolute pointer coordinates are global
//! compositor-space logical coordinates, i.e. the same space as output
//! positions and workspace screenshots.

use anyhow::{bail, Context, Result};
use std::thread::sleep;
use std::time::Duration;
use wayland_client::{
    delegate_noop,
    protocol::{wl_output, wl_registry, wl_seat},
    Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum,
};
use wayland_protocols::ext::idle_notify::v1::client::{
    ext_idle_notification_v1::{self, ExtIdleNotificationV1},
    ext_idle_notifier_v1::ExtIdleNotifierV1,
};
use wayland_protocols_plasma::fake_input::client::org_kde_kwin_fake_input::OrgKdeKwinFakeInput;

pub const BTN_LEFT: u32 = 0x110;
pub const BTN_RIGHT: u32 = 0x111;
pub const BTN_MIDDLE: u32 = 0x112;
pub const BTN_SIDE: u32 = 0x113;
pub const BTN_EXTRA: u32 = 0x114;

pub fn button_code(name: &str) -> Result<u32> {
    Ok(match name.to_ascii_lowercase().as_str() {
        "left" | "l" => BTN_LEFT,
        "right" | "r" => BTN_RIGHT,
        "middle" | "m" => BTN_MIDDLE,
        "side" | "back" => BTN_SIDE,
        "extra" | "forward" => BTN_EXTRA,
        other => bail!("unknown button '{other}' (left|right|middle|side|extra)"),
    })
}

#[derive(Debug, Default, Clone)]
pub struct Output {
    pub name: String,
    pub x: i32,
    pub y: i32,
    pub pw: i32, // pixel (mode) size
    pub ph: i32,
    pub scale: i32,
}

impl Output {
    pub fn lw(&self) -> i32 {
        self.pw / self.scale.max(1)
    }
    pub fn lh(&self) -> i32 {
        self.ph / self.scale.max(1)
    }
}

pub struct State {
    outputs: Vec<(wl_output::WlOutput, Output)>,
    fake: Option<OrgKdeKwinFakeInput>,
    seat: Option<wl_seat::WlSeat>,
    idle_notifier: Option<ExtIdleNotifierV1>,
    idled: bool,
}

impl Dispatch<wl_registry::WlRegistry, ()> for State {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        {
            match interface.as_str() {
                "wl_output" => {
                    let idx = state.outputs.len();
                    let out =
                        registry.bind::<wl_output::WlOutput, usize, State>(name, version.min(4), qh, idx);
                    state.outputs.push((
                        out,
                        Output {
                            scale: 1,
                            ..Default::default()
                        },
                    ));
                }
                "org_kde_kwin_fake_input" => {
                    state.fake =
                        Some(registry.bind::<OrgKdeKwinFakeInput, (), State>(name, version.min(5), qh, ()));
                }
                "wl_seat" => {
                    state.seat =
                        Some(registry.bind::<wl_seat::WlSeat, (), State>(name, version.min(5), qh, ()));
                }
                "ext_idle_notifier_v1" => {
                    state.idle_notifier =
                        Some(registry.bind::<ExtIdleNotifierV1, (), State>(name, 1, qh, ()));
                }
                _ => {}
            }
        }
    }
}

impl Dispatch<wl_output::WlOutput, usize> for State {
    fn event(
        state: &mut Self,
        _: &wl_output::WlOutput,
        event: wl_output::Event,
        idx: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let out = &mut state.outputs[*idx].1;
        match event {
            wl_output::Event::Geometry { x, y, .. } => {
                out.x = x;
                out.y = y;
            }
            wl_output::Event::Mode {
                flags: WEnum::Value(flags),
                width,
                height,
                ..
            } => {
                if flags.contains(wl_output::Mode::Current) {
                    out.pw = width;
                    out.ph = height;
                }
            }
            wl_output::Event::Scale { factor } => out.scale = factor,
            wl_output::Event::Name { name } => out.name = name,
            _ => {}
        }
    }
}

delegate_noop!(State: ignore OrgKdeKwinFakeInput);
delegate_noop!(State: ignore wl_seat::WlSeat);
delegate_noop!(State: ignore ExtIdleNotifierV1);

impl Dispatch<ExtIdleNotificationV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &ExtIdleNotificationV1,
        event: ext_idle_notification_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            ext_idle_notification_v1::Event::Idled => state.idled = true,
            ext_idle_notification_v1::Event::Resumed => state.idled = false,
            _ => {}
        }
    }
}

pub struct Desktop {
    queue: EventQueue<State>,
    state: State,
    fake_authed: bool,
}

impl Desktop {
    pub fn connect() -> Result<Self> {
        let conn = Connection::connect_to_env().context("connect to Wayland display")?;
        let display = conn.display();
        let mut queue = conn.new_event_queue();
        let qh = queue.handle();
        display.get_registry(&qh, ());
        let mut state = State {
            outputs: Vec::new(),
            fake: None,
            seat: None,
            idle_notifier: None,
            idled: false,
        };
        queue.roundtrip(&mut state)?; // registry globals
        queue.roundtrip(&mut state)?; // wl_output property events
        Ok(Self {
            queue,
            state,
            fake_authed: false,
        })
    }

    pub fn outputs(&self) -> Vec<Output> {
        let mut v: Vec<Output> = self
            .state
            .outputs
            .iter()
            .map(|(_, o)| o.clone())
            .filter(|o| o.pw > 0)
            .collect();
        v.sort_by_key(|o| (o.x, o.y));
        v
    }

    fn roundtrip(&mut self) -> Result<()> {
        self.queue.roundtrip(&mut self.state)?;
        Ok(())
    }

    fn fake(&mut self) -> Result<OrgKdeKwinFakeInput> {
        let f = self
            .state
            .fake
            .clone()
            .context("compositor does not expose org_kde_kwin_fake_input (KWin required)")?;
        if f.version() < 4 {
            bail!(
                "org_kde_kwin_fake_input version {} too old (need >= 4 for absolute motion / keys)",
                f.version()
            );
        }
        if !self.fake_authed {
            f.authenticate("desk".into(), "Claude Code desktop control".into());
            self.queue.roundtrip(&mut self.state)?;
            self.fake_authed = true;
        }
        Ok(f)
    }

    pub fn mouse_move(&mut self, x: f64, y: f64) -> Result<()> {
        let f = self.fake()?;
        f.pointer_motion_absolute(x, y);
        self.roundtrip()
    }

    pub fn button(&mut self, code: u32, pressed: bool) -> Result<()> {
        let f = self.fake()?;
        f.button(code, pressed as u32);
        self.roundtrip()
    }

    pub fn click(&mut self, code: u32, double: bool) -> Result<()> {
        self.button(code, true)?;
        sleep(Duration::from_millis(40));
        self.button(code, false)?;
        if double {
            sleep(Duration::from_millis(90));
            self.button(code, true)?;
            sleep(Duration::from_millis(40));
            self.button(code, false)?;
        }
        Ok(())
    }

    /// Positive notches scroll down/right; one notch = 15 axis units.
    pub fn scroll(&mut self, notches: f64, horizontal: bool) -> Result<()> {
        let f = self.fake()?;
        let axis = if horizontal { 1 } else { 0 };
        let whole = notches.abs().round().max(1.0) as u32;
        let step = 15.0 * notches.signum();
        for _ in 0..whole {
            f.axis(axis, step);
            self.roundtrip()?;
            sleep(Duration::from_millis(15));
        }
        Ok(())
    }

    pub fn drag(&mut self, x1: f64, y1: f64, x2: f64, y2: f64, code: u32, ms: u64) -> Result<()> {
        self.mouse_move(x1, y1)?;
        sleep(Duration::from_millis(60));
        self.button(code, true)?;
        sleep(Duration::from_millis(60));
        let steps = (ms / 10).clamp(4, 200);
        for i in 1..=steps {
            let t = i as f64 / steps as f64;
            self.mouse_move(x1 + (x2 - x1) * t, y1 + (y2 - y1) * t)?;
            sleep(Duration::from_millis(ms / steps));
        }
        sleep(Duration::from_millis(60));
        self.button(code, false)
    }

    pub fn key(&mut self, code: u32, pressed: bool) -> Result<()> {
        let f = self.fake()?;
        f.keyboard_key(code, pressed as u32);
        self.roundtrip()
    }

    /// True if the seat has been idle for at least threshold_ms.
    /// ext-idle-notify sends Idled immediately when the threshold is already met.
    pub fn idle_check(&mut self, threshold_ms: u32) -> Result<bool> {
        let seat = self.state.seat.clone().context("no wl_seat advertised")?;
        let notifier = self
            .state
            .idle_notifier
            .clone()
            .context("compositor lacks ext_idle_notifier_v1")?;
        let qh = self.queue.handle();
        let notification = notifier.get_idle_notification(threshold_ms, &seat, &qh, ());
        self.roundtrip()?;
        if !self.state.idled {
            sleep(Duration::from_millis(300));
            self.roundtrip()?;
        }
        notification.destroy();
        let _ = self.roundtrip();
        Ok(self.state.idled)
    }
}
