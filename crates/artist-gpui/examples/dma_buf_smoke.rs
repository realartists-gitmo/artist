#![cfg(target_os = "linux")]

use artist_computer::stage::wayland::{StageLease, StageWayland};
use artist_computer::stage::{AppCommand, Stage, StageId};
use gpui::{
    App, Bounds, Context, DmaBufPlane, DmaBufSurface, ObjectFit, Render, Window, WindowBounds,
    WindowOptions, div, prelude::*, px, rgb, size, surface,
};
use gpui_platform::application;
use std::{sync::Arc, time::Duration};

struct SmokeLogger;

impl log::Log for SmokeLogger {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.level() <= log::Level::Error || metadata.target() == "gpui_wgpu::wgpu_renderer"
    }

    fn log(&self, record: &log::Record<'_>) {
        if self.enabled(record.metadata()) {
            eprintln!("{} {}: {}", record.level(), record.target(), record.args());
        }
    }

    fn flush(&self) {}
}

static SMOKE_LOGGER: SmokeLogger = SmokeLogger;

struct DmaBufSmoke {
    surface: DmaBufSurface,
    _stage: Arc<StageWayland>,
    _runtime_dir: Arc<tempfile::TempDir>,
}

impl Render for DmaBufSmoke {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().size_full().bg(rgb(0x202124)).child(
            surface(self.surface.clone())
                .object_fit(ObjectFit::Contain)
                .size_full(),
        )
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let _ = log::set_logger(&SMOKE_LOGGER);
    log::set_max_level(log::LevelFilter::Debug);

    let runtime = tokio::runtime::Runtime::new()?;
    let _runtime_context = runtime.enter();
    let runtime_dir = Arc::new(tempfile::tempdir()?);
    let stage = Arc::new(StageWayland::start_sized(
        StageId("gpui-dma-buf-smoke".into()),
        runtime_dir.path(),
        720,
        480,
    )?);

    let surface = runtime.block_on(prepare_surface(Arc::clone(&stage)))?;
    application().run(move |cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(760.), px(540.)), cx);
        let stage = Arc::clone(&stage);
        let runtime_dir = Arc::clone(&runtime_dir);
        let surface = surface.clone();
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                app_id: Some("dev.artist.dmabuf-smoke".into()),
                ..Default::default()
            },
            move |_, cx| {
                cx.new(|_| DmaBufSmoke {
                    surface,
                    _stage: stage,
                    _runtime_dir: runtime_dir,
                })
            },
        )
        .expect("open DMA-BUF smoke window");
        cx.activate(true);
    });
    Ok(())
}

async fn prepare_surface(
    stage: Arc<StageWayland>,
) -> Result<DmaBufSurface, Box<dyn std::error::Error>> {
    let command = AppCommand::new("zenity")
        .arg("--info")
        .arg("--title=Artist DMA-BUF smoke")
        .arg("--text=This dialog is rendered by the private Artist compositor and imported into GPUI without pixel readback.")
        .arg("--width=520")
        .arg("--height=240");
    stage.spawn(command).await?;

    for _ in 0..100 {
        if stage
            .windows()
            .await
            .map(|windows| windows.iter().any(|window| window.mapped))
            .unwrap_or(false)
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let export = stage.export().await?;
    let buffer_index = export.lease.current_buffer();
    if !export.lease.hold(buffer_index) {
        return Err(format!("stage buffer {buffer_index} was already leased").into());
    }
    let buffer = export
        .buffers
        .into_iter()
        .find(|buffer| buffer.index == buffer_index)
        .ok_or_else(|| format!("stage did not export front buffer {buffer_index}"))?;
    let lease = export.lease.clone();
    let completion = release_callback(lease, buffer_index);
    let planes = buffer
        .planes
        .into_iter()
        .map(|plane| DmaBufPlane::new(plane.fd, plane.offset, plane.stride))
        .collect();

    Ok(DmaBufSurface::new(
        buffer.width,
        buffer.height,
        buffer.format,
        buffer.modifier,
        planes,
        Some(completion),
    ))
}

fn release_callback(lease: StageLease, buffer_index: u32) -> Arc<dyn Fn() + Send + Sync> {
    Arc::new(move || {
        lease.release(buffer_index);
    })
}
