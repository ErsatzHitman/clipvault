#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]
use anyhow::{Context, Result};
use clipvault::{
    clipboard,
    config::{Config, Paths},
    ui,
};
use fs2::FileExt;

fn main() {
    if let Err(error) = run() {
        eprintln!("ClipVault: {error:#}");
        std::process::exit(1);
    }
}
fn run() -> Result<()> {
    let args: Vec<_> = std::env::args().collect();
    if args.iter().any(|s| s == "--clipboard-self-test") {
        return clipboard::self_test();
    }
    if args.iter().any(|s| s == "--version") {
        println!("ClipVault {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    let root = if let Some(i) = args.iter().position(|s| s == "--data-dir") {
        args.get(i + 1)
            .context("--data-dir requires a path")?
            .into()
    } else {
        directories::ProjectDirs::from("io.github", "ErsatzHitman", "ClipVault")
            .context("Cannot locate application data directory")?
            .data_local_dir()
            .to_owned()
    };
    let paths = Paths::open(root)?;
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(paths.root.join("instance.lock"))?;
    lock.try_lock_exclusive()
        .context("ClipVault is already running. Use its global shortcut to open history.")?;
    let config = Config::load(&paths.config)?;
    if !paths.config.exists() {
        config.save(&paths.config)?;
    }
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("ClipVault")
            .with_app_id("clipvault")
            .with_inner_size([920.0, 620.0])
            .with_min_inner_size([680.0, 420.0]),
        ..Default::default()
    };
    eframe::run_native(
        "ClipVault",
        options,
        Box::new(move |cc| Ok(Box::new(ui::App::new(cc, paths, config)?))),
    )
    .map_err(|e| anyhow::anyhow!(e.to_string()))?;
    drop(lock);
    Ok(())
}
