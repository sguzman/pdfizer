use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use config::{Config, Environment, File};
use directories::ProjectDirs;
use eframe::egui::{self, Color32, ViewportBuilder};
use serde::{Deserialize, Serialize};

const ENV_PREFIX: &str = "PDFIZER";

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct AppConfig {
    pub window: WindowConfig,
    pub startup: StartupConfig,
    pub pdfium: PdfiumConfig,
    pub rendering: RenderingConfig,
    pub ui: UiConfig,
    pub logging: LoggingConfig,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            window: WindowConfig::default(),
            startup: StartupConfig::default(),
            pdfium: PdfiumConfig::default(),
            rendering: RenderingConfig::default(),
            ui: UiConfig::default(),
            logging: LoggingConfig::default(),
        }
    }
}

impl AppConfig {
    pub fn load() -> Result<Self> {
        Self::load_from_paths(Self::candidate_paths())
    }

    pub fn load_from_paths(paths: Vec<PathBuf>) -> Result<Self> {
        let window = WindowConfig::default();
        let startup = StartupConfig::default();
        let pdfium = PdfiumConfig::default();
        let rendering = RenderingConfig::default();
        let ui = UiConfig::default();
        let logging = LoggingConfig::default();

        let mut builder = Config::builder()
            .set_default("window.title", window.title)?
            .set_default("window.width", f64::from(window.width))?
            .set_default("window.height", f64::from(window.height))?
            .set_default("window.min_width", f64::from(window.min_width))?
            .set_default("window.min_height", f64::from(window.min_height))?
            .set_default("startup.open_last_document", startup.open_last_document)?
            .set_default("startup.restore_last_page", startup.restore_last_page)?
            .set_default(
                "startup.preferred_config_name",
                startup.preferred_config_name,
            )?
            .set_default("pdfium.library_path", pdfium.library_path)?
            .set_default("pdfium.library_env_var", pdfium.library_env_var)?
            .set_default("rendering.initial_zoom", f64::from(rendering.initial_zoom))?
            .set_default("rendering.max_zoom", f64::from(rendering.max_zoom))?
            .set_default("rendering.min_zoom", f64::from(rendering.min_zoom))?
            .set_default(
                "rendering.texture_filter",
                rendering.texture_filter.as_str(),
            )?
            .set_default("rendering.preferred_bg_hex", rendering.preferred_bg_hex)?
            .set_default("ui.left_panel_width", f64::from(ui.left_panel_width))?
            .set_default("ui.bottom_panel_height", f64::from(ui.bottom_panel_height))?
            .set_default("ui.show_metrics", ui.show_metrics)?
            .set_default("ui.show_logs_hint", ui.show_logs_hint)?
            .set_default("logging.level", logging.level)?
            .set_default("logging.span_events", logging.span_events)?;

        for path in paths {
            builder = builder.add_source(File::from(path).required(false));
        }

        let settings = builder
            .add_source(Environment::with_prefix(ENV_PREFIX).separator("__"))
            .build()
            .context("failed to assemble layered configuration")?;

        settings
            .try_deserialize()
            .context("failed to deserialize application configuration")
    }

    pub fn candidate_paths() -> Vec<PathBuf> {
        let mut paths = vec![
            PathBuf::from("config/pdfizer.toml"),
            PathBuf::from("config/default.toml"),
        ];

        if let Some(project_dirs) = ProjectDirs::from("dev", "pdfizer", "pdfizer") {
            paths.push(project_dirs.config_dir().join("pdfizer.toml"));
        }

        paths
    }

    pub fn config_preview(&self) -> String {
        toml::to_string_pretty(self).unwrap_or_else(|_| "<failed to serialize config>".into())
    }

    pub fn to_native_options(&self) -> eframe::NativeOptions {
        eframe::NativeOptions {
            viewport: ViewportBuilder::default()
                .with_inner_size([self.window.width, self.window.height])
                .with_min_inner_size([self.window.min_width, self.window.min_height])
                .with_title(self.window.title.clone()),
            ..Default::default()
        }
    }

    pub fn background_color(&self) -> Color32 {
        parse_hex_color(&self.rendering.preferred_bg_hex).unwrap_or(Color32::from_gray(30))
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct WindowConfig {
    pub title: String,
    pub width: f32,
    pub height: f32,
    pub min_width: f32,
    pub min_height: f32,
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self {
            title: "Pdfizer".into(),
            width: 1600.0,
            height: 960.0,
            min_width: 960.0,
            min_height: 640.0,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct StartupConfig {
    pub open_last_document: bool,
    pub restore_last_page: bool,
    pub preferred_config_name: String,
}

impl Default for StartupConfig {
    fn default() -> Self {
        Self {
            open_last_document: false,
            restore_last_page: false,
            preferred_config_name: "config/pdfizer.toml".into(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct PdfiumConfig {
    pub library_path: Option<String>,
    pub library_env_var: String,
}

impl Default for PdfiumConfig {
    fn default() -> Self {
        Self {
            library_path: None,
            library_env_var: "PDFIUM_DYNAMIC_LIB_PATH".into(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct RenderingConfig {
    pub initial_zoom: f32,
    pub min_zoom: f32,
    pub max_zoom: f32,
    pub texture_filter: TextureFilterName,
    pub preferred_bg_hex: String,
}

impl Default for RenderingConfig {
    fn default() -> Self {
        Self {
            initial_zoom: 1.25,
            min_zoom: 0.25,
            max_zoom: 4.0,
            texture_filter: TextureFilterName::Linear,
            preferred_bg_hex: "#1f1f24".into(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum TextureFilterName {
    Nearest,
    Linear,
}

impl TextureFilterName {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Nearest => "nearest",
            Self::Linear => "linear",
        }
    }

    pub fn to_texture_options(&self) -> egui::TextureOptions {
        match self {
            Self::Nearest => egui::TextureOptions::NEAREST,
            Self::Linear => egui::TextureOptions::LINEAR,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct UiConfig {
    pub left_panel_width: f32,
    pub bottom_panel_height: f32,
    pub show_metrics: bool,
    pub show_logs_hint: bool,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            left_panel_width: 280.0,
            bottom_panel_height: 180.0,
            show_metrics: true,
            show_logs_hint: true,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct LoggingConfig {
    pub level: String,
    pub span_events: bool,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: "debug".into(),
            span_events: false,
        }
    }
}

fn parse_hex_color(value: &str) -> Option<Color32> {
    let trimmed = value.trim().trim_start_matches('#');

    if trimmed.len() != 6 {
        return None;
    }

    let bytes = u32::from_str_radix(trimmed, 16).ok()?;
    Some(Color32::from_rgb(
        ((bytes >> 16) & 0xff) as u8,
        ((bytes >> 8) & 0xff) as u8,
        (bytes & 0xff) as u8,
    ))
}

pub fn library_path_from_config_or_env(config: &PdfiumConfig) -> Option<PathBuf> {
    config
        .library_path
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .map(Path::new)
        .map(Path::to_path_buf)
        .or_else(|| {
            std::env::var(&config.library_env_var)
                .ok()
                .map(PathBuf::from)
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn default_config_has_sane_zoom_range() {
        let config = AppConfig::default();

        assert!(config.rendering.min_zoom < config.rendering.initial_zoom);
        assert!(config.rendering.initial_zoom < config.rendering.max_zoom);
    }

    #[test]
    fn layered_config_overrides_defaults() {
        let temp = tempdir().unwrap();
        let config_path = temp.path().join("pdfizer.toml");
        fs::write(
            &config_path,
            r##"
                [window]
                title = "Lab Build"

                [rendering]
                initial_zoom = 2.0
                min_zoom = 0.5
                max_zoom = 5.0
                texture_filter = "nearest"
                preferred_bg_hex = "#ffffff"
            "##,
        )
        .unwrap();

        let config = AppConfig::load_from_paths(vec![config_path]).unwrap();

        assert_eq!(config.window.title, "Lab Build");
        assert_eq!(config.rendering.initial_zoom, 2.0);
        assert_eq!(config.rendering.texture_filter, TextureFilterName::Nearest);
    }

    #[test]
    fn config_preview_is_toml() {
        let preview = AppConfig::default().config_preview();
        assert!(preview.contains("[window]"));
        assert!(preview.contains("[rendering]"));
    }
}
