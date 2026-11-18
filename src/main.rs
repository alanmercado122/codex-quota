#![cfg_attr(all(target_os = "windows", not(test)), windows_subsystem = "windows")]

slint::include_modules!();
mod quota;
#[cfg(windows)] mod native;
use slint::ComponentHandle;
use std::{path::PathBuf, time::{Duration, Instant}};

fn main() {
    if let Err(error) = run() {
        // Only controlled messages from this app, never raw RPC responses or credentials.
        let _ = std::fs::write(output_path("error-rust.txt"), error);
        std::process::exit(1);
    }
}

fn output_path(name: &str) -> PathBuf {
    std::env::current_exe().unwrap_or_default().parent().unwrap_or(std::path::Path::new(".")).join(name)
}

fn apply(panel: &QuotaPanel, data: &quota::Snapshot, remaining: bool, stale: bool) {
    panel.set_five_label(data.five.as_ref().map(|q| q.label(remaining)).unwrap_or("--".into()).into());
    panel.set_week_label(data.week.as_ref().map(|q| q.label(remaining)).unwrap_or("--".into()).into());
    panel.set_five_value(data.five.as_ref().map(|q| q.fraction(remaining)).unwrap_or_default());
    panel.set_week_value(data.week.as_ref().map(|q| q.fraction(remaining)).unwrap_or_default());
    panel.set_stale(stale);
}

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--probe") {
        let data = quota::read()?;
        std::fs::write(output_path("probe-rust.json"), serde_json::to_vec_pretty(&data).map_err(|_| "No se pudo serializar la cuota")?).map_err(|_| "No se pudo guardar la consulta")?;
        return Ok(());
    }
    #[cfg(windows)] { run_windows(&args) }
    #[cfg(not(windows))] { run_portable() }
}

#[cfg(windows)]
struct SoftwarePlatform(std::rc::Rc<slint::platform::software_renderer::MinimalSoftwareWindow>);
#[cfg(windows)]
impl slint::platform::Platform for SoftwarePlatform {
    fn create_window_adapter(&self) -> Result<std::rc::Rc<dyn slint::platform::WindowAdapter>, slint::PlatformError> { Ok(self.0.clone()) }
}

#[cfg(windows)]
fn run_windows(args: &[String]) -> Result<(), String> {
    use slint::platform::{WindowEvent, software_renderer::{MinimalSoftwareWindow, RepaintBufferType, PremultipliedRgbaColor}};
    let preview_demo = args.iter().any(|a| a == "--preview-demo");
    let preview = preview_demo || args.iter().any(|a| a == "--preview");
    let smoke = args.iter().any(|a| a == "--smoke");
    let _instance = if preview || smoke { None } else {
        let Some(lock) = native::Instance::lock() else { return Ok(()); }; Some(lock)
    };
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    slint::platform::set_platform(Box::new(SoftwarePlatform(window.clone()))).map_err(|_| "No se pudo iniciar Slint")?;
    let panel = QuotaPanel::new().map_err(|_| "No se pudo crear el panel Slint")?;
    panel.show().map_err(|_| "No se pudo mostrar el panel Slint")?;
    if preview {
        if preview_demo {
            panel.set_five_label("82%".into());
            panel.set_week_label("67%".into());
            panel.set_five_value(0.82);
            panel.set_week_value(0.67);
        } else {
            apply(&panel, &quota::read()?, true, false);
        }
        let width = 290;
        let height = 88;
        for (light, name) in [(false, "rust-preview-dark.png"), (true, "rust-preview-light.png")] {
            native::theme(&panel, light);
            window.dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor: 2. });
            window.set_size(slint::PhysicalSize::new(width as u32, height as u32));
            slint::platform::update_timers_and_animations();
            window.request_redraw();
            let mut pixels = vec![PremultipliedRgbaColor::default(); width * height];
            window.draw_if_needed(|renderer| { renderer.render(&mut pixels, width); });
            save_preview(&pixels, width, height, light, &output_path(name))?;
        }
        return Ok(());
    }
    let mut host = native::Host::new(native::Settings::load())?;
    panel.set_pixel_icon(host.settings.pixel_icon);
    let worker = quota::Worker::start();
    let mut data = quota::Snapshot::default(); let mut stale = true;
    let start = Instant::now();
    let mut last_layout = Instant::now() - Duration::from_secs(5);
    let mut last_theme = Instant::now() - Duration::from_secs(10);
    let mut layout: Option<(usize,usize,f32)> = None;
    let mut pixels = Vec::new();
    let mut painted = false;
    loop {
        if !host.pump() { break; }
        let mut command = host.poll_command();
        if command == 100 { command = host.menu(&data,stale); }
        match command {
            1 => { let _ = worker.refresh.send(()); },
            2 => { host.settings.remaining = !host.settings.remaining; host.settings.save(); },
            3 => native::toggle_startup(),
            4 => host.reset(),
            5 => break,
            6 => host.about(),
            7 => {
                host.settings.pixel_icon = !host.settings.pixel_icon;
                host.settings.save();
                panel.set_pixel_icon(host.settings.pixel_icon);
            }
            _ => (),
        }
        while let Ok(result) = worker.results.try_recv() {
            match result { Ok(snapshot) => { data = snapshot; stale = false; }, Err(_) => stale = true }
        }
        if last_layout.elapsed() >= Duration::from_secs(2) {
            let previous_handle = host.hwnd;
            let previous_parent = host.parent;
            if host.position().is_err() { host.wait(); continue; }
            if host.hwnd != previous_handle || host.parent != previous_parent { window.request_redraw(); }
            last_layout = Instant::now();
            let width = (145.*host.scale).round() as usize; let height = (44.*host.scale).round() as usize;
            if layout != Some((width,height,host.scale)) {
                window.dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor:host.scale });
                window.set_size(slint::PhysicalSize::new(width as u32,height as u32));
                pixels.resize(width*height,PremultipliedRgbaColor::default());
                layout = Some((width,height,host.scale)); window.request_redraw();
            }
        }
        if last_theme.elapsed() >= Duration::from_secs(5) { native::theme(&panel,native::light_theme()); last_theme = Instant::now(); }
        apply(&panel,&data,host.settings.remaining,stale);
        slint::platform::update_timers_and_animations();
        if let Some((width,height,_)) = layout {
            let mut paint_error = None;
            window.draw_if_needed(|renderer| {
                pixels.fill(PremultipliedRgbaColor::default());
                renderer.render(&mut pixels,width);
                match host.paint(&pixels,width,height) { Ok(()) => painted = true, Err(error) => paint_error = Some(error) }
            });
            if let Some(error) = paint_error { return Err(error); }
        }
        if smoke && start.elapsed() >= Duration::from_secs(5) {
            let mut report = host.report(); report["painted"] = painted.into(); report["live_quota"] = (!stale).into();
            std::fs::write(output_path("smoke-rust.json"),serde_json::to_vec_pretty(&report).unwrap()).map_err(|_| "No se pudo guardar la verificación")?;
            if !painted { return Err("El panel no se dibujó".into()); }
            break;
        }
        host.wait();
    }
    Ok(())
}

#[cfg(windows)]
fn save_preview(pixels: &[slint::platform::software_renderer::PremultipliedRgbaColor], width: usize,height: usize, light: bool,path: &std::path::Path) -> Result<(),String> {
    let file = std::fs::File::create(path).map_err(|_| "No se pudo guardar la vista previa")?;
    let mut encoder = png::Encoder::new(file,width as u32,height as u32); encoder.set_color(png::ColorType::Rgba); encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().map_err(|_| "No se pudo escribir la imagen")?;
    let background = if light { 243 } else { 32 };
    let mut bytes = Vec::with_capacity(pixels.len()*4);
    for p in pixels {
        let blend = |value: u8| (u16::from(value)+((255-u16::from(p.alpha))*background)/255).min(255) as u8;
        bytes.extend_from_slice(&[blend(p.red),blend(p.green),blend(p.blue),255]);
    }
    writer.write_image_data(&bytes).map_err(|_| "No se pudo escribir la imagen".into())
}

#[cfg(not(windows))]
fn run_portable() -> Result<(),String> {
    // Same reader and Slint UI. Native panel/dock adapters need validation on each OS.
    let panel = QuotaPanel::new().map_err(|_| "No se pudo iniciar Slint")?;
    let weak = panel.as_weak(); let worker = quota::Worker::start(); let mut data = quota::Snapshot::default(); let mut stale = true;
    let timer = slint::Timer::default();
    timer.start(slint::TimerMode::Repeated,Duration::from_secs(1), move || {
        if let Some(panel) = weak.upgrade() {
            while let Ok(result) = worker.results.try_recv() { match result { Ok(s) => { data=s;stale=false; },Err(_) => stale=true } }
            apply(&panel,&data,true,stale);
        }
    });
    panel.run().map_err(|_| "No se pudo ejecutar la interfaz Slint")
}
