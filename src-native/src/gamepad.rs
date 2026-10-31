//! Shared controller state/lifecycle and the stock host's 16-button wire format.
//! OS adapters only read devices and apply motor output. No media work lives here.
use anyhow::Result;

pub const PADS: usize = 4;
pub const POLL: std::time::Duration = std::time::Duration::from_millis(8);
pub const DISCOVERY: std::time::Duration = std::time::Duration::from_secs(1);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct State {
    pub buttons: u16,
    pub lx: i16,
    pub ly: i16,
    pub rx: i16,
    pub ry: i16,
    pub lt: u8,
    pub rt: u8,
}

/// Finite, clamped, rescaled dead zone; both platforms use the same rule.
pub fn stick(value: f32, flat: f32) -> i16 {
    if !value.is_finite() || !flat.is_finite() {
        return 0;
    }
    let flat = flat.clamp(0., 0.99);
    let v = value.clamp(-1., 1.);
    let v = v.signum() * ((v.abs() - flat).max(0.) / (1. - flat));
    (v * if v < 0. { 32768. } else { 32767. }).round() as i16
}
pub fn trigger(value: f32, flat: f32) -> u8 {
    if !value.is_finite() || !flat.is_finite() {
        return 0;
    }
    let flat = flat.clamp(0., 0.99);
    (((value.clamp(0., 1.) - flat).max(0.) / (1. - flat)) * 255.).round() as u8
}

pub fn report(pad: usize, s: State) -> [u8; 28] {
    let mut frame = [0u8; 28];
    frame[..4].copy_from_slice(&(pad as u32).to_be_bytes());
    frame[12] = 23;
    // Header arguments 1/2 and the three protocol padding bytes stay zero.
    for (i, v) in [
        s.buttons,
        s.lx as u16,
        s.ly as u16,
        s.rx as u16,
        s.ry as u16,
    ]
    .into_iter()
    .enumerate()
    {
        frame[16 + i * 2..18 + i * 2].copy_from_slice(&v.to_be_bytes());
    }
    frame[26] = s.lt;
    frame[27] = s.rt;
    frame
}

#[derive(Clone, Copy, Default)]
struct Slot {
    device: Option<i32>,
    state: State,
    sent: Option<State>,
    // After a focus loss, a neutral state must precede another activation.
    blocked: bool,
    digital_triggers: u8,
    hat: u16,
}
#[derive(Default)]
pub struct Manager {
    slots: [Slot; PADS],
    active: bool,
}

pub trait Motors {
    fn rumble(&mut self, device: i32, low: u8, high: u8);
}

impl Manager {
    pub fn focus(
        &mut self,
        active: bool,
        motors: &mut impl Motors,
        mut send: impl FnMut(&[u8]) -> Result<()>,
    ) -> Result<()> {
        if self.active == active {
            return Ok(());
        }
        self.active = active;
        if !active {
            for (pad, slot) in self.slots.iter_mut().enumerate() {
                if let Some(id) = slot.device {
                    motors.rumble(id, 0, 0);
                }
                if slot.sent.take().is_some() {
                    send(&report(pad, State::default()))?;
                    send(&crate::control::header(6, 0, 0, pad as i32))?;
                }
                slot.state = State::default();
                slot.digital_triggers = 0;
                slot.hat = 0;
                slot.blocked = true;
            }
        }
        Ok(())
    }
    fn slot(&mut self, id: i32) -> Option<usize> {
        if let Some(p) = self.slots.iter().position(|s| s.device == Some(id)) {
            return Some(p);
        }
        let p = self.slots.iter().position(|s| s.device.is_none())?;
        self.slots[p].device = Some(id);
        Some(p)
    }
    pub fn update(
        &mut self,
        id: i32,
        state: State,
        send: impl FnMut(&[u8]) -> Result<()>,
    ) -> Result<()> {
        let Some(p) = self.slot(id) else {
            return Ok(());
        };
        self.slots[p].state = state;
        self.emit(p, send)
    }
    fn emit(&mut self, p: usize, mut send: impl FnMut(&[u8]) -> Result<()>) -> Result<()> {
        let slot = &mut self.slots[p];
        if !self.active {
            return Ok(());
        }
        let mut state = slot.state;
        state.buttons |= slot.hat;
        if slot.digital_triggers & 1 != 0 {
            state.lt = 255;
        }
        if slot.digital_triggers & 2 != 0 {
            state.rt = 255;
        }
        if slot.blocked {
            // Resting sticks can drift: buttons and triggers establish release.
            if state.buttons == 0 && state.lt == 0 && state.rt == 0 {
                slot.blocked = false;
            }
            return Ok(());
        }
        // Match stock client activation: a face button claims a host pad.
        if slot.sent.is_none() && state.buttons & 0xf000 == 0 {
            return Ok(());
        }
        if slot.sent == Some(state) {
            return Ok(());
        }
        send(&report(p, state))?;
        if slot.sent.is_none() {
            crate::event_log::record(
                "INFO",
                "controller_forwarding_started",
                serde_json::json!({"slot":p}),
            );
        }
        slot.sent = Some(state);
        Ok(())
    }
    pub fn remove(
        &mut self,
        id: i32,
        motors: &mut impl Motors,
        mut send: impl FnMut(&[u8]) -> Result<()>,
    ) -> Result<()> {
        let Some(p) = self.slots.iter().position(|s| s.device == Some(id)) else {
            return Ok(());
        };
        motors.rumble(id, 0, 0);
        if self.slots[p].sent.is_some() {
            send(&report(p, State::default()))?;
            send(&crate::control::header(6, 0, 0, p as i32))?;
        }
        self.slots[p] = Slot::default();
        Ok(())
    }
    pub fn feedback(&self, pad: u32, low: u8, high: u8, motors: &mut impl Motors) {
        if let Some(s) = self
            .slots
            .get(pad as usize)
            .filter(|s| self.active && s.sent.is_some())
        {
            if let Some(id) = s.device {
                motors.rumble(id, low, high);
            }
        }
    }
}

#[cfg(any(target_os = "android", test))]
mod events {
    use super::*;
    use std::{collections::VecDeque, sync::Mutex};
    #[derive(Clone, Copy, Debug)]
    pub enum Event {
        Button(i32, u16, bool),
        TriggerButton(i32, u8, bool),
        Axes(i32, State, u16),
        Remove(i32),
        Focus(bool),
    }
    #[derive(Default)]
    pub struct Inbox(Mutex<(VecDeque<Event>, bool)>);
    impl Inbox {
        pub fn push(&self, event: Event) {
            let mut q = self.0.lock().unwrap_or_else(|e| e.into_inner());
            if q.1 {
                return;
            }
            if let (Some(Event::Axes(id, old, hat)), Event::Axes(next, state, nhat)) =
                (q.0.back_mut(), event)
            {
                if *id == next
                    && *hat == nhat
                    && (old.lt == 0) == (state.lt == 0)
                    && (old.rt == 0) == (state.rt == 0)
                {
                    *old = state;
                    return;
                }
            }
            if q.0.len() == 128 {
                q.0.clear();
                q.1 = true;
            } else {
                q.0.push_back(event);
            }
        }
        pub fn take(&self, into: &mut VecDeque<Event>) -> Result<()> {
            let mut q = self.0.lock().unwrap_or_else(|e| e.into_inner());
            anyhow::ensure!(
                !q.1,
                "Controller input queue overloaded; reconnect to release input safely"
            );
            std::mem::swap(&mut q.0, into);
            Ok(())
        }
    }
    impl Manager {
        pub fn event(
            &mut self,
            event: Event,
            motors: &mut impl Motors,
            send: impl FnMut(&[u8]) -> Result<()>,
        ) -> Result<()> {
            let id = match event {
                Event::Focus(active) => return self.focus(active, motors, send),
                Event::Remove(id) => return self.remove(id, motors, send),
                Event::Button(id, ..) | Event::TriggerButton(id, ..) | Event::Axes(id, ..) => id,
            };
            let Some(p) = self.slot(id) else {
                return Ok(());
            };
            let slot = &mut self.slots[p];
            match event {
                Event::Button(_, mask, down) => {
                    if down {
                        slot.state.buttons |= mask;
                    } else {
                        slot.state.buttons &= !mask;
                    }
                }
                Event::TriggerButton(_, mask, down) => {
                    if down {
                        slot.digital_triggers |= mask;
                    } else {
                        slot.digital_triggers &= !mask;
                    }
                }
                Event::Axes(_, state, hat) => {
                    let buttons = slot.state.buttons;
                    slot.state = state;
                    slot.state.buttons = buttons;
                    slot.hat = hat;
                }
                _ => unreachable!(),
            }
            self.emit(p, send)
        }
    }
}
#[cfg(any(target_os = "android", test))]
pub use events::{Event, Inbox};

#[cfg(any(windows, test))]
pub mod polling {
    use super::*;
    use std::time::{Duration, Instant};
    pub trait Devices: Motors {
        fn read(&mut self, index: u32) -> Option<State>;
    }
    pub struct Poller {
        connected: [bool; PADS],
        scan: Instant,
        next: Instant,
    }
    impl Default for Poller {
        fn default() -> Self {
            let now = Instant::now();
            Self {
                connected: [false; PADS],
                scan: now,
                next: now,
            }
        }
    }
    impl Poller {
        pub fn tick(
            &mut self,
            pads: &mut Manager,
            devices: &mut impl Devices,
            active: bool,
            now: Instant,
            mut send: impl FnMut(&[u8]) -> Result<()>,
        ) -> Result<Duration> {
            pads.focus(active, devices, &mut send)?;
            if !active {
                self.next = now;
                self.scan = now;
                return Ok(DISCOVERY);
            }
            if now >= self.next {
                let scan = now >= self.scan;
                for i in 0..PADS {
                    if !self.connected[i] && !scan {
                        continue;
                    }
                    if let Some(state) = devices.read(i as u32) {
                        self.connected[i] = true;
                        pads.update(i as i32, state, &mut send)?;
                    } else if self.connected[i] {
                        self.connected[i] = false;
                        pads.remove(i as i32, devices, &mut send)?;
                    }
                }
                if scan {
                    self.scan = now + DISCOVERY;
                }
                self.next = if self.connected.iter().any(|c| *c) {
                    now + POLL
                } else {
                    self.scan
                };
            }
            Ok(self.next.saturating_duration_since(now))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Default)]
    struct Output(Vec<(i32, u8, u8)>);
    impl Motors for Output {
        fn rumble(&mut self, id: i32, low: u8, high: u8) {
            self.0.push((id, low, high));
        }
    }
    #[test]
    fn android_events_preserve_chords_hat_and_digital_trigger_fallback() {
        let mut m = Manager::default();
        let mut motors = Output::default();
        let mut frames = Vec::new();
        let mut send = |f: &[u8]| {
            frames.push(f.to_vec());
            Ok(())
        };
        for event in [
            Event::Focus(true),
            Event::Button(51, 0x1000, true),
            Event::Button(51, 0x2000, true),
            Event::TriggerButton(51, 1, true),
            Event::Axes(
                51,
                State {
                    lx: 123,
                    ..Default::default()
                },
                4,
            ),
            Event::TriggerButton(51, 1, false),
            Event::Remove(51),
        ] {
            m.event(event, &mut motors, &mut send).unwrap();
        }
        assert_eq!(&frames[3][16..18], &0x3004u16.to_be_bytes());
        assert_eq!(frames[3][26], 255);
        assert_eq!(frames[4][26], 0);
        assert_eq!(
            frames.last().unwrap().as_slice(),
            &crate::control::header(6, 0, 0, 0)[..]
        );
    }
    #[test]
    fn disconnected_slots_are_not_polled_at_input_rate() {
        struct Devices {
            reads: usize,
            state: Option<State>,
        }
        impl Motors for Devices {
            fn rumble(&mut self, _: i32, _: u8, _: u8) {}
        }
        impl polling::Devices for Devices {
            fn read(&mut self, _: u32) -> Option<State> {
                self.reads += 1;
                self.state
            }
        }
        let mut poller = polling::Poller::default();
        let mut pads = Manager::default();
        let mut devices = Devices {
            reads: 0,
            state: None,
        };
        let mut frames = Vec::new();
        let mut send = |f: &[u8]| {
            frames.push(f.to_vec());
            Ok(())
        };
        let now = std::time::Instant::now();
        poller
            .tick(&mut pads, &mut devices, true, now, &mut send)
            .unwrap();
        assert_eq!(devices.reads, 4);
        for n in 1..100 {
            poller
                .tick(&mut pads, &mut devices, true, now + POLL * n, &mut send)
                .unwrap();
        }
        assert_eq!(devices.reads, 4);
        devices.state = Some(State {
            buttons: 0x1000,
            ..Default::default()
        });
        poller
            .tick(&mut pads, &mut devices, true, now + DISCOVERY, &mut send)
            .unwrap();
        assert_eq!(devices.reads, 8);
        devices.state = None;
        poller
            .tick(
                &mut pads,
                &mut devices,
                true,
                now + DISCOVERY + POLL,
                &mut send,
            )
            .unwrap();
        assert_eq!(frames.len(), 12);
    }
    #[test]
    fn wire_has_exact_stock_layout_and_signed_sticks() {
        let frame = report(
            2,
            State {
                buttons: 0x3101,
                lx: -32768,
                ly: 32767,
                rx: -1,
                ry: 1,
                lt: 17,
                rt: 255,
            },
        );
        assert_eq!(&frame[..13], &crate::control::header(23, 2, 0, 0)[..]);
        assert_eq!(
            &frame[13..],
            &[0, 0, 0, 0x31, 1, 0x80, 0, 0x7f, 0xff, 0xff, 0xff, 0, 1, 17, 255]
        );
    }
    #[test]
    fn joint_buttons_dedup_focus_release_and_slot_reuse() {
        let mut m = Manager::default();
        let mut motors = Output::default();
        let mut frames = Vec::new();
        let mut send = |f: &[u8]| {
            frames.push(f.to_vec());
            Ok(())
        };
        m.focus(true, &mut motors, &mut send).unwrap();
        let a = State {
            buttons: 0x1000,
            ..Default::default()
        };
        m.update(99, a, &mut send).unwrap();
        m.update(99, a, &mut send).unwrap();
        m.update(
            99,
            State {
                buttons: 0x3000,
                ..a
            },
            &mut send,
        )
        .unwrap();
        m.feedback(0, 55, 88, &mut motors);
        assert_eq!(motors.0.last(), Some(&(99, 55, 88)));
        m.focus(false, &mut motors, &mut send).unwrap();
        m.focus(true, &mut motors, &mut send).unwrap();
        m.update(99, a, &mut send).unwrap(); // Held across focus loss: cannot claim.
        m.update(99, State::default(), &mut send).unwrap();
        m.update(99, a, &mut send).unwrap();
        m.remove(99, &mut motors, &mut send).unwrap();
        m.update(101, a, &mut send).unwrap();
        assert_eq!(frames.len(), 8);
        assert_eq!(&frames[1][16..18], &0x3000u16.to_be_bytes());
        assert_eq!(
            frames[3].as_slice(),
            &crate::control::header(6, 0, 0, 0)[..]
        );
        assert_eq!(&frames[7][..4], &0i32.to_be_bytes());
    }
    #[test]
    fn axis_bursts_keep_button_and_trigger_edges_and_bound_memory() {
        let q = Inbox::default();
        for n in 0..1000 {
            q.push(Event::Axes(
                1,
                State {
                    lx: n,
                    ..Default::default()
                },
                0,
            ));
        }
        q.push(Event::Button(1, 0x1000, true));
        q.push(Event::Button(1, 0x1000, false));
        q.push(Event::Axes(
            1,
            State {
                lt: 255,
                ..Default::default()
            },
            0,
        ));
        q.push(Event::Axes(1, State::default(), 0));
        let mut events = std::collections::VecDeque::new();
        q.take(&mut events).unwrap();
        assert_eq!(events.len(), 5);
        let q = Inbox::default();
        for _ in 0..129 {
            q.push(Event::Button(1, 0x1000, true));
        }
        assert!(q.take(&mut events).is_err());
        assert_eq!(stick(f32::NAN, 0.), 0);
        assert_eq!(stick(-1., 0.), i16::MIN);
        assert_eq!(stick(1., 0.), i16::MAX);
        assert_eq!(trigger(2., 0.), 255);
        assert_eq!(stick(0.1, 0.2), 0);
    }
}
