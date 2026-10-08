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
                | "MTY_HttpRequest"
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
    mut caller: Caller<'_, HostState>,
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
            for (name, values) in [
                ("mty_window_update_position", vec![app, 0, 0]),
                ("mty_window_update_screen", vec![app, w, h]),
                ("mty_window_update_size", vec![app, w, h]),
                ("mty_window_update_focus", vec![app, 1]),
                ("mty_window_update_fullscreen", vec![app, 0]),
                ("mty_window_update_visibility", vec![app, 1]),
                ("mty_window_update_relative_mouse", vec![app, 0]),
            ] {
                invoke(
                    &mut caller,
                    name,
                    &values.into_iter().map(Val::I32).collect::<Vec<_>>(),
                )?;
            }
            invoke(
                &mut caller,
                "mty_window_update_pixel_ratio",
                &[Val::I32(app), Val::F32(1.0_f32.to_bits())],
            )?;
        }
        "web_set_title" => {
            let title = m.string(int(0)? as u32, 1024)?;
            let wide: Vec<u16> = title.encode_utf16().chain([0]).collect();
            unsafe {
                windows_sys::Win32::UI::WindowsAndMessaging::SetWindowTextW(
                    window.handle(),
                    wide.as_ptr(),
                );
            }
            caller.data_mut().title = Some(title);
        }
        "MTY_HttpRequest" => {
            // This experiment explicitly has no HTTP service. A request fails,
            // as an offline browser fetch would; it never fabricates a response.
            if let Some(slot) = results.first_mut() {
                *slot = Val::I32(0);
            }
        }
        "MTY_DecompressImage" => {
            let bytes = m.read(int(0)? as u32, usize::try_from(int(1)?)?)?;
            if bytes.len() > 16 * 1024 * 1024 {
                bail!("Encoded UI image limit exceeded");
            }
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
                .typed::<(i32, i32), i32>(&caller)?;
            let p = alloc.call(&mut caller, (image.as_raw().len() as i32, 1))? as u32;
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
    name: &str,
    args: &[Val],
) -> Result<()> {
    instance
        .get_func(&mut *store, name)
        .with_context(|| format!("Missing {name}"))?
        .call(store, args, &mut [])
}

pub fn run(store: &mut Store<HostState>, instance: &Instance) -> Result<()> {
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
    export(store, instance, "mty_app_set_keys", &[])?;
    let until = Instant::now() + Duration::from_secs(8);
    while !window.closing.load(Ordering::Acquire) && Instant::now() < until {
        store.set_fuel(50_000_000)?;
        let app = store.data().app_pointer.context("app pointer missing")? as i32;
        let events = window
            .events
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .drain(..)
            .collect::<Vec<_>>();
        for event in events {
            let (name, values) = match event {
                Event::Size(w, h) => {
                    export(
                        store,
                        instance,
                        "mty_window_update_size",
                        &[Val::I32(app), Val::I32(w), Val::I32(h)],
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
                Event::Motion(x, y) => ("mty_window_motion", vec![app, 0, x, y]),
                Event::Button(down, button, x, y) => {
                    ("mty_window_button", vec![app, down as i32, button, x, y])
                }
                Event::Text(code) => {
                    let Some(ch) = char::from_u32(code) else {
                        continue;
                    };
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
        if callback.call(&mut *store, opaque as i32)? == 0 {
            break;
        }
        std::thread::sleep(Duration::from_millis(16));
    }
    Ok(())
}
