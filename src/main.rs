mod app;
mod config;
mod pdf;

use anyhow::{Result, anyhow};
use app::PdfizerApp;
use config::AppConfig;
use tracing::{debug, info};
use tracing_subscriber::EnvFilter;

fn main() -> Result<()> {
    init_tracing();

    let config = AppConfig::load()?;
    info!(?config, "loaded application configuration");

    let native_options = config.to_native_options();
    let title = config.window.title.clone();

    eframe::run_native(
        &title,
        native_options,
        Box::new(move |cc| {
            debug!("creating Pdfizer application");
            Ok(Box::new(PdfizerApp::new(cc, config.clone())))
        }),
    )
    .map_err(|err| anyhow!(err.to_string()))?;

    Ok(())
}

fn init_tracing() {
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("pdfizer=debug,info"));

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(true)
        .compact()
        .init();
}
