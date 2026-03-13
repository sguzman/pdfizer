mod app;
mod config;
mod pdf;

use anyhow::{Result, anyhow};
use app::PdfizerApp;
use config::AppConfig;
use tracing::{debug, info};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};

fn main() -> Result<()> {
    let config = AppConfig::load()?;
    let _guard = init_tracing(&config)?;
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

fn init_tracing(config: &AppConfig) -> Result<Option<WorkerGuard>> {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(format!("pdfizer={},info", config.logging.level)));

    let stdout_layer = tracing_subscriber::fmt::layer().compact().with_target(true);

    if config.is_dev() || config.logging.write_to_file {
        let path = config.log_file_path()?;
        let directory = path
            .parent()
            .ok_or_else(|| anyhow!("invalid log file path {}", path.display()))?;
        let file_name = path
            .file_name()
            .ok_or_else(|| anyhow!("invalid log file name {}", path.display()))?
            .to_string_lossy()
            .to_string();
        let appender = tracing_appender::rolling::never(directory, file_name);
        let (non_blocking, guard) = tracing_appender::non_blocking(appender);
        let file_layer = tracing_subscriber::fmt::layer()
            .compact()
            .with_ansi(false)
            .with_writer(non_blocking);

        tracing_subscriber::registry()
            .with(filter)
            .with(stdout_layer)
            .with(file_layer)
            .init();

        Ok(Some(guard))
    } else {
        tracing_subscriber::registry()
            .with(filter)
            .with(stdout_layer)
            .init();

        Ok(None)
    }
}
