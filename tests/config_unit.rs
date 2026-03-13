use super::*;
use std::fs;
use tempfile::tempdir;

#[test]
fn default_config_has_sane_zoom_range() {
    let config = AppConfig::default();

    assert!(config.rendering.min_zoom < config.rendering.initial_zoom);
    assert!(config.rendering.initial_zoom < config.rendering.max_zoom);
    assert_eq!(config.rendering.compare_presets.len(), 2);
    assert_eq!(config.mode, AppMode::Prod);
    assert!(config.tts.enabled);
    assert_eq!(config.tts.sentence_prefetch, 8);
    assert_eq!(config.tts.ocr_policy, TtsOcrPolicy::Deferred);
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
            thumbnail_size = 128
            tile_size = 256
            tile_render_min_width = 1024
            cache_zoom_bucket = 0.1
            default_preset = "crisp"
            compare_presets = ["balanced", "grayscale"]

            [tts]
            sentence_prefetch = 12
            language = "es"
            engine = "dry_run"
            ocr_policy = "disabled"
        "##,
    )
    .unwrap();

    let config = AppConfig::load_from_paths(vec![config_path]).unwrap();

    assert_eq!(config.window.title, "Lab Build");
    assert_eq!(config.rendering.initial_zoom, 2.0);
    assert_eq!(config.rendering.texture_filter, TextureFilterName::Nearest);
    assert_eq!(config.rendering.default_preset, "crisp");
    assert_eq!(config.tts.sentence_prefetch, 12);
    assert_eq!(config.tts.language, "es");
    assert_eq!(config.tts.engine, "dry_run");
    assert_eq!(config.tts.ocr_policy, TtsOcrPolicy::Disabled);
}

#[test]
fn config_preview_is_toml() {
    let preview = AppConfig::default().config_preview();
    assert!(preview.contains("mode = \"prod\""));
    assert!(preview.contains("[window]"));
    assert!(preview.contains("[rendering]"));
    assert!(preview.contains("[storage]"));
    assert!(preview.contains("[tts]"));
}

#[test]
fn dev_mode_uses_timestamped_log_name() {
    let mut config = AppConfig::default();
    config.mode = AppMode::Dev;

    let path = config.log_file_path().unwrap();
    let file_name = path.file_name().unwrap().to_string_lossy();

    assert!(path.to_string_lossy().contains("/logs/") || path.to_string_lossy().contains("\\logs\\"));
    assert!(file_name.starts_with("pdfizer-"));
    assert!(file_name.ends_with(".log"));
}

#[test]
fn tts_artifact_path_is_stable() {
    let temp = tempdir().unwrap();
    let source_path = temp.path().join("fixture.pdf");
    fs::write(&source_path, b"%PDF-1.4").unwrap();

    let config = AppConfig::default();
    let first = config.tts_artifact_path(&source_path).unwrap();
    let second = config.tts_artifact_path(&source_path).unwrap();

    assert_eq!(first, second);
    assert!(first.to_string_lossy().contains("tts"));
}
