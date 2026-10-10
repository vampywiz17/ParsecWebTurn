//! Bounded offline native UI experiment, separate from connected video.
use crate::{host::HostState, window::Event};
use anyhow::{bail, Context, Result};
use std::{
    sync::atomic::Ordering,
    time::{Duration, Instant},
};
use wasmtime::{Caller, Instance, Store, Val};

pub fn handles(name: &str) -> bool {
    name.starts_with("gl")
        || matches!(
            name,
            "web_set_gfx"
                | "web_gl_flush"
                | "web_present"
                | "web_set_canvas_size"
                | "web_run_and_yield"
                | "web_set_app"
                | "web_set_title"
                | "MTY_DecompressImage"
        )
}

fn invoke(caller: &mut Caller<'_, HostState>, name: &str, args: &[Val]) -> Result<()> {
    let f = caller
        .get_export(name)
        .and_then(|e| e.into_func())
        .with_context(|| format!("Missing {name}"))?;
    f.call(caller, args, &mut [])
}

pub fn dispatch(
    caller: &mut Caller<'_, HostState>,
    name: &str,
    args: &[Val],
    results: &mut [Val],
) -> Result<()> {
    let window = caller
        .data()
        .window
        .clone()
        .context("native window missing")?;
    let m = caller.data().memory.clone();
    let int = |n: usize| {
        args.get(n)
            .and_then(Val::i32)
            .context("native bridge expected i32")
    };
    match name {
        "web_set_gfx" => {
            if caller.data().graphics.is_some() {
                bail!("GPU context already initialized");
            }
            caller.data_mut().graphics = Some(crate::graphics::Graphics::create(window)?);
        }
        "web_present" => caller
            .data_mut()
            .graphics
            .as_mut()
            .context("GPU context missing")?
            .present()?,
        "web_set_canvas_size" => {
            let (w, h) = (int(0)?, int(1)?);
            if w < 0 || h < 0 || w > 8192 || h > 8192 {
                bail!("Invalid native canvas dimensions");
            }
            // Win32 owns the swapchain size. Matoya sets viewport in its GL calls.
        }
        "web_run_and_yield" => {
            caller.data_mut().event_loop = Some((int(0)? as u32, int(1)? as u32));
            bail!("native event-loop handoff");
        }
        "web_set_app" => {
            let app = int(0)?;
            caller.data_mut().app_pointer = Some(app as u32);
            let (w, h) = *window.dimensions.lock().unwrap_or_else(|e| e.into_inner());
            let (x, y, screen_w, screen_h, focused) = window.initial_geometry();
            for (name, x, y) in [
                ("mty_window_update_position", x, y),
                ("mty_window_update_screen", screen_w, screen_h),
                ("mty_window_update_size", w, h),
            ] {
                invoke(
                    caller,
                    name,
                    &[
                        Val::I32(app),
                        Val::F64((x as f64).to_bits()),
                        Val::F64((y as f64).to_bits()),
                    ],
                )?;
            }
            for (name, values) in [
                ("mty_window_update_focus", vec![app, focused as i32]),
                ("mty_window_update_fullscreen", vec![app, 0]),
                ("mty_window_update_visibility", vec![app, 1]),
                ("mty_window_update_relative_mouse", vec![app, 0]),
            ] {
                invoke(
                    caller,
                    name,
                    &values.into_iter().map(Val::I32).collect::<Vec<_>>(),
                )?;
            }
            invoke(
                caller,
                "mty_window_update_pixel_ratio",
                &[Val::I32(app), Val::F64(1.0_f64.to_bits())],
            )?;
        }
        "web_set_title" => {
            m.string(int(0)? as u32, 1024)?;
            let title = crate::APP_TITLE.to_owned();
            let wide: Vec<u16> = title.encode_utf16().chain([0]).collect();
            unsafe {
                windows_sys::Win32::UI::WindowsAndMessaging::SetWindowTextW(
                    window.handle(),
                    wide.as_ptr(),
                );
            }
            caller.data_mut().title = Some(title);
        }
        "MTY_DecompressImage" => {
            let size = usize::try_from(int(1)?)?;
            if size > 16 * 1024 * 1024 {
                bail!("Encoded UI image limit exceeded");
            }
            let bytes = m.read(int(0)? as u32, size)?;
            let mut decoder =
                image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format()?;
            let mut limits = image::Limits::default();
            limits.max_image_width = Some(4096);
            limits.max_image_height = Some(4096);
            limits.max_alloc = Some(32 * 1024 * 1024);
            decoder.limits(limits);
            let image = decoder.decode()?.into_rgba8();
            let (w, h) = image.dimensions();
            let alloc = caller
                .get_export("mty_system_alloc")
                .and_then(|e| e.into_func())
                .context("guest allocator missing")?
                .typed::<(i32, i32), i32>(&*caller)?;
            let p = alloc.call(&mut *caller, (image.as_raw().len() as i32, 1))? as u32;
            if p == 0 {
                bail!("Guest UI image allocation failed");
            }
            m.write(p, image.as_raw())?;
            m.set_u32(int(2)? as u32, w)?;
            m.set_u32(int(3)? as u32, h)?;
            if let Some(slot) = results.first_mut() {
                *slot = Val::I32(p as i32);
            }
        }
        _ => {
            let value = caller
                .data_mut()
                .graphics
                .as_mut()
                .context("GPU context missing")?
                .call(name, &m, args)?;
            if let Some(value) = value {
                if let Some(slot) = results.first_mut() {
                    *slot = Val::I32(value);
                }
            }
        }
    }
    Ok(())
}

fn export(
    store: &mut Store<HostState>,
    instance: &Instance,
    name: &'static str,
    args: &[Val],
) -> Result<()> {
    #[cfg(any(test, feature = "diagnostics"))]
    {
        store.data_mut().execution_stage = Some(name);
    }
    instance
        .get_func(&mut *store, name)
        .with_context(|| format!("Missing {name}"))?
        .call(store, args, &mut [])
}

pub fn run(store: &mut Store<HostState>, instance: &Instance) -> Result<()> {
    run_loop(store, instance, true)
}

pub fn run_worker(store: &mut Store<HostState>, instance: &Instance) -> Result<()> {
    run_loop(store, instance, false)
}

fn run_loop(store: &mut Store<HostState>, instance: &Instance, input: bool) -> Result<()> {
    let window = store
        .data()
        .window
        .clone()
        .context("native window missing")?;
    let (index, opaque) = store.data().event_loop.context("event loop missing")?;
    let table = instance
        .get_table(&mut *store, "__indirect_function_table")
        .context("callback table missing")?;
    let callback = match table.get(&mut *store, index as u64) {
        Some(wasmtime::Ref::Func(Some(f))) => f.typed::<i32, i32>(&*store)?,
        _ => bail!("invalid event-loop callback"),
    };
    if input {
        export(store, instance, "mty_app_set_keys", &[])?;
    }
    let until = Instant::now() + Duration::from_secs(window.run_seconds);
    #[cfg(any(test, feature = "diagnostics"))]
    let mut script_frame = 0;
    let mut first = true;
    while !window.closing.load(Ordering::Acquire) && (window.live || Instant::now() < until) {
        let frame_started = Instant::now();
        // The first render callback decompresses/rasterizes the embedded font
        // atlas. Give that one-time initialization its own bounded budget;
        // subsequent iterations retain the smaller per-frame allowance.
        store.set_fuel(if first && !input {
            500_000_000
        } else {
            50_000_000
        })?;
        if input {
            #[cfg(any(test, feature = "diagnostics"))]
            if window.synthetic_login {
                // Isolated offline test of the pinned UI layout, not login
                // automation for real accounts. Wait for rendered widgets and
                // separate press/release/text stages by actual presentations.
                let frames = window
                    .graphics
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .as_ref()
                    .map_or(0, |g| g.frames_presented);
                let step = window.script_steps.load(Ordering::Acquire);
                if frames >= 2 && frames > script_frame && step < 9 {
                    let (width, height) =
                        *window.dimensions.lock().unwrap_or_else(|e| e.into_inner());
                    let x = width / 2;
                    let email_y = height / 2 - 34;
                    let login_y = height / 2 + 138;
                    let mut events = window.events.lock().unwrap_or_else(|e| e.into_inner());
                    match step {
                        0 => {
                            events.push_back(Event::Focus(true));
                            events.push_back(Event::Motion(x, email_y));
                            events.push_back(Event::Button(true, 0, x, email_y));
                        }
                        1 => events.push_back(Event::Button(false, 0, x, email_y)),
                        2 => events.extend(
                            "native-audit@example.invalid"
                                .chars()
                                .map(|c| Event::Text(c as u32)),
                        ),
                        3 => events.push_back(Event::Key(true, "Tab", 0)),
                        4 => events.push_back(Event::Key(false, "Tab", 0)),
                        5 => {
                            events.push_back(Event::Key(true, "ControlLeft", 2));
                            events.push_back(Event::Key(true, "KeyV", 2));
                        }
                        6 => {
                            events.push_back(Event::Key(false, "KeyV", 2));
                            events.push_back(Event::Key(false, "ControlLeft", 0));
                        }
                        7 => {
                            events.push_back(Event::Motion(x, login_y));
                            events.push_back(Event::Button(true, 0, x, login_y));
                        }
                        8 => events.push_back(Event::Button(false, 0, x, login_y)),
                        _ => unreachable!(),
                    }
                    script_frame = frames;
                    window.script_steps.store(step + 1, Ordering::Release);
                }
            }
            let app = store.data().app_pointer.context("app pointer missing")? as i32;
            let events = window
                .events
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .drain(..)
                .collect::<Vec<_>>();
            for event in events {
                let (name, values) = match event {
                    Event::Fullscreen(active) => {
                        ("mty_window_update_fullscreen", vec![app, active as i32])
                    }
                    Event::Size(w, h) => {
                        export(
                            store,
                            instance,
                            "mty_window_update_size",
                            &[
                                Val::I32(app),
                                Val::F64((w as f64).to_bits()),
                                Val::F64((h as f64).to_bits()),
                            ],
                        )?;
                        ("mty_window_size", vec![app])
                    }
                    Event::Focus(f) => {
                        export(
                            store,
                            instance,
                            "mty_window_update_focus",
                            &[Val::I32(app), Val::I32(f as i32)],
                        )?;
                        ("mty_window_focus", vec![app, f as i32])
                    }
                    Event::RelativeMode(active) => {
                        ("mty_window_update_relative_mouse", vec![app, active as i32])
                    }
                    Event::RelativeMotion(x, y) => ("mty_window_motion", vec![app, 1, x, y]),
                    Event::Motion(x, y) => ("mty_window_motion", vec![app, 0, x, y]),
                    Event::Button(down, button, x, y) => {
                        ("mty_window_button", vec![app, down as i32, button, x, y])
                    }
                    Event::Scroll(x, y) => ("mty_window_scroll", vec![app, x, y]),
                    Event::Key(down, code, mods) => {
                        let Some(key) = store.data().key_codes.get(code).copied() else {
                            continue;
                        };
                        ("mty_window_keyboard", vec![app, down as i32, key, 0, mods])
                    }
                    Event::Text(code) => {
                        let Some(ch) = char::from_u32(code) else {
                            continue;
                        };
                        if ch.is_control() {
                            continue;
                        }
                        let mut bytes = [0; 4];
                        ch.encode_utf8(&mut bytes);
                        (
                            "mty_window_keyboard",
                            vec![app, 1, 0, u32::from_le_bytes(bytes) as i32, 0],
                        )
                    }
                };
                export(
                    store,
                    instance,
                    name,
                    &values.into_iter().map(Val::I32).collect::<Vec<_>>(),
                )?;
            }
        }
        #[cfg(any(test, feature = "diagnostics"))]
        {
            store.data_mut().execution_stage = Some("event-loop-callback");
        }
        if callback.call(&mut *store, opaque as i32)? == 0 {
            break;
        }
        first = false;
        // SwapBuffers can already wait for vblank. Do not add a full second
        // frame delay on top of that; idle callbacks still avoid busy polling.
        let remaining =
            Duration::from_nanos(1_000_000_000 / 60).saturating_sub(frame_started.elapsed());
        if !remaining.is_zero() {
            std::thread::sleep(remaining);
        }
    }
    Ok(())
}
