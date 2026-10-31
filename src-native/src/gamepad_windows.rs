//! Native XInput adapter. No bundled SDL, controller driver or second input thread.
use crate::gamepad::{self, Motors, State};
use windows_sys::Win32::UI::Input::XboxController::{
    XInputGetState, XInputSetState, XINPUT_STATE, XINPUT_VIBRATION,
};
#[derive(Default)]
pub struct Devices {
    vibrating: [bool; gamepad::PADS],
}
impl gamepad::polling::Devices for Devices {
    fn read(&mut self, index: u32) -> Option<State> {
        let mut s = XINPUT_STATE::default();
        if unsafe { XInputGetState(index, &mut s) } != 0 {
            return None;
        }
        let s = s.Gamepad;
        let axis = |v: i16, flat: f32| {
            gamepad::stick(
                v as f32 / if v < 0 { 32768. } else { 32767. },
                flat / 32767.,
            )
        };
        Some(State {
            buttons: s.wButtons & !0x0800,
            lx: axis(s.sThumbLX, 7849.),
            ly: axis(s.sThumbLY, 7849.),
            rx: axis(s.sThumbRX, 8689.),
            ry: axis(s.sThumbRY, 8689.),
            lt: gamepad::trigger(s.bLeftTrigger as f32 / 255., 30. / 255.),
            rt: gamepad::trigger(s.bRightTrigger as f32 / 255., 30. / 255.),
        })
    }
}
impl Motors for Devices {
    fn rumble(&mut self, id: i32, low: u8, high: u8) {
        let Some(running) = usize::try_from(id)
            .ok()
            .and_then(|i| self.vibrating.get_mut(i))
        else {
            return;
        };
        if low == 0 && high == 0 && !*running {
            return;
        }
        let v = XINPUT_VIBRATION {
            wLeftMotorSpeed: u16::from(low) * 257,
            wRightMotorSpeed: u16::from(high) * 257,
        };
        unsafe {
            XInputSetState(id as u32, &v);
        }
        *running = low != 0 || high != 0;
    }
}
impl Drop for Devices {
    fn drop(&mut self) {
        for id in 0..gamepad::PADS {
            self.rumble(id as i32, 0, 0);
        }
    }
}
