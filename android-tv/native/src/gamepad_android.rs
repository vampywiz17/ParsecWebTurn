//! Android event-code adapter and controller-local vibration callback only.
use crate::gamepad::{self, Event, Motors, State};
use jni::{
    objects::{GlobalRef, JObject, JValue},
    JNIEnv, JavaVM,
};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
struct Callback {
    vm: JavaVM,
    object: GlobalRef,
}
static CALLBACK: Mutex<Option<Arc<Callback>>> = Mutex::new(None);
pub static ACTIVE: AtomicBool = AtomicBool::new(false);
pub fn bind(env: &JNIEnv, object: JObject) -> jni::errors::Result<()> {
    let callback = if object.is_null() {
        None
    } else {
        Some(Arc::new(Callback {
            vm: env.get_java_vm()?,
            object: env.new_global_ref(object)?,
        }))
    };
    *CALLBACK.lock().unwrap_or_else(|e| e.into_inner()) = callback;
    Ok(())
}
pub fn focus(active: bool) {
    ACTIVE.store(active, Ordering::Release);
    crate::stream::controller_event(Event::Focus(active));
}
pub fn button(id: i32, code: i32, down: bool) -> bool {
    if !ACTIVE.load(Ordering::Acquire) {
        return false;
    }
    let mask = match code {
        19 => 1,
        20 => 2,
        21 => 4,
        22 => 8,
        108 => 0x10,
        109 => 0x20,
        106 => 0x40,
        107 => 0x80,
        102 => 0x100,
        103 => 0x200,
        96 => 0x1000,
        97 => 0x2000,
        99 => 0x4000,
        100 => 0x8000,
        // HOME/MODE remains Android-owned, not a remote Guide shortcut.
        104 | 105 => {
            crate::stream::controller_event(Event::TriggerButton(
                id,
                if code == 104 { 1 } else { 2 },
                down,
            ));
            return true;
        }
        _ => return false,
    };
    crate::stream::controller_event(Event::Button(id, mask, down));
    true
}
pub fn axes(id: i32, values: [f32; 8], flat: [f32; 6]) {
    if !ACTIVE.load(Ordering::Acquire) {
        return;
    }
    let [x, y, rx, ry, lt, rt, hx, hy] = values;
    let hat = (if hy < -0.5 { 1 } else { 0 })
        | (if hy > 0.5 { 2 } else { 0 })
        | (if hx < -0.5 { 4 } else { 0 })
        | (if hx > 0.5 { 8 } else { 0 });
    crate::stream::controller_event(Event::Axes(
        id,
        State {
            lx: gamepad::stick(x, flat[0]),
            ly: gamepad::stick(-y, flat[1]),
            rx: gamepad::stick(rx, flat[2]),
            ry: gamepad::stick(-ry, flat[3]),
            lt: gamepad::trigger(lt, flat[4]),
            rt: gamepad::trigger(rt, flat[5]),
            ..Default::default()
        },
        hat,
    ));
}
pub struct Output {
    callback: Option<Arc<Callback>>,
    active: [Option<i32>; gamepad::PADS],
}
impl Default for Output {
    fn default() -> Self {
        Self {
            callback: CALLBACK.lock().unwrap_or_else(|e| e.into_inner()).clone(),
            active: [None; gamepad::PADS],
        }
    }
}
impl Motors for Output {
    fn rumble(&mut self, id: i32, low: u8, high: u8) {
        let present = self.active.iter().position(|v| *v == Some(id));
        if low == 0 && high == 0 && present.is_none() {
            return;
        }
        if let Some(slot) = present.or_else(|| self.active.iter().position(Option::is_none)) {
            self.active[slot] = if low == 0 && high == 0 {
                None
            } else {
                Some(id)
            };
        }
        if let Some(c) = &self.callback {
            if let Ok(mut env) = c.vm.attach_current_thread() {
                if env
                    .call_method(
                        c.object.as_obj(),
                        "rumble",
                        "(III)V",
                        &[
                            JValue::Int(id),
                            JValue::Int(low.into()),
                            JValue::Int(high.into()),
                        ],
                    )
                    .is_err()
                {
                    let _ = env.exception_clear();
                }
            }
        }
    }
}
impl Drop for Output {
    fn drop(&mut self) {
        for id in self.active.into_iter().flatten() {
            self.rumble(id, 0, 0);
        }
    }
}
