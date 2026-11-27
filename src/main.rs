pub use velox_wm::{apps, config, font, mem_cache, notifications, system_ctrl, updates};

mod animation;
mod backend;
mod cursor;
mod grabs;
mod handlers;
mod shell_ui;
mod state;
mod wallpaper;
mod xwayland;

use std::os::unix::process::CommandExt;

use tracing_subscriber::EnvFilter;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    if let Ok(env_filter) = EnvFilter::try_from_default_env() {
        tracing_subscriber::fmt().with_env_filter(env_filter).init();
    } else {
        tracing_subscriber::fmt()
            .with_env_filter(EnvFilter::new("info"))
            .init();
    }

    let args: Vec<String> = std::env::args().collect();
    let force_nested = args.iter().any(|a| a == "--nested");
    let force_drm = args.iter().any(|a| a == "--drm");

    let has_parent_display =
        std::env::var_os("WAYLAND_DISPLAY").is_some() || std::env::var_os("DISPLAY").is_some();

    let reload = if force_nested || (!force_drm && has_parent_display) {
        tracing::info!("Starting velox-wm in nested winit mode");
        backend::winit::run_winit()?
    } else {
        tracing::info!("Starting velox-wm in bare-metal DRM/udev mode");
        backend::udev::run_udev()?
    };

    if reload {
        if std::env::var("VELOX_SUPERVISED").as_deref() == Ok("1") {
            std::process::exit(updates::SEAMLESS_RELOAD_EXIT_CODE);
        }
        let target_bin = if std::path::Path::new("/usr/local/bin/velox-wm").exists() {
            std::path::PathBuf::from("/usr/local/bin/velox-wm")
        } else {
            std::env::current_exe().unwrap_or_else(|_| std::path::PathBuf::from("velox-wm"))
        };
        tracing::info!(?target_bin, "Re-execing updated velox-wm seamlessly");
        let err = std::process::Command::new(&target_bin)
            .args(args.iter().skip(1))
            .exec();
        return Err(Box::new(err));
    }

    Ok(())
}
