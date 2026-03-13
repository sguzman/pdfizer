use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use config::{Config, Environment, File};
use directories::ProjectDirs;
use eframe::egui::{self, Color32, ViewportBuilder};
use serde::{Deserialize, Serialize};

const ENV_PREFIX: &str = "PDFIZER";

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct AppConfig {
    pub mode: AppMode,
    pub window: WindowConfig,
    pub startup: StartupConfig,
    pub pdfium: PdfiumConfig,
    pub rendering: RenderingConfig,
    pub ui: UiConfig,
    pub logging: LoggingConfig,
    pub storage: StorageConfig,
    pub tts: TtsConfig,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            mode: AppMode::default(),
            window: WindowConfig::default(),
            startup: StartupConfig::default(),
            pdfium: PdfiumConfig::default(),
            rendering: RenderingConfig::default(),
            ui: UiConfig::default(),
            logging: LoggingConfig::default(),
            storage: StorageConfig::default(),
            tts: TtsConfig::default(),
        }
    }
}

impl AppConfig {
    pub fn load() -> Result<Self> {
        Self::load_from_paths(Self::candidate_paths())
    }

    pub fn load_from_paths(paths: Vec<PathBuf>) -> Result<Self> {
        let defaults = Self::default();

        let mut builder = Config::builder()
            .set_default("mode", defaults.mode.as_str())?
            .set_default("window.title", defaults.window.title.clone())?
            .set_default("window.width", f64::from(defaults.window.width))?
            .set_default("window.height", f64::from(defaults.window.height))?
            .set_default("window.min_width", f64::from(defaults.window.min_width))?
            .set_default("window.min_height", f64::from(defaults.window.min_height))?
            .set_default(
                "startup.open_last_document",
                defaults.startup.open_last_document,
            )?
            .set_default(
                "startup.restore_last_page",
                defaults.startup.restore_last_page,
            )?
            .set_default(
                "startup.preferred_config_name",
                defaults.startup.preferred_config_name.clone(),
            )?
            .set_default(
                "startup.reopen_last_document_on_launch",
                defaults.startup.reopen_last_document_on_launch,
            )?
            .set_default("pdfium.library_path", defaults.pdfium.library_path.clone())?
            .set_default(
                "pdfium.library_env_var",
                defaults.pdfium.library_env_var.clone(),
            )?
            .set_default(
                "rendering.initial_zoom",
                f64::from(defaults.rendering.initial_zoom),
            )?
            .set_default("rendering.max_zoom", f64::from(defaults.rendering.max_zoom))?
            .set_default("rendering.min_zoom", f64::from(defaults.rendering.min_zoom))?
            .set_default(
                "rendering.texture_filter",
                defaults.rendering.texture_filter.as_str(),
            )?
            .set_default(
                "rendering.preferred_bg_hex",
                defaults.rendering.preferred_bg_hex.clone(),
            )?
            .set_default(
                "rendering.thumbnail_size",
                defaults.rendering.thumbnail_size,
            )?
            .set_default("rendering.tile_size", defaults.rendering.tile_size)?
            .set_default(
                "rendering.tile_render_min_width",
                defaults.rendering.tile_render_min_width,
            )?
            .set_default(
                "rendering.cache_zoom_bucket",
                f64::from(defaults.rendering.cache_zoom_bucket),
            )?
            .set_default(
                "rendering.default_preset",
                defaults.rendering.default_preset.clone(),
            )?
            .set_default(
                "rendering.compare_presets",
                defaults.rendering.compare_presets.clone(),
            )?
            .set_default(
                "ui.left_panel_width",
                f64::from(defaults.ui.left_panel_width),
            )?
            .set_default(
                "ui.bottom_panel_height",
                f64::from(defaults.ui.bottom_panel_height),
            )?
            .set_default(
                "ui.thumbnail_panel_width",
                f64::from(defaults.ui.thumbnail_panel_width),
            )?
            .set_default("ui.show_metrics", defaults.ui.show_metrics)?
            .set_default("ui.show_logs_hint", defaults.ui.show_logs_hint)?
            .set_default("ui.show_thumbnails", defaults.ui.show_thumbnails)?
            .set_default(
                "ui.enable_pixel_inspector",
                defaults.ui.enable_pixel_inspector,
            )?
            .set_default("ui.compare_mode_default", defaults.ui.compare_mode_default)?
            .set_default("logging.level", defaults.logging.level.clone())?
            .set_default("logging.span_events", defaults.logging.span_events)?
            .set_default("logging.write_to_file", defaults.logging.write_to_file)?
            .set_default("logging.file_name", defaults.logging.file_name.clone())?
            .set_default("storage.persist_session", defaults.storage.persist_session)?
            .set_default(
                "storage.session_file",
                defaults.storage.session_file.clone(),
            )?
            .set_default(
                "storage.benchmark_dir",
                defaults.storage.benchmark_dir.clone(),
            )?
            .set_default(
                "storage.benchmark_prefix",
                defaults.storage.benchmark_prefix.clone(),
            )?
            .set_default("storage.log_dir", defaults.storage.log_dir.clone())?
            .set_default("tts.enabled", defaults.tts.enabled)?
            .set_default(
                "tts.auto_analyze_on_open",
                defaults.tts.auto_analyze_on_open,
            )?
            .set_default(
                "tts.normalizer_config_path",
                defaults.tts.normalizer_config_path.clone(),
            )?
            .set_default(
                "tts.abbreviations_config_path",
                defaults.tts.abbreviations_config_path.clone(),
            )?
            .set_default("tts.language", defaults.tts.language.clone())?
            .set_default("tts.engine", defaults.tts.engine.clone())?
            .set_default("tts.voice", defaults.tts.voice.clone())?
            .set_default("tts.rate", f64::from(defaults.tts.rate))?
            .set_default("tts.volume", f64::from(defaults.tts.volume))?
            .set_default(
                "tts.sentence_boundary_markers",
                defaults.tts.sentence_boundary_markers.clone(),
            )?
            .set_default(
                "tts.sentence_break_on_double_newline",
                defaults.tts.sentence_break_on_double_newline,
            )?
            .set_default(
                "tts.min_sentence_chars",
                defaults.tts.min_sentence_chars as i64,
            )?
            .set_default(
                "tts.block_fallback_min_chars",
                defaults.tts.block_fallback_min_chars as i64,
            )?
            .set_default(
                "tts.line_merge_vertical_tolerance",
                f64::from(defaults.tts.line_merge_vertical_tolerance),
            )?
            .set_default(
                "tts.column_split_min_gap",
                f64::from(defaults.tts.column_split_min_gap),
            )?
            .set_default(
                "tts.column_detection_min_lines",
                defaults.tts.column_detection_min_lines as i64,
            )?
            .set_default(
                "tts.block_vertical_gap_multiplier",
                f64::from(defaults.tts.block_vertical_gap_multiplier),
            )?
            .set_default(
                "tts.suppress_rotated_narrow_segments",
                defaults.tts.suppress_rotated_narrow_segments,
            )?
            .set_default(
                "tts.rotated_segment_aspect_ratio",
                f64::from(defaults.tts.rotated_segment_aspect_ratio),
            )?
            .set_default("tts.ocr_policy", defaults.tts.ocr_policy.as_str())?
            .set_default(
                "tts.ocr_min_confidence",
                f64::from(defaults.tts.ocr_min_confidence),
            )?
            .set_default(
                "tts.sentence_prefetch",
                defaults.tts.sentence_prefetch as i64,
            )?
            .set_default(
                "tts.clip_budget_sentences",
                defaults.tts.clip_budget_sentences as i64,
            )?
            .set_default(
                "tts.sync_budget_sentences",
                defaults.tts.sync_budget_sentences as i64,
            )?
            .set_default(
                "tts.active_latency_budget_ms",
                defaults.tts.active_latency_budget_ms as i64,
            )?
            .set_default(
                "tts.prefetch_duration_budget_ms",
                defaults.tts.prefetch_duration_budget_ms as i64,
            )?
            .set_default(
                "tts.sentence_pause_ms",
                defaults.tts.sentence_pause_ms as i64,
            )?
            .set_default(
                "tts.analysis_max_pages",
                defaults.tts.analysis_max_pages as i64,
            )?
            .set_default(
                "tts.analysis_window_radius",
                defaults.tts.analysis_window_radius as i64,
            )?
            .set_default(
                "tts.follow_visible_margin_ratio",
                f64::from(defaults.tts.follow_visible_margin_ratio),
            )?
            .set_default(
                "tts.follow_preload_page_radius",
                defaults.tts.follow_preload_page_radius as i64,
            )?
            .set_default(
                "tts.follow_center_on_target",
                defaults.tts.follow_center_on_target,
            )?
            .set_default(
                "tts.exact_sync_min_score",
                f64::from(defaults.tts.exact_sync_min_score),
            )?
            .set_default(
                "tts.fuzzy_sync_min_score",
                f64::from(defaults.tts.fuzzy_sync_min_score),
            )?
            .set_default(
                "tts.block_sync_min_score",
                f64::from(defaults.tts.block_sync_min_score),
            )?
            .set_default("tts.audio_cache_dir", defaults.tts.audio_cache_dir.clone())?
            .set_default("tts.artifacts_dir", defaults.tts.artifacts_dir.clone())?
            .set_default(
                "tts.sync_artifacts_dir",
                defaults.tts.sync_artifacts_dir.clone(),
            )?
            .set_default(
                "tts.ocr_artifacts_dir",
                defaults.tts.ocr_artifacts_dir.clone(),
            )?
            .set_default(
                "tts.highlight_exact_rgba",
                defaults.tts.highlight_exact_rgba.clone(),
            )?
            .set_default(
                "tts.highlight_fuzzy_rgba",
                defaults.tts.highlight_fuzzy_rgba.clone(),
            )?
            .set_default(
                "tts.highlight_block_rgba",
                defaults.tts.highlight_block_rgba.clone(),
            )?
            .set_default(
                "tts.highlight_page_rgba",
                defaults.tts.highlight_page_rgba.clone(),
            )?
            .set_default(
                "tts.highlight_stroke_width",
                f64::from(defaults.tts.highlight_stroke_width),
            )?
            .set_default(
                "tts.highlight_page_margin",
                f64::from(defaults.tts.highlight_page_margin),
            )?
            .set_default(
                "tts.experimental_pdf_sync",
                defaults.tts.experimental_pdf_sync,
            )?
            .set_default(
                "tts.verbose_degraded_logging",
                defaults.tts.verbose_degraded_logging,
            )?
            .set_default(
                "tts.min_chars_per_text_page",
                defaults.tts.min_chars_per_text_page as i64,
            )?
            .set_default(
                "tts.min_segments_per_text_page",
                defaults.tts.min_segments_per_text_page as i64,
            )?
            .set_default(
                "tts.min_text_page_ratio",
                f64::from(defaults.tts.min_text_page_ratio),
            )?
            .set_default(
                "tts.max_duplicate_line_ratio",
                f64::from(defaults.tts.max_duplicate_line_ratio),
            )?
            .set_default(
                "tts.max_repeated_edge_line_ratio",
                f64::from(defaults.tts.max_repeated_edge_line_ratio),
            )?
            .set_default(
                "tts.repeated_edge_line_min_pages",
                defaults.tts.repeated_edge_line_min_pages as i64,
            )?
            .set_default(
                "tts.page_edge_line_scan_depth",
                defaults.tts.page_edge_line_scan_depth as i64,
            )?
            .set_default(
                "tts.max_edge_line_length",
                defaults.tts.max_edge_line_length as i64,
            )?
            .set_default(
                "tts.min_chars_per_line_kept",
                defaults.tts.min_chars_per_line_kept as i64,
            )?
            .set_default("tts.abbreviations", defaults.tts.abbreviations.clone())?;

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
            PathBuf::from("config/default.toml"),
            PathBuf::from("config/pdfizer.toml"),
        ];

        if let Some(project_dirs) = Self::project_dirs() {
            paths.push(project_dirs.config_dir().join("pdfizer.toml"));
        }

        paths
    }

    pub fn project_dirs() -> Option<ProjectDirs> {
        ProjectDirs::from("dev", "pdfizer", "pdfizer")
    }

    pub fn config_preview(&self) -> String {
        toml::to_string_pretty(self).unwrap_or_else(|_| "<failed to serialize config>".into())
    }

    pub fn is_dev(&self) -> bool {
        self.mode == AppMode::Dev
    }

    pub fn preferred_config_path(&self) -> Result<PathBuf> {
        let path = PathBuf::from(&self.startup.preferred_config_name);

        if path.is_absolute() {
            return Ok(path);
        }

        Ok(std::env::current_dir()
            .context("failed to get current directory for config save path")?
            .join(path))
    }

    pub fn session_path(&self) -> Result<PathBuf> {
        let path = self.resolve_storage_path(&self.storage.session_file)?;
        ensure_parent_dir(&path)?;
        Ok(path)
    }

    pub fn benchmark_dir(&self) -> Result<PathBuf> {
        let path = self.resolve_storage_dir(&self.storage.benchmark_dir)?;
        fs::create_dir_all(&path)
            .with_context(|| format!("failed to create benchmark directory {}", path.display()))?;
        Ok(path)
    }

    pub fn log_file_path(&self) -> Result<PathBuf> {
        let dir = if self.is_dev() {
            std::env::current_dir()
                .context("failed to get current directory for dev log path")?
                .join("logs")
        } else {
            self.resolve_storage_dir(&self.storage.log_dir)?
        };
        fs::create_dir_all(&dir)
            .with_context(|| format!("failed to create log directory {}", dir.display()))?;

        let file_name = if self.is_dev() {
            format!("pdfizer-{}.log", unix_timestamp_secs())
        } else {
            self.logging.file_name.clone()
        };

        Ok(dir.join(file_name))
    }

    pub fn tts_artifacts_dir(&self) -> Result<PathBuf> {
        let path = self.resolve_storage_dir(&self.tts.artifacts_dir)?;
        fs::create_dir_all(&path).with_context(|| {
            format!("failed to create TTS artifact directory {}", path.display())
        })?;
        Ok(path)
    }

    pub fn tts_audio_cache_dir(&self) -> Result<PathBuf> {
        let path = self.resolve_storage_dir(&self.tts.audio_cache_dir)?;
        fs::create_dir_all(&path).with_context(|| {
            format!(
                "failed to create TTS audio cache directory {}",
                path.display()
            )
        })?;
        Ok(path)
    }

    pub fn tts_sync_artifacts_dir(&self) -> Result<PathBuf> {
        let path = self.resolve_storage_dir(&self.tts.sync_artifacts_dir)?;
        fs::create_dir_all(&path).with_context(|| {
            format!(
                "failed to create TTS sync artifact directory {}",
                path.display()
            )
        })?;
        Ok(path)
    }

    pub fn tts_ocr_artifacts_dir(&self) -> Result<PathBuf> {
        let path = self.resolve_storage_dir(&self.tts.ocr_artifacts_dir)?;
        fs::create_dir_all(&path).with_context(|| {
            format!(
                "failed to create TTS OCR artifact directory {}",
                path.display()
            )
        })?;
        Ok(path)
    }

    pub fn tts_artifact_path(&self, source_path: &Path) -> Result<PathBuf> {
        let dir = self.tts_artifacts_dir()?;
        Ok(dir.join(format!("{}.toml", stable_source_fingerprint(source_path)?)))
    }

    pub fn tts_sync_artifact_path(&self, source_path: &Path, sentence_id: u64) -> Result<PathBuf> {
        let dir = self.tts_sync_artifacts_dir()?;
        Ok(dir.join(format!(
            "{}-{:016x}.toml",
            stable_source_fingerprint(source_path)?,
            sentence_id
        )))
    }

    pub fn tts_ocr_artifact_path(&self, source_path: &Path) -> Result<PathBuf> {
        let dir = self.tts_ocr_artifacts_dir()?;
        Ok(dir.join(format!("{}.toml", stable_source_fingerprint(source_path)?)))
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

    fn resolve_storage_dir(&self, value: &str) -> Result<PathBuf> {
        let path = PathBuf::from(value);

        if path.is_absolute() {
            return Ok(path);
        }

        if let Some(project_dirs) = Self::project_dirs() {
            return Ok(project_dirs.data_local_dir().join(path));
        }

        Ok(std::env::current_dir()
            .context("failed to get current directory for storage path")?
            .join(path))
    }

    fn resolve_storage_path(&self, value: &str) -> Result<PathBuf> {
        let path = PathBuf::from(value);

        if path.is_absolute() {
            return Ok(path);
        }

        if let Some(project_dirs) = Self::project_dirs() {
            return Ok(project_dirs.data_local_dir().join(path));
        }

        Ok(std::env::current_dir()
            .context("failed to get current directory for storage file path")?
            .join(path))
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

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum AppMode {
    #[default]
    Prod,
    Dev,
}

impl AppMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Prod => "prod",
            Self::Dev => "dev",
        }
    }
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
    pub reopen_last_document_on_launch: bool,
}

impl Default for StartupConfig {
    fn default() -> Self {
        Self {
            open_last_document: false,
            restore_last_page: false,
            preferred_config_name: "config/pdfizer.toml".into(),
            reopen_last_document_on_launch: true,
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
    pub thumbnail_size: i32,
    pub tile_size: i32,
    pub tile_render_min_width: i32,
    pub cache_zoom_bucket: f32,
    pub default_preset: String,
    pub compare_presets: Vec<String>,
}

impl Default for RenderingConfig {
    fn default() -> Self {
        Self {
            initial_zoom: 1.25,
            min_zoom: 0.25,
            max_zoom: 4.0,
            texture_filter: TextureFilterName::Linear,
            preferred_bg_hex: "#1f1f24".into(),
            thumbnail_size: 180,
            tile_size: 512,
            tile_render_min_width: 2200,
            cache_zoom_bucket: 0.05,
            default_preset: "balanced".into(),
            compare_presets: vec!["balanced".into(), "crisp".into()],
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
    pub thumbnail_panel_width: f32,
    pub show_metrics: bool,
    pub show_logs_hint: bool,
    pub show_thumbnails: bool,
    pub enable_pixel_inspector: bool,
    pub compare_mode_default: bool,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            left_panel_width: 280.0,
            bottom_panel_height: 220.0,
            thumbnail_panel_width: 220.0,
            show_metrics: true,
            show_logs_hint: true,
            show_thumbnails: true,
            enable_pixel_inspector: true,
            compare_mode_default: false,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct LoggingConfig {
    pub level: String,
    pub span_events: bool,
    pub write_to_file: bool,
    pub file_name: String,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: "debug".into(),
            span_events: false,
            write_to_file: true,
            file_name: "pdfizer.log".into(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct StorageConfig {
    pub persist_session: bool,
    pub session_file: String,
    pub benchmark_dir: String,
    pub benchmark_prefix: String,
    pub log_dir: String,
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            persist_session: true,
            session_file: "session.toml".into(),
            benchmark_dir: "benchmarks".into(),
            benchmark_prefix: "render-snapshot".into(),
            log_dir: "logs".into(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct TtsConfig {
    pub enabled: bool,
    pub auto_analyze_on_open: bool,
    pub normalizer_config_path: String,
    pub abbreviations_config_path: String,
    pub language: String,
    pub engine: String,
    pub voice: String,
    pub rate: f32,
    pub volume: f32,
    pub sentence_boundary_markers: Vec<String>,
    pub sentence_break_on_double_newline: bool,
    pub min_sentence_chars: usize,
    pub block_fallback_min_chars: usize,
    pub line_merge_vertical_tolerance: f32,
    pub column_split_min_gap: f32,
    pub column_detection_min_lines: usize,
    pub block_vertical_gap_multiplier: f32,
    pub suppress_rotated_narrow_segments: bool,
    pub rotated_segment_aspect_ratio: f32,
    pub ocr_policy: TtsOcrPolicy,
    pub ocr_min_confidence: f32,
    pub sentence_prefetch: usize,
    pub clip_budget_sentences: usize,
    pub sync_budget_sentences: usize,
    pub active_latency_budget_ms: u64,
    pub prefetch_duration_budget_ms: u64,
    pub sentence_pause_ms: u64,
    pub analysis_max_pages: usize,
    pub analysis_window_radius: usize,
    pub follow_visible_margin_ratio: f32,
    pub follow_preload_page_radius: usize,
    pub follow_center_on_target: bool,
    pub exact_sync_min_score: f32,
    pub fuzzy_sync_min_score: f32,
    pub block_sync_min_score: f32,
    pub audio_cache_dir: String,
    pub artifacts_dir: String,
    pub sync_artifacts_dir: String,
    pub ocr_artifacts_dir: String,
    pub highlight_exact_rgba: String,
    pub highlight_fuzzy_rgba: String,
    pub highlight_block_rgba: String,
    pub highlight_page_rgba: String,
    pub highlight_stroke_width: f32,
    pub highlight_page_margin: f32,
    pub experimental_pdf_sync: bool,
    pub verbose_degraded_logging: bool,
    pub min_chars_per_text_page: usize,
    pub min_segments_per_text_page: usize,
    pub min_text_page_ratio: f32,
    pub max_duplicate_line_ratio: f32,
    pub max_repeated_edge_line_ratio: f32,
    pub repeated_edge_line_min_pages: usize,
    pub page_edge_line_scan_depth: usize,
    pub max_edge_line_length: usize,
    pub min_chars_per_line_kept: usize,
    pub abbreviations: Vec<String>,
}

impl Default for TtsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            auto_analyze_on_open: true,
            normalizer_config_path: "config/normalizer.toml".into(),
            abbreviations_config_path: "config/abbreviations.toml".into(),
            language: "en".into(),
            engine: "tone_preview".into(),
            voice: "default".into(),
            rate: 1.0,
            volume: 1.0,
            sentence_boundary_markers: vec![".".into(), "!".into(), "?".into()],
            sentence_break_on_double_newline: true,
            min_sentence_chars: 8,
            block_fallback_min_chars: 32,
            line_merge_vertical_tolerance: 0.75,
            column_split_min_gap: 96.0,
            column_detection_min_lines: 4,
            block_vertical_gap_multiplier: 1.8,
            suppress_rotated_narrow_segments: true,
            rotated_segment_aspect_ratio: 3.0,
            ocr_policy: TtsOcrPolicy::Deferred,
            ocr_min_confidence: 0.85,
            sentence_prefetch: 8,
            clip_budget_sentences: 16,
            sync_budget_sentences: 12,
            active_latency_budget_ms: 120,
            prefetch_duration_budget_ms: 30_000,
            sentence_pause_ms: 140,
            analysis_max_pages: 96,
            analysis_window_radius: 32,
            follow_visible_margin_ratio: 0.18,
            follow_preload_page_radius: 1,
            follow_center_on_target: true,
            exact_sync_min_score: 0.82,
            fuzzy_sync_min_score: 0.58,
            block_sync_min_score: 0.28,
            audio_cache_dir: "tts/audio".into(),
            artifacts_dir: "tts/artifacts".into(),
            sync_artifacts_dir: "tts/sync".into(),
            ocr_artifacts_dir: "tts/ocr".into(),
            highlight_exact_rgba: "#FF785038".into(),
            highlight_fuzzy_rgba: "#FFB05030".into(),
            highlight_block_rgba: "#FFDC502C".into(),
            highlight_page_rgba: "#B4B4B420".into(),
            highlight_stroke_width: 2.0,
            highlight_page_margin: 6.0,
            experimental_pdf_sync: false,
            verbose_degraded_logging: true,
            min_chars_per_text_page: 80,
            min_segments_per_text_page: 24,
            min_text_page_ratio: 0.6,
            max_duplicate_line_ratio: 0.35,
            max_repeated_edge_line_ratio: 0.35,
            repeated_edge_line_min_pages: 3,
            page_edge_line_scan_depth: 3,
            max_edge_line_length: 96,
            min_chars_per_line_kept: 2,
            abbreviations: vec![
                "mr.".into(),
                "mrs.".into(),
                "ms.".into(),
                "dr.".into(),
                "prof.".into(),
                "etc.".into(),
                "e.g.".into(),
                "i.e.".into(),
            ],
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TtsOcrPolicy {
    Disabled,
    Deferred,
    RequireArtifacts,
}

impl TtsOcrPolicy {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::Deferred => "deferred",
            Self::RequireArtifacts => "require_artifacts",
        }
    }
}

impl Default for TtsOcrPolicy {
    fn default() -> Self {
        Self::Deferred
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

fn ensure_parent_dir(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }

    Ok(())
}

fn unix_timestamp_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn stable_source_fingerprint(path: &Path) -> Result<String> {
    use std::hash::{DefaultHasher, Hash, Hasher};

    let metadata = fs::metadata(path)
        .with_context(|| format!("failed to read metadata for {}", path.display()))?;
    let modified = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs())
        .unwrap_or(0);

    let mut hasher = DefaultHasher::new();
    path.display().to_string().hash(&mut hasher);
    metadata.len().hash(&mut hasher);
    modified.hash(&mut hasher);
    Ok(format!("{:016x}", hasher.finish()))
}
