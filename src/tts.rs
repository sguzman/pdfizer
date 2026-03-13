use std::{
    collections::{HashMap, HashSet},
    fs,
    hash::{DefaultHasher, Hash, Hasher},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tracing::{debug, info, instrument};

use crate::{
    config::{AppConfig, TtsOcrPolicy},
    pdf::{PdfDocument, PdfRectData, PdfRuntime},
};

const PDF_LIGATURES: [(&str, &str); 7] = [
    ("\u{FB00}", "ff"),
    ("\u{FB01}", "fi"),
    ("\u{FB02}", "fl"),
    ("\u{FB03}", "ffi"),
    ("\u{FB04}", "ffl"),
    ("\u{FB05}", "ft"),
    ("\u{FB06}", "st"),
];

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PdfTtsMode {
    HighTextTrust,
    MixedTextTrust,
    OcrRequired,
    RenderOnlyNoSync,
}

impl PdfTtsMode {
    pub fn label(&self) -> &'static str {
        match self {
            Self::HighTextTrust => "high_text_trust",
            Self::MixedTextTrust => "mixed_text_trust",
            Self::OcrRequired => "ocr_required",
            Self::RenderOnlyNoSync => "render_only_no_sync",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TextRange {
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PageRange {
    pub start_page: usize,
    pub end_page: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CanonicalTokenArtifact {
    pub page_index: usize,
    pub block_index: usize,
    pub line_index: usize,
    pub token_index: usize,
    pub text: String,
    pub range: TextRange,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CanonicalLineArtifact {
    pub page_index: usize,
    pub block_index: usize,
    pub line_index: usize,
    pub text: String,
    pub range: TextRange,
    pub tokens: Vec<CanonicalTokenArtifact>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CanonicalBlockArtifact {
    pub page_index: usize,
    pub block_index: usize,
    pub text: String,
    pub range: TextRange,
    pub lines: Vec<CanonicalLineArtifact>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CanonicalPageArtifact {
    pub page_index: usize,
    pub range: Option<TextRange>,
    pub blocks: Vec<CanonicalBlockArtifact>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CanonicalTtsTextArtifact {
    pub text: String,
    pub pages: Vec<CanonicalPageArtifact>,
    pub block_count: usize,
    pub line_count: usize,
    pub token_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClassificationSummary {
    pub coverage_ratio: f32,
    pub duplicate_ratio: f32,
    pub boilerplate_ratio: f32,
    pub avg_chars_per_text_page: f32,
    pub avg_segments_per_text_page: f32,
    pub reason: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TtsTextSourceKind {
    Embedded,
    Ocr,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OcrTrustClass {
    OcrHighTrust,
    OcrMixedTrust,
    OcrTextOnly,
}

impl OcrTrustClass {
    pub fn label(&self) -> &'static str {
        match self {
            Self::OcrHighTrust => "ocr_high_trust",
            Self::OcrMixedTrust => "ocr_mixed_trust",
            Self::OcrTextOnly => "ocr_text_only",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OcrTokenArtifact {
    pub page_index: usize,
    pub block_index: usize,
    pub line_index: usize,
    pub token_index: usize,
    pub text: String,
    pub bounds: PdfRectData,
    pub confidence: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OcrLineArtifact {
    pub page_index: usize,
    pub block_index: usize,
    pub line_index: usize,
    pub text: String,
    pub bounds: PdfRectData,
    pub confidence: f32,
    pub tokens: Vec<OcrTokenArtifact>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OcrBlockArtifact {
    pub page_index: usize,
    pub block_index: usize,
    pub text: String,
    pub bounds: PdfRectData,
    pub confidence: f32,
    pub lines: Vec<OcrLineArtifact>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OcrPageArtifact {
    pub page_index: usize,
    pub confidence: f32,
    pub blocks: Vec<OcrBlockArtifact>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OcrArtifacts {
    pub source_path: PathBuf,
    pub source_fingerprint: String,
    pub generated_at_unix_secs: u64,
    pub confidence: f32,
    pub trust_class: OcrTrustClass,
    pub pages: Vec<OcrPageArtifact>,
    pub artifact_path: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SentencePlan {
    pub id: u64,
    pub text: String,
    pub range: TextRange,
    pub page_range: PageRange,
    pub unit_kind: SentenceUnitKind,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum SentenceUnitKind {
    Sentence,
    BlockFallback,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageTtsArtifact {
    pub page_index: usize,
    pub original_char_count: usize,
    pub normalized_char_count: usize,
    pub segment_count: usize,
    pub duplicate_lines_removed: usize,
    pub repeated_edge_lines_removed: usize,
    pub empty_after_normalization: bool,
    pub range: Option<TextRange>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NormalizationStats {
    pub pages_with_text: usize,
    pub empty_pages: usize,
    pub original_chars: usize,
    pub normalized_chars: usize,
    pub ligatures_replaced: usize,
    pub soft_hyphens_removed: usize,
    pub zero_width_chars_removed: usize,
    pub duplicate_lines_removed: usize,
    pub repeated_edge_lines_removed: usize,
    pub joined_hyphenations: usize,
    pub collapsed_whitespace_runs: usize,
    pub rotated_segments_suppressed: usize,
    pub duplicate_segments_suppressed: usize,
    pub column_reorders: usize,
    pub extracted_blocks: usize,
    pub extracted_lines: usize,
    pub table_like_blocks: usize,
    pub caption_like_blocks: usize,
    pub footnote_like_blocks: usize,
    pub sidenote_like_blocks: usize,
    pub block_fallback_units: usize,
    pub sentence_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TtsAnalysisArtifacts {
    pub source_path: PathBuf,
    pub source_fingerprint: String,
    pub generated_at_unix_secs: u64,
    pub text_source: TtsTextSourceKind,
    pub ocr_trust: Option<OcrTrustClass>,
    pub ocr_confidence: Option<f32>,
    pub ocr_artifact_path: Option<PathBuf>,
    pub mode: PdfTtsMode,
    pub confidence: f32,
    pub classification: ClassificationSummary,
    pub tts_text: String,
    pub canonical_text: CanonicalTtsTextArtifact,
    pub sentences: Vec<SentencePlan>,
    pub pages: Vec<PageTtsArtifact>,
    pub stats: NormalizationStats,
    pub analysis_scope: AnalysisScope,
    pub artifact_path: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AnalysisScope {
    pub start_page: usize,
    pub end_page: usize,
    pub full_document: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SentenceSyncConfidence {
    ExactSentence,
    FuzzySentence,
    BlockFallback,
    PageFallback,
    Missing,
}

impl SentenceSyncConfidence {
    pub fn label(&self) -> &'static str {
        match self {
            Self::ExactSentence => "exact_sentence",
            Self::FuzzySentence => "fuzzy_sentence",
            Self::BlockFallback => "block_fallback",
            Self::PageFallback => "page_fallback",
            Self::Missing => "missing",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SentenceSyncTarget {
    pub sentence_index: usize,
    pub sentence_id: u64,
    pub confidence: SentenceSyncConfidence,
    pub page_index: Option<usize>,
    pub rects: Vec<PdfRectData>,
    pub fallback_reason: String,
    pub score: f32,
    pub score_breakdown: SyncScoreBreakdown,
    pub lineage: Vec<SyncTokenLineage>,
    pub artifact_path: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SyncScoreBreakdown {
    pub text_similarity: f32,
    pub reading_order: f32,
    pub geometry_compactness: f32,
    pub page_continuity: f32,
    pub total: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncTokenLineage {
    pub token: String,
    pub page_index: usize,
    pub block_index: usize,
    pub line_index: usize,
    pub token_index: usize,
    pub rect: PdfRectData,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TtsRuntimePolicy {
    pub allow_playback: bool,
    pub allow_rect_highlights: bool,
    pub allow_sync_prefetch: bool,
    pub max_sync_confidence: SentenceSyncConfidence,
    pub reason: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TtsEngineKind {
    DryRun,
    TonePreview,
}

impl TtsEngineKind {
    pub fn label(&self) -> &'static str {
        match self {
            Self::DryRun => "dry_run",
            Self::TonePreview => "tone_preview",
        }
    }

    pub fn from_name(name: &str) -> Self {
        match name {
            "tone_preview" => Self::TonePreview,
            _ => Self::DryRun,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TtsPlaybackState {
    Stopped,
    Preparing,
    Playing,
    Paused,
    Failed,
}

impl TtsPlaybackState {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Stopped => "stopped",
            Self::Preparing => "preparing",
            Self::Playing => "playing",
            Self::Paused => "paused",
            Self::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreparedSentenceClip {
    pub source_fingerprint: String,
    pub sentence_id: u64,
    pub sentence_index: usize,
    pub engine: TtsEngineKind,
    pub text: String,
    pub manifest_path: PathBuf,
    pub audio_path: Option<PathBuf>,
    pub estimated_duration_ms: u64,
    pub word_count: usize,
    pub rate: f32,
    pub generated_at_unix_secs: u64,
    #[serde(default)]
    pub cache_hit: bool,
    pub cache_key: PreparedClipCacheKey,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PreparedClipCacheKey {
    pub source_fingerprint: String,
    pub sentence_id: u64,
    pub engine: TtsEngineKind,
    pub language: String,
    pub voice: String,
    pub rate_milli: u32,
    pub sentence_pause_ms: u64,
    pub text_hash: u64,
}

impl PreparedClipCacheKey {
    pub fn stem(&self) -> String {
        format!(
            "{}-{}-{}-{}-{}-{}-{}",
            self.source_fingerprint,
            self.sentence_id,
            self.engine.label(),
            sanitize_cache_component(&self.language),
            sanitize_cache_component(&self.voice),
            self.rate_milli,
            self.text_hash,
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TtsSynthesisSettings {
    pub language: String,
    pub voice: String,
    pub rate: f32,
    pub volume: f32,
    pub sentence_pause_ms: u64,
}

impl TtsSynthesisSettings {
    pub fn from_config(config: &AppConfig) -> Self {
        Self {
            language: config.tts.language.clone(),
            voice: config.tts.voice.clone(),
            rate: config.tts.rate,
            volume: config.tts.volume,
            sentence_pause_ms: config.tts.sentence_pause_ms,
        }
    }
}

#[derive(Debug, Clone)]
pub struct TtsPrefetchPlan {
    pub sentence_indexes: Vec<usize>,
    pub estimated_duration_ms_total: u64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TtsRuntimeCommand {
    Play,
    Pause,
    Resume,
    Stop,
    SeekToSentence,
    NextSentence,
    PreviousSentence,
}

impl TtsRuntimeCommand {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Play => "play",
            Self::Pause => "pause",
            Self::Resume => "resume",
            Self::Stop => "stop",
            Self::SeekToSentence => "seek_to_sentence",
            Self::NextSentence => "next_sentence",
            Self::PreviousSentence => "previous_sentence",
        }
    }
}

pub trait TtsEngine {
    fn kind(&self) -> TtsEngineKind;
    fn build_cache_key(
        &self,
        analysis: &TtsAnalysisArtifacts,
        sentence: &SentencePlan,
        settings: &TtsSynthesisSettings,
    ) -> PreparedClipCacheKey;
    fn synthesize_clip(
        &self,
        audio_path: &Path,
        sentence: &SentencePlan,
        settings: &TtsSynthesisSettings,
    ) -> Result<Option<PathBuf>>;
}

struct DryRunEngine;
struct TonePreviewEngine;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TtsPrefetchSnapshot {
    pub active_sentence_index: usize,
    pub prepared_sentence_indexes: Vec<usize>,
    pub queued_sentence_indexes: Vec<usize>,
    pub failed_sentence_indexes: Vec<usize>,
    pub request_id: u64,
    pub engine: TtsEngineKind,
}

pub fn prefetch_snapshot(
    active_sentence_index: usize,
    prepared_sentence_indexes: Vec<usize>,
    queued_sentence_indexes: Vec<usize>,
    failed_sentence_indexes: Vec<usize>,
    request_id: u64,
    engine: TtsEngineKind,
) -> TtsPrefetchSnapshot {
    TtsPrefetchSnapshot {
        active_sentence_index,
        prepared_sentence_indexes,
        queued_sentence_indexes,
        failed_sentence_indexes,
        request_id,
        engine,
    }
}

#[derive(Debug, Clone)]
pub enum TtsWorkerMessage {
    Completed {
        request_id: u64,
        artifacts: TtsAnalysisArtifacts,
    },
    Failed {
        request_id: u64,
        error: String,
    },
    PrefetchCompleted {
        request_id: u64,
        cancel_token: u64,
        clip: PreparedSentenceClip,
        elapsed_ms: u64,
    },
    PrefetchFailed {
        request_id: u64,
        cancel_token: u64,
        sentence_index: usize,
        error: String,
        elapsed_ms: u64,
    },
    SyncCompleted {
        request_id: u64,
        cancel_token: u64,
        target: SentenceSyncTarget,
        elapsed_ms: u64,
    },
    SyncFailed {
        request_id: u64,
        cancel_token: u64,
        sentence_index: usize,
        error: String,
        elapsed_ms: u64,
    },
}

#[instrument(skip(config))]
pub fn analyze_pdf_for_tts(config: &AppConfig, source_path: &Path) -> Result<TtsAnalysisArtifacts> {
    let runtime =
        PdfRuntime::new(config).context("failed to initialize Pdfium for TTS analysis")?;
    let document = runtime
        .open_document(source_path)
        .with_context(|| format!("failed to open {} for TTS analysis", source_path.display()))?;
    build_artifacts_from_document(config, source_path, &document)
}

#[instrument(skip(config))]
pub fn analyze_pdf_for_tts_in_scope(
    config: &AppConfig,
    source_path: &Path,
    start_page: usize,
    end_page: usize,
) -> Result<TtsAnalysisArtifacts> {
    let runtime =
        PdfRuntime::new(config).context("failed to initialize Pdfium for TTS analysis")?;
    let document = runtime
        .open_document(source_path)
        .with_context(|| format!("failed to open {} for TTS analysis", source_path.display()))?;
    build_artifacts_from_document_with_scope(config, source_path, &document, start_page, end_page)
}

pub fn compute_sentence_sync(
    config: &AppConfig,
    analysis: &TtsAnalysisArtifacts,
    sentence_index: usize,
) -> Result<SentenceSyncTarget> {
    let sentence = analysis
        .sentences
        .get(sentence_index)
        .with_context(|| format!("sentence index {sentence_index} is out of bounds"))?;
    let sync_path = config.tts_sync_artifact_path(&analysis.source_path, sentence.id)?;
    if sync_path.exists() {
        let contents = fs::read_to_string(&sync_path)
            .with_context(|| format!("failed to read {}", sync_path.display()))?;
        let mut cached = toml::from_str::<SentenceSyncTarget>(&contents)
            .with_context(|| format!("failed to parse {}", sync_path.display()))?;
        cached.artifact_path = Some(sync_path);
        debug!(
            sentence_index,
            sentence_id = sentence.id,
            confidence = %cached.confidence.label(),
            score = cached.score,
            text_similarity = cached.score_breakdown.text_similarity,
            reading_order = cached.score_breakdown.reading_order,
            geometry_compactness = cached.score_breakdown.geometry_compactness,
            page_continuity = cached.score_breakdown.page_continuity,
            cache_hit = true,
            "loaded cached PDF sentence sync target"
        );
        return Ok(cached);
    }
    let runtime =
        PdfRuntime::new(config).context("failed to initialize Pdfium for TTS sync analysis")?;
    let document = runtime
        .open_document(&analysis.source_path)
        .with_context(|| {
            format!(
                "failed to open {} for TTS sync",
                analysis.source_path.display()
            )
        })?;
    let mut target = compute_sentence_sync_from_document(&document, analysis, sentence_index)?;
    let artifact_path = persist_sync_target(config, analysis, &target)?;
    target.artifact_path = Some(artifact_path);
    debug!(
        sentence_index,
        sentence_id = sentence.id,
        confidence = %target.confidence.label(),
        score = target.score,
        text_similarity = target.score_breakdown.text_similarity,
        reading_order = target.score_breakdown.reading_order,
        geometry_compactness = target.score_breakdown.geometry_compactness,
        page_continuity = target.score_breakdown.page_continuity,
        cache_hit = false,
        "computed PDF sentence sync target"
    );
    Ok(target)
}

pub fn persist_artifacts(config: &AppConfig, artifacts: &TtsAnalysisArtifacts) -> Result<PathBuf> {
    let path = config.tts_artifact_path(&artifacts.source_path)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }

    let contents =
        toml::to_string_pretty(artifacts).context("failed to serialize TTS artifacts")?;
    fs::write(&path, contents)
        .with_context(|| format!("failed to write TTS artifacts to {}", path.display()))?;
    Ok(path)
}

pub fn load_ocr_artifacts(config: &AppConfig, source_path: &Path) -> Result<Option<OcrArtifacts>> {
    let path = config.tts_ocr_artifact_path(source_path)?;
    if !path.exists() {
        return Ok(None);
    }

    let contents = fs::read_to_string(&path)
        .with_context(|| format!("failed to read OCR artifacts from {}", path.display()))?;
    let mut artifacts = toml::from_str::<OcrArtifacts>(&contents)
        .with_context(|| format!("failed to parse OCR artifacts from {}", path.display()))?;
    artifacts.artifact_path = Some(path);
    Ok(Some(artifacts))
}

pub fn persist_sync_target(
    config: &AppConfig,
    analysis: &TtsAnalysisArtifacts,
    target: &SentenceSyncTarget,
) -> Result<PathBuf> {
    let path = config.tts_sync_artifact_path(&analysis.source_path, target.sentence_id)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    fs::write(&path, toml::to_string_pretty(target)?)
        .with_context(|| format!("failed to write sync artifact {}", path.display()))?;
    Ok(path)
}

pub fn sentence_index_for_id(analysis: &TtsAnalysisArtifacts, sentence_id: u64) -> Option<usize> {
    analysis
        .sentences
        .iter()
        .position(|sentence| sentence.id == sentence_id)
}

pub fn build_prefetch_plan(
    analysis: &TtsAnalysisArtifacts,
    start_sentence_index: usize,
    sentence_count: usize,
) -> Vec<usize> {
    let end = (start_sentence_index + sentence_count).min(analysis.sentences.len());
    (start_sentence_index..end).collect()
}

pub fn build_prefetch_plan_with_budget(
    analysis: &TtsAnalysisArtifacts,
    start_sentence_index: usize,
    sentence_count: usize,
    duration_budget_ms: u64,
    settings: &TtsSynthesisSettings,
) -> TtsPrefetchPlan {
    let mut sentence_indexes = Vec::new();
    let mut estimated_duration_ms_total = 0u64;

    for sentence_index in build_prefetch_plan(analysis, start_sentence_index, sentence_count) {
        let Some(sentence) = analysis.sentences.get(sentence_index) else {
            continue;
        };
        let estimated = estimate_sentence_duration_ms(&sentence.text, settings);
        if !sentence_indexes.is_empty()
            && estimated_duration_ms_total.saturating_add(estimated) > duration_budget_ms
        {
            break;
        }
        sentence_indexes.push(sentence_index);
        estimated_duration_ms_total = estimated_duration_ms_total.saturating_add(estimated);
    }

    TtsPrefetchPlan {
        sentence_indexes,
        estimated_duration_ms_total,
    }
}

pub fn build_sync_prefetch_plan(
    analysis: &TtsAnalysisArtifacts,
    start_sentence_index: usize,
    sentence_count: usize,
) -> Vec<usize> {
    build_prefetch_plan(analysis, start_sentence_index, sentence_count)
}

pub fn sentence_budget_window(
    active_sentence_index: usize,
    sentence_count: usize,
    radius: usize,
) -> HashSet<usize> {
    if sentence_count == 0 {
        return HashSet::new();
    }

    let start = active_sentence_index.saturating_sub(radius);
    let end = (active_sentence_index + radius + 1).min(sentence_count);
    (start..end).collect()
}

#[instrument(skip(config, analysis))]
pub fn evaluate_runtime_policy(
    config: &AppConfig,
    analysis: &TtsAnalysisArtifacts,
) -> TtsRuntimePolicy {
    if analysis.text_source == TtsTextSourceKind::Ocr {
        let trust = analysis.ocr_trust.unwrap_or(OcrTrustClass::OcrTextOnly);
        let policy = match trust {
            OcrTrustClass::OcrHighTrust => TtsRuntimePolicy {
                allow_playback: !analysis.sentences.is_empty(),
                allow_rect_highlights: true,
                allow_sync_prefetch: true,
                max_sync_confidence: SentenceSyncConfidence::BlockFallback,
                reason: "ocr_high_trust_block_precision_cap".into(),
            },
            OcrTrustClass::OcrMixedTrust => TtsRuntimePolicy {
                allow_playback: !analysis.sentences.is_empty(),
                allow_rect_highlights: true,
                allow_sync_prefetch: true,
                max_sync_confidence: SentenceSyncConfidence::BlockFallback,
                reason: "ocr_mixed_trust_block_precision_cap".into(),
            },
            OcrTrustClass::OcrTextOnly => TtsRuntimePolicy {
                allow_playback: !analysis.sentences.is_empty(),
                allow_rect_highlights: false,
                allow_sync_prefetch: false,
                max_sync_confidence: SentenceSyncConfidence::PageFallback,
                reason: "ocr_text_only_page_follow".into(),
            },
        };

        info!(
            ocr_trust = %trust.label(),
            allow_playback = policy.allow_playback,
            allow_rect_highlights = policy.allow_rect_highlights,
            allow_sync_prefetch = policy.allow_sync_prefetch,
            max_sync_confidence = %policy.max_sync_confidence.label(),
            reason = %policy.reason,
            "evaluated OCR-backed PDF TTS runtime policy"
        );
        return policy;
    }

    let policy = match analysis.mode {
        PdfTtsMode::HighTextTrust | PdfTtsMode::MixedTextTrust => TtsRuntimePolicy {
            allow_playback: !analysis.sentences.is_empty(),
            allow_rect_highlights: true,
            allow_sync_prefetch: true,
            max_sync_confidence: SentenceSyncConfidence::ExactSentence,
            reason: "embedded_text_sync_allowed".into(),
        },
        PdfTtsMode::OcrRequired => match config.tts.ocr_policy {
            TtsOcrPolicy::Disabled => TtsRuntimePolicy {
                allow_playback: false,
                allow_rect_highlights: false,
                allow_sync_prefetch: false,
                max_sync_confidence: SentenceSyncConfidence::Missing,
                reason: "ocr_required_but_ocr_policy_disabled".into(),
            },
            TtsOcrPolicy::Deferred => TtsRuntimePolicy {
                allow_playback: !analysis.sentences.is_empty(),
                allow_rect_highlights: false,
                allow_sync_prefetch: false,
                max_sync_confidence: SentenceSyncConfidence::PageFallback,
                reason: format!(
                    "ocr_required_with_deferred_policy_min_confidence_{:.2}",
                    config.tts.ocr_min_confidence
                ),
            },
            TtsOcrPolicy::RequireArtifacts => TtsRuntimePolicy {
                allow_playback: false,
                allow_rect_highlights: false,
                allow_sync_prefetch: false,
                max_sync_confidence: SentenceSyncConfidence::Missing,
                reason: "ocr_artifacts_not_integrated_yet".into(),
            },
        },
        PdfTtsMode::RenderOnlyNoSync => TtsRuntimePolicy {
            allow_playback: !analysis.sentences.is_empty(),
            allow_rect_highlights: false,
            allow_sync_prefetch: false,
            max_sync_confidence: SentenceSyncConfidence::PageFallback,
            reason: "render_only_page_level_follow".into(),
        },
    };

    info!(
        mode = %analysis.mode.label(),
        ocr_policy = %config.tts.ocr_policy.as_str(),
        allow_playback = policy.allow_playback,
        allow_rect_highlights = policy.allow_rect_highlights,
        allow_sync_prefetch = policy.allow_sync_prefetch,
        max_sync_confidence = %policy.max_sync_confidence.label(),
        reason = %policy.reason,
        "evaluated PDF TTS runtime policy"
    );

    policy
}

pub fn apply_runtime_policy(
    target: &SentenceSyncTarget,
    policy: &TtsRuntimePolicy,
) -> SentenceSyncTarget {
    if !policy.allow_rect_highlights {
        return SentenceSyncTarget {
            sentence_index: target.sentence_index,
            sentence_id: target.sentence_id,
            confidence: if policy.max_sync_confidence == SentenceSyncConfidence::Missing {
                SentenceSyncConfidence::Missing
            } else if target.page_index.is_some() {
                SentenceSyncConfidence::PageFallback
            } else {
                SentenceSyncConfidence::Missing
            },
            page_index: target.page_index,
            rects: Vec::new(),
            fallback_reason: policy.reason.clone(),
            score: target.score,
            score_breakdown: target.score_breakdown.clone(),
            lineage: Vec::new(),
            artifact_path: target.artifact_path.clone(),
        };
    }

    if sync_confidence_rank(target.confidence) <= sync_confidence_rank(policy.max_sync_confidence) {
        return target.clone();
    }

    SentenceSyncTarget {
        sentence_index: target.sentence_index,
        sentence_id: target.sentence_id,
        confidence: policy.max_sync_confidence,
        page_index: target.page_index,
        rects: if policy.max_sync_confidence == SentenceSyncConfidence::PageFallback {
            Vec::new()
        } else {
            target.rects.clone()
        },
        fallback_reason: format!("{}; {}", target.fallback_reason, policy.reason),
        score: target.score,
        score_breakdown: target.score_breakdown.clone(),
        lineage: if policy.max_sync_confidence == SentenceSyncConfidence::PageFallback {
            Vec::new()
        } else {
            target.lineage.clone()
        },
        artifact_path: target.artifact_path.clone(),
    }
}

pub fn prepare_sentence_clip(
    config: &AppConfig,
    analysis: &TtsAnalysisArtifacts,
    sentence_index: usize,
    engine: TtsEngineKind,
) -> Result<PreparedSentenceClip> {
    let backend = create_tts_engine(engine);
    let settings = TtsSynthesisSettings::from_config(config);
    let sentence = analysis
        .sentences
        .get(sentence_index)
        .with_context(|| format!("sentence index {sentence_index} is out of bounds"))?;
    let cache_key = backend.build_cache_key(analysis, sentence, &settings);
    let stem = cache_key.stem();
    let manifest_path = config.tts_audio_cache_dir()?.join(format!("{stem}.toml"));
    let audio_path = match backend.kind() {
        TtsEngineKind::DryRun => None,
        TtsEngineKind::TonePreview => {
            Some(config.tts_audio_cache_dir()?.join(format!("{stem}.wav")))
        }
    };

    if manifest_path.exists() {
        let contents = fs::read_to_string(&manifest_path)
            .with_context(|| format!("failed to read {}", manifest_path.display()))?;
        let mut cached = toml::from_str::<PreparedSentenceClip>(&contents)
            .with_context(|| format!("failed to parse {}", manifest_path.display()))?;
        let audio_ok = cached.audio_path.as_ref().is_none_or(|path| path.exists());
        if cached.cache_key == cache_key && audio_ok {
            cached.cache_hit = true;
            return Ok(cached);
        }
    }

    if let Some(parent) = manifest_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }

    let estimated_duration_ms = estimate_sentence_duration_ms(&sentence.text, &settings);
    if let Some(audio_path) = &audio_path {
        backend.synthesize_clip(audio_path, sentence, &settings)?;
    }

    let clip = PreparedSentenceClip {
        source_fingerprint: analysis.source_fingerprint.clone(),
        sentence_id: sentence.id,
        sentence_index,
        engine,
        text: sentence.text.clone(),
        manifest_path: manifest_path.clone(),
        audio_path,
        estimated_duration_ms,
        word_count: sentence.text.split_whitespace().count(),
        rate: settings.rate,
        generated_at_unix_secs: unix_timestamp_secs(),
        cache_hit: false,
        cache_key,
    };

    fs::write(&manifest_path, toml::to_string_pretty(&clip)?)
        .with_context(|| format!("failed to write {}", manifest_path.display()))?;

    Ok(clip)
}

pub fn estimate_sentence_duration_ms(text: &str, settings: &TtsSynthesisSettings) -> u64 {
    let word_count = text.split_whitespace().count().max(1) as u64;
    let punctuation_pause = text
        .chars()
        .filter(|ch| matches!(ch, '.' | ',' | ';' | ':' | '?' | '!'))
        .count() as u64
        * 120
        + settings.sentence_pause_ms;
    let base = (word_count * 360 + punctuation_pause).clamp(900, 12_000) as f32;
    (base / settings.rate.max(0.25)).round() as u64
}

pub fn create_tts_engine(kind: TtsEngineKind) -> Box<dyn TtsEngine + Send + Sync> {
    match kind {
        TtsEngineKind::DryRun => Box::new(DryRunEngine),
        TtsEngineKind::TonePreview => Box::new(TonePreviewEngine),
    }
}

impl TtsEngine for DryRunEngine {
    fn kind(&self) -> TtsEngineKind {
        TtsEngineKind::DryRun
    }

    fn build_cache_key(
        &self,
        analysis: &TtsAnalysisArtifacts,
        sentence: &SentencePlan,
        settings: &TtsSynthesisSettings,
    ) -> PreparedClipCacheKey {
        PreparedClipCacheKey {
            source_fingerprint: analysis.source_fingerprint.clone(),
            sentence_id: sentence.id,
            engine: self.kind(),
            language: settings.language.clone(),
            voice: settings.voice.clone(),
            rate_milli: (settings.rate * 1000.0).round() as u32,
            sentence_pause_ms: settings.sentence_pause_ms,
            text_hash: stable_text_hash(&sentence.text),
        }
    }

    fn synthesize_clip(
        &self,
        _audio_path: &Path,
        _sentence: &SentencePlan,
        _settings: &TtsSynthesisSettings,
    ) -> Result<Option<PathBuf>> {
        Ok(None)
    }
}

impl TtsEngine for TonePreviewEngine {
    fn kind(&self) -> TtsEngineKind {
        TtsEngineKind::TonePreview
    }

    fn build_cache_key(
        &self,
        analysis: &TtsAnalysisArtifacts,
        sentence: &SentencePlan,
        settings: &TtsSynthesisSettings,
    ) -> PreparedClipCacheKey {
        PreparedClipCacheKey {
            source_fingerprint: analysis.source_fingerprint.clone(),
            sentence_id: sentence.id,
            engine: self.kind(),
            language: settings.language.clone(),
            voice: settings.voice.clone(),
            rate_milli: (settings.rate * 1000.0).round() as u32,
            sentence_pause_ms: settings.sentence_pause_ms,
            text_hash: stable_text_hash(&sentence.text),
        }
    }

    fn synthesize_clip(
        &self,
        audio_path: &Path,
        sentence: &SentencePlan,
        settings: &TtsSynthesisSettings,
    ) -> Result<Option<PathBuf>> {
        let estimated_duration_ms = estimate_sentence_duration_ms(&sentence.text, settings);
        generate_tone_preview_clip(audio_path, sentence.id as usize, estimated_duration_ms)?;
        Ok(Some(audio_path.to_path_buf()))
    }
}

#[derive(Debug, Clone)]
struct ClassificationResult {
    mode: PdfTtsMode,
    confidence: f32,
    summary: ClassificationSummary,
}

pub fn compute_sentence_sync_from_document(
    document: &PdfDocument<'_>,
    analysis: &TtsAnalysisArtifacts,
    sentence_index: usize,
) -> Result<SentenceSyncTarget> {
    let sentence = analysis
        .sentences
        .get(sentence_index)
        .with_context(|| format!("sentence index {sentence_index} is out of bounds"))?;
    let normalized_sentence = normalize_text_for_sync(&sentence.text);
    let sentence_tokens = build_sync_tokens(&normalized_sentence);
    let mut best: Option<SentenceSyncTarget> = None;

    for page_index in sentence.page_range.start_page..=sentence.page_range.end_page {
        let segments = document.text_segments_for_page(page_index)?;
        let candidate = sync_target_for_page(
            page_index,
            sentence_index,
            sentence.id,
            &sentence_tokens,
            &normalized_sentence,
            &segments,
        );
        if best
            .as_ref()
            .is_none_or(|current| candidate.score > current.score)
        {
            best = Some(candidate);
        }
    }

    Ok(best.unwrap_or(SentenceSyncTarget {
        sentence_index,
        sentence_id: sentence.id,
        confidence: SentenceSyncConfidence::Missing,
        page_index: None,
        rects: Vec::new(),
        fallback_reason: "no_page_candidate".into(),
        score: 0.0,
        score_breakdown: SyncScoreBreakdown::default(),
        lineage: Vec::new(),
        artifact_path: None,
    }))
}

fn sync_target_for_page(
    page_index: usize,
    sentence_index: usize,
    sentence_id: u64,
    sentence_tokens: &[String],
    normalized_sentence: &str,
    segments: &[crate::pdf::TextSegmentData],
) -> SentenceSyncTarget {
    let normalized_segments = segments
        .iter()
        .map(|segment| normalize_text_for_sync(&segment.text))
        .collect::<Vec<_>>();

    let mut page_text = String::new();
    let mut ranges = Vec::with_capacity(normalized_segments.len());
    for segment_text in &normalized_segments {
        if !page_text.is_empty() && !segment_text.is_empty() {
            page_text.push(' ');
        }
        let start = page_text.len();
        page_text.push_str(segment_text);
        let end = page_text.len();
        ranges.push((start, end));
    }

    if !normalized_sentence.is_empty() {
        if let Some(offset) = page_text.find(normalized_sentence) {
            let end = offset + normalized_sentence.len();
            let rects = collect_overlapping_rects(segments, &ranges, offset, end);
            if !rects.is_empty() {
                let lineage =
                    collect_overlapping_lineage(segments, &ranges, offset, end, page_index);
                return SentenceSyncTarget {
                    sentence_index,
                    sentence_id,
                    confidence: SentenceSyncConfidence::ExactSentence,
                    page_index: Some(page_index),
                    rects,
                    fallback_reason: "exact_substring_match".into(),
                    score: 1.0,
                    score_breakdown: SyncScoreBreakdown {
                        text_similarity: 1.0,
                        reading_order: 1.0,
                        geometry_compactness: geometry_compactness_score(&lineage),
                        page_continuity: 1.0,
                        total: 1.0,
                    },
                    lineage,
                    artifact_path: None,
                };
            }
        }
    }

    let mut best_window: Option<(usize, usize, SyncScoreBreakdown)> = None;
    for start in 0..normalized_segments.len() {
        let mut window_text = String::new();
        for end in start
            ..normalized_segments
                .len()
                .min(start + sentence_tokens.len().max(1) + 8)
        {
            if !window_text.is_empty() {
                window_text.push(' ');
            }
            window_text.push_str(&normalized_segments[end]);
            let text_similarity = token_coverage_score(sentence_tokens, &window_text);
            let reading_order = reading_order_continuity_score(start, end);
            let lineage = segments[start..=end]
                .iter()
                .enumerate()
                .map(|(offset, segment)| SyncTokenLineage {
                    token: segment.text.clone(),
                    page_index,
                    block_index: 0,
                    line_index: start + offset,
                    token_index: 0,
                    rect: segment.rect,
                })
                .collect::<Vec<_>>();
            let geometry_compactness = geometry_compactness_score(&lineage);
            let page_continuity = 1.0;
            let total = text_similarity * 0.55
                + reading_order * 0.15
                + geometry_compactness * 0.20
                + page_continuity * 0.10;
            let breakdown = SyncScoreBreakdown {
                text_similarity,
                reading_order,
                geometry_compactness,
                page_continuity,
                total,
            };
            if best_window
                .as_ref()
                .is_none_or(|(_, _, current)| breakdown.total > current.total)
            {
                best_window = Some((start, end, breakdown));
            }
        }
    }

    if let Some((start, end, breakdown)) = best_window {
        let lineage = segments[start..=end]
            .iter()
            .enumerate()
            .map(|(offset, segment)| SyncTokenLineage {
                token: segment.text.clone(),
                page_index,
                block_index: 0,
                line_index: start + offset,
                token_index: 0,
                rect: segment.rect,
            })
            .collect::<Vec<_>>();
        let rects = segments[start..=end]
            .iter()
            .map(|segment| segment.rect)
            .collect::<Vec<_>>();
        let implausible = breakdown.geometry_compactness < 0.12 || breakdown.reading_order < 0.25;
        let confidence = if implausible {
            SentenceSyncConfidence::PageFallback
        } else if breakdown.total >= 0.88 {
            SentenceSyncConfidence::FuzzySentence
        } else if breakdown.total >= 0.42 {
            SentenceSyncConfidence::BlockFallback
        } else {
            SentenceSyncConfidence::PageFallback
        };
        return SentenceSyncTarget {
            sentence_index,
            sentence_id,
            confidence,
            page_index: Some(page_index),
            rects: if confidence == SentenceSyncConfidence::PageFallback {
                Vec::new()
            } else {
                rects
            },
            fallback_reason: match confidence {
                SentenceSyncConfidence::FuzzySentence => "high_token_window_match".into(),
                SentenceSyncConfidence::BlockFallback => "block_window_fallback".into(),
                SentenceSyncConfidence::PageFallback => {
                    if implausible {
                        "rejected_visually_implausible_match".into()
                    } else {
                        "page_location_only".into()
                    }
                }
                _ => "unknown".into(),
            },
            score: breakdown.total,
            score_breakdown: breakdown,
            lineage: if confidence == SentenceSyncConfidence::PageFallback {
                Vec::new()
            } else {
                lineage
            },
            artifact_path: None,
        };
    }

    SentenceSyncTarget {
        sentence_index,
        sentence_id,
        confidence: SentenceSyncConfidence::PageFallback,
        page_index: Some(page_index),
        rects: Vec::new(),
        fallback_reason: "page_location_only".into(),
        score: 0.1,
        score_breakdown: SyncScoreBreakdown {
            text_similarity: 0.1,
            reading_order: 0.0,
            geometry_compactness: 0.0,
            page_continuity: 1.0,
            total: 0.1,
        },
        lineage: Vec::new(),
        artifact_path: None,
    }
}

fn collect_overlapping_rects(
    segments: &[crate::pdf::TextSegmentData],
    ranges: &[(usize, usize)],
    start: usize,
    end: usize,
) -> Vec<PdfRectData> {
    ranges
        .iter()
        .enumerate()
        .filter_map(|(index, (segment_start, segment_end))| {
            if *segment_end <= start || *segment_start >= end {
                None
            } else {
                segments.get(index).map(|segment| segment.rect)
            }
        })
        .collect()
}

fn collect_overlapping_lineage(
    segments: &[crate::pdf::TextSegmentData],
    ranges: &[(usize, usize)],
    start: usize,
    end: usize,
    page_index: usize,
) -> Vec<SyncTokenLineage> {
    ranges
        .iter()
        .enumerate()
        .filter_map(|(index, (segment_start, segment_end))| {
            if *segment_end <= start || *segment_start >= end {
                None
            } else {
                segments.get(index).map(|segment| SyncTokenLineage {
                    token: segment.text.clone(),
                    page_index,
                    block_index: 0,
                    line_index: index,
                    token_index: 0,
                    rect: segment.rect,
                })
            }
        })
        .collect()
}

fn token_coverage_score(sentence_tokens: &[String], text: &str) -> f32 {
    if sentence_tokens.is_empty() {
        return 0.0;
    }
    let normalized_text = normalize_text_for_sync(text);
    let hits = sentence_tokens
        .iter()
        .filter(|token| normalized_text.contains(token.as_str()))
        .count();
    hits as f32 / sentence_tokens.len() as f32
}

fn reading_order_continuity_score(start: usize, end: usize) -> f32 {
    let span = end.saturating_sub(start) + 1;
    (1.0 / span.max(1) as f32).clamp(0.0, 1.0).max(0.2)
}

fn geometry_compactness_score(lineage: &[SyncTokenLineage]) -> f32 {
    if lineage.is_empty() {
        return 0.0;
    }
    let left = lineage
        .iter()
        .map(|item| item.rect.left)
        .fold(f32::MAX, f32::min);
    let right = lineage
        .iter()
        .map(|item| item.rect.right)
        .fold(f32::MIN, f32::max);
    let top = lineage
        .iter()
        .map(|item| item.rect.top)
        .fold(f32::MIN, f32::max);
    let bottom = lineage
        .iter()
        .map(|item| item.rect.bottom)
        .fold(f32::MAX, f32::min);
    let total_area = lineage
        .iter()
        .map(|item| {
            (item.rect.right - item.rect.left).abs() * (item.rect.top - item.rect.bottom).abs()
        })
        .sum::<f32>();
    let bbox_area = (right - left).abs() * (top - bottom).abs();
    if bbox_area <= 0.0 {
        return 0.0;
    }
    (total_area / bbox_area).clamp(0.0, 1.0)
}

fn sync_confidence_rank(confidence: SentenceSyncConfidence) -> u8 {
    match confidence {
        SentenceSyncConfidence::ExactSentence => 0,
        SentenceSyncConfidence::FuzzySentence => 1,
        SentenceSyncConfidence::BlockFallback => 2,
        SentenceSyncConfidence::PageFallback => 3,
        SentenceSyncConfidence::Missing => 4,
    }
}

fn build_sync_tokens(text: &str) -> Vec<String> {
    normalize_text_for_sync(text)
        .split(' ')
        .filter(|token| !token.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn normalize_text_for_sync(value: &str) -> String {
    let mut normalized = value.to_string();
    for (ligature, replacement) in PDF_LIGATURES {
        normalized = normalized.replace(ligature, replacement);
    }
    normalized = normalized
        .replace('\u{00AD}', "")
        .replace(
            ['\u{200B}', '\u{200C}', '\u{200D}', '\u{2060}', '\u{FEFF}'],
            "",
        )
        .replace(['\n', '\r', '\t'], " ");

    let mut collapsed = String::with_capacity(normalized.len());
    let mut last_was_space = false;
    for ch in normalized.chars() {
        let lower = ch.to_ascii_lowercase();
        if lower.is_alphanumeric() {
            collapsed.push(lower);
            last_was_space = false;
        } else if !last_was_space {
            collapsed.push(' ');
            last_was_space = true;
        }
    }

    collapsed.trim().to_string()
}

fn generate_tone_preview_clip(path: &Path, sentence_index: usize, duration_ms: u64) -> Result<()> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 22_050,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec)
        .with_context(|| format!("failed to create {}", path.display()))?;
    let sample_count = (spec.sample_rate as u64 * duration_ms / 1_000) as usize;
    let frequency_hz = 330.0 + (sentence_index % 6) as f32 * 55.0;
    let amplitude = 0.2;
    let fade_samples = (spec.sample_rate / 40) as usize;

    for index in 0..sample_count {
        let t = index as f32 / spec.sample_rate as f32;
        let envelope = if index < fade_samples {
            index as f32 / fade_samples.max(1) as f32
        } else if index + fade_samples >= sample_count {
            (sample_count.saturating_sub(index)) as f32 / fade_samples.max(1) as f32
        } else {
            1.0
        };
        let sample = (2.0 * std::f32::consts::PI * frequency_hz * t).sin() * amplitude * envelope;
        writer.write_sample((sample * i16::MAX as f32) as i16)?;
    }

    writer.finalize()?;
    Ok(())
}

fn sanitize_cache_component(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

fn stable_text_hash(text: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

#[instrument(skip(config, document))]
pub fn build_artifacts_from_document(
    config: &AppConfig,
    source_path: &Path,
    document: &PdfDocument<'_>,
) -> Result<TtsAnalysisArtifacts> {
    build_artifacts_from_document_with_scope(
        config,
        source_path,
        document,
        0,
        document.metadata.page_count.saturating_sub(1),
    )
}

#[instrument(skip(config, document))]
pub fn build_artifacts_from_document_with_scope(
    config: &AppConfig,
    source_path: &Path,
    document: &PdfDocument<'_>,
    start_page: usize,
    end_page: usize,
) -> Result<TtsAnalysisArtifacts> {
    let fingerprint = fingerprint_document_path(source_path)?;
    let repeated_edge_lines = collect_repeated_edge_lines(config, document)?;
    let mut stats = NormalizationStats::default();
    let mut full_text = String::new();
    let page_count = document.metadata.page_count;
    let start_page = start_page.min(page_count.saturating_sub(1));
    let end_page = end_page.min(page_count.saturating_sub(1)).max(start_page);
    let scoped_page_count = end_page - start_page + 1;
    let mut pages = Vec::with_capacity(scoped_page_count);
    let mut canonical_pages = Vec::with_capacity(scoped_page_count);
    let mut block_count = 0usize;
    let mut line_count = 0usize;
    let mut token_count = 0usize;

    for page_index in start_page..=end_page {
        let segments = document.text_segments_for_page(page_index)?;
        let segment_count = segments.len();
        let extracted = extract_page_text_for_tts(&segments, config);
        let normalized = normalize_page_text(&extracted.text, &repeated_edge_lines, config);

        stats.original_chars += extracted.text.chars().count();
        stats.normalized_chars += normalized.text.chars().count();
        stats.ligatures_replaced += normalized.stats.ligatures_replaced;
        stats.soft_hyphens_removed += normalized.stats.soft_hyphens_removed;
        stats.zero_width_chars_removed += normalized.stats.zero_width_chars_removed;
        stats.duplicate_lines_removed += normalized.stats.duplicate_lines_removed;
        stats.repeated_edge_lines_removed += normalized.stats.repeated_edge_lines_removed;
        stats.joined_hyphenations += normalized.stats.joined_hyphenations;
        stats.collapsed_whitespace_runs += normalized.stats.collapsed_whitespace_runs;
        stats.rotated_segments_suppressed += extracted.stats.rotated_segments_suppressed;
        stats.duplicate_segments_suppressed += extracted.stats.duplicate_segments_suppressed;
        stats.column_reorders += extracted.stats.column_reorders;
        stats.extracted_blocks += normalized.blocks.len();
        stats.extracted_lines += normalized
            .blocks
            .iter()
            .map(|block| block.lines.len())
            .sum::<usize>();
        stats.table_like_blocks += normalized.stats.table_like_blocks;
        stats.caption_like_blocks += normalized.stats.caption_like_blocks;
        stats.footnote_like_blocks += normalized.stats.footnote_like_blocks;
        stats.sidenote_like_blocks += normalized.stats.sidenote_like_blocks;

        if normalized.text.trim().is_empty() {
            stats.empty_pages += 1;
            pages.push(PageTtsArtifact {
                page_index,
                original_char_count: extracted.text.chars().count(),
                normalized_char_count: 0,
                segment_count,
                duplicate_lines_removed: normalized.stats.duplicate_lines_removed,
                repeated_edge_lines_removed: normalized.stats.repeated_edge_lines_removed,
                empty_after_normalization: true,
                range: None,
            });
            canonical_pages.push(CanonicalPageArtifact {
                page_index,
                range: None,
                blocks: Vec::new(),
            });
            continue;
        }

        stats.pages_with_text += 1;
        if !full_text.is_empty() {
            full_text.push_str("\n\n");
        }
        let start = full_text.len();
        let page_blocks = append_page_to_canonical_text(
            &mut full_text,
            page_index,
            &normalized,
            &mut line_count,
            &mut token_count,
        );
        let end = full_text.len();
        block_count += page_blocks.len();

        pages.push(PageTtsArtifact {
            page_index,
            original_char_count: extracted.text.chars().count(),
            normalized_char_count: normalized.text.chars().count(),
            segment_count,
            duplicate_lines_removed: normalized.stats.duplicate_lines_removed,
            repeated_edge_lines_removed: normalized.stats.repeated_edge_lines_removed,
            empty_after_normalization: false,
            range: Some(TextRange { start, end }),
        });
        canonical_pages.push(CanonicalPageArtifact {
            page_index,
            range: Some(TextRange { start, end }),
            blocks: page_blocks,
        });
    }

    let canonical_text = CanonicalTtsTextArtifact {
        text: full_text.clone(),
        pages: canonical_pages,
        block_count,
        line_count,
        token_count,
    };

    let sentences = build_sentence_plan(&canonical_text, &fingerprint, config);
    stats.sentence_count = sentences.len();
    stats.block_fallback_units = sentences
        .iter()
        .filter(|sentence| sentence.unit_kind == SentenceUnitKind::BlockFallback)
        .count();

    let classification = classify_pdf_for_tts(scoped_page_count, &pages, &stats, config);

    let mut artifacts = TtsAnalysisArtifacts {
        source_path: source_path.to_path_buf(),
        source_fingerprint: fingerprint,
        generated_at_unix_secs: unix_timestamp_secs(),
        text_source: TtsTextSourceKind::Embedded,
        ocr_trust: None,
        ocr_confidence: None,
        ocr_artifact_path: None,
        mode: classification.mode,
        confidence: classification.confidence,
        classification: classification.summary,
        tts_text: canonical_text.text.clone(),
        canonical_text,
        sentences,
        pages,
        stats,
        analysis_scope: AnalysisScope {
            start_page,
            end_page,
            full_document: start_page == 0 && end_page + 1 == page_count,
        },
        artifact_path: None,
    };

    if artifacts.mode == PdfTtsMode::OcrRequired
        && config.tts.ocr_policy != TtsOcrPolicy::Disabled
        && let Some(ocr_artifacts) = load_ocr_artifacts(config, source_path)?
        && ocr_artifacts.confidence >= config.tts.ocr_min_confidence
    {
        let (ocr_canonical_text, ocr_pages, ocr_stats) =
            ocr_artifacts_to_canonical_text(&ocr_artifacts, config, &artifacts.stats);
        let ocr_sentences =
            build_sentence_plan(&ocr_canonical_text, &artifacts.source_fingerprint, config);
        let ocr_mode = match ocr_artifacts.trust_class {
            OcrTrustClass::OcrHighTrust => PdfTtsMode::MixedTextTrust,
            OcrTrustClass::OcrMixedTrust => PdfTtsMode::MixedTextTrust,
            OcrTrustClass::OcrTextOnly => PdfTtsMode::RenderOnlyNoSync,
        };
        artifacts.text_source = TtsTextSourceKind::Ocr;
        artifacts.ocr_trust = Some(ocr_artifacts.trust_class);
        artifacts.ocr_confidence = Some(ocr_artifacts.confidence);
        artifacts.ocr_artifact_path = ocr_artifacts.artifact_path.clone();
        artifacts.mode = ocr_mode;
        artifacts.confidence = ocr_artifacts.confidence;
        artifacts.classification.reason =
            format!("ocr_artifact_loaded_{}", ocr_artifacts.trust_class.label());
        artifacts.tts_text = ocr_canonical_text.text.clone();
        artifacts.canonical_text = ocr_canonical_text;
        artifacts.pages = ocr_pages;
        artifacts.sentences = ocr_sentences;
        artifacts.stats = ocr_stats;
        artifacts.stats.sentence_count = artifacts.sentences.len();
    }

    let artifact_path = persist_artifacts(config, &artifacts)?;
    artifacts.artifact_path = Some(artifact_path.clone());

    debug!(
        ligatures_replaced = artifacts.stats.ligatures_replaced,
        soft_hyphens_removed = artifacts.stats.soft_hyphens_removed,
        zero_width_chars_removed = artifacts.stats.zero_width_chars_removed,
        duplicate_lines_removed = artifacts.stats.duplicate_lines_removed,
        repeated_edge_lines_removed = artifacts.stats.repeated_edge_lines_removed,
        joined_hyphenations = artifacts.stats.joined_hyphenations,
        collapsed_whitespace_runs = artifacts.stats.collapsed_whitespace_runs,
        table_like_blocks = artifacts.stats.table_like_blocks,
        caption_like_blocks = artifacts.stats.caption_like_blocks,
        footnote_like_blocks = artifacts.stats.footnote_like_blocks,
        sidenote_like_blocks = artifacts.stats.sidenote_like_blocks,
        sentence_count = artifacts.stats.sentence_count,
        "normalization edit summary for PDF TTS analysis"
    );
    info!(
        path = %source_path.display(),
        mode = %artifacts.mode.label(),
        text_source = ?artifacts.text_source,
        confidence = artifacts.confidence,
        sentences = artifacts.sentences.len(),
        chars = artifacts.stats.normalized_chars,
        blocks = artifacts.canonical_text.block_count,
        lines = artifacts.canonical_text.line_count,
        tokens = artifacts.canonical_text.token_count,
        analysis_start_page = artifacts.analysis_scope.start_page,
        analysis_end_page = artifacts.analysis_scope.end_page,
        full_document = artifacts.analysis_scope.full_document,
        column_reorders = artifacts.stats.column_reorders,
        rotated_segments_suppressed = artifacts.stats.rotated_segments_suppressed,
        duplicate_segments_suppressed = artifacts.stats.duplicate_segments_suppressed,
        block_fallback_units = artifacts.stats.block_fallback_units,
        classification_reason = %artifacts.classification.reason,
        artifact = %artifact_path.display(),
        "built PDF TTS analysis artifacts"
    );

    Ok(artifacts)
}

fn classify_pdf_for_tts(
    total_pages: usize,
    pages: &[PageTtsArtifact],
    stats: &NormalizationStats,
    config: &AppConfig,
) -> ClassificationResult {
    if stats.normalized_chars == 0 || stats.pages_with_text == 0 {
        return ClassificationResult {
            mode: PdfTtsMode::OcrRequired,
            confidence: 0.05,
            summary: ClassificationSummary {
                coverage_ratio: 0.0,
                duplicate_ratio: 0.0,
                boilerplate_ratio: 0.0,
                avg_chars_per_text_page: 0.0,
                avg_segments_per_text_page: 0.0,
                reason: "no_usable_embedded_text".into(),
            },
        };
    }

    if stats.sentence_count == 0 {
        return ClassificationResult {
            mode: PdfTtsMode::RenderOnlyNoSync,
            confidence: 0.15,
            summary: ClassificationSummary {
                coverage_ratio: stats.pages_with_text as f32 / total_pages.max(1) as f32,
                duplicate_ratio: 0.0,
                boilerplate_ratio: 0.0,
                avg_chars_per_text_page: stats.normalized_chars as f32
                    / stats.pages_with_text.max(1) as f32,
                avg_segments_per_text_page: 0.0,
                reason: "text_present_but_sentence_plan_empty".into(),
            },
        };
    }

    let coverage_ratio = stats.pages_with_text as f32 / total_pages.max(1) as f32;
    let duplicate_ratio =
        stats.duplicate_lines_removed as f32 / (stats.pages_with_text.max(1) as f32 * 4.0);
    let boilerplate_ratio =
        stats.repeated_edge_lines_removed as f32 / (stats.pages_with_text.max(1) as f32 * 4.0);
    let avg_chars_per_text_page =
        stats.normalized_chars as f32 / stats.pages_with_text.max(1) as f32;
    let avg_segments_per_text_page = pages
        .iter()
        .filter(|page| !page.empty_after_normalization)
        .map(|page| page.segment_count)
        .sum::<usize>() as f32
        / stats.pages_with_text.max(1) as f32;

    let high_trust = coverage_ratio >= config.tts.min_text_page_ratio
        && avg_chars_per_text_page >= config.tts.min_chars_per_text_page as f32
        && avg_segments_per_text_page >= config.tts.min_segments_per_text_page as f32
        && duplicate_ratio <= config.tts.max_duplicate_line_ratio
        && boilerplate_ratio <= config.tts.max_repeated_edge_line_ratio;

    if high_trust {
        let confidence = (0.72
            + (coverage_ratio * 0.1)
            + (avg_segments_per_text_page.min(120.0) / 120.0) * 0.08)
            .clamp(0.0, 0.98);
        return ClassificationResult {
            mode: PdfTtsMode::HighTextTrust,
            confidence,
            summary: ClassificationSummary {
                coverage_ratio,
                duplicate_ratio,
                boilerplate_ratio,
                avg_chars_per_text_page,
                avg_segments_per_text_page,
                reason: "embedded_text_meets_high_trust_thresholds".into(),
            },
        };
    }

    let mixed_confidence = (0.35
        + (coverage_ratio * 0.2)
        + ((avg_chars_per_text_page / config.tts.min_chars_per_text_page.max(1) as f32) * 0.05))
        .clamp(0.0, 0.7);
    ClassificationResult {
        mode: PdfTtsMode::MixedTextTrust,
        confidence: mixed_confidence,
        summary: ClassificationSummary {
            coverage_ratio,
            duplicate_ratio,
            boilerplate_ratio,
            avg_chars_per_text_page,
            avg_segments_per_text_page,
            reason: "embedded_text_usable_but_below_high_trust_thresholds".into(),
        },
    }
}

fn ocr_artifacts_to_canonical_text(
    ocr: &OcrArtifacts,
    _config: &AppConfig,
    baseline_stats: &NormalizationStats,
) -> (
    CanonicalTtsTextArtifact,
    Vec<PageTtsArtifact>,
    NormalizationStats,
) {
    let mut text = String::new();
    let mut pages = Vec::new();
    let mut canonical_pages = Vec::new();
    let mut block_count = 0usize;
    let mut line_count = 0usize;
    let mut token_count = 0usize;

    for page in &ocr.pages {
        if !text.is_empty() {
            text.push_str("\n\n");
        }
        let page_start = text.len();
        let mut canonical_blocks = Vec::new();

        for block in &page.blocks {
            if !canonical_blocks.is_empty() {
                text.push_str("\n\n");
            }
            let block_start = text.len();
            let mut canonical_lines = Vec::new();

            for line in &block.lines {
                let line_start = text.len();
                let mut canonical_tokens = Vec::new();
                for token in &line.tokens {
                    if text.len() > line_start {
                        text.push(' ');
                    }
                    let token_start = text.len();
                    text.push_str(&token.text);
                    let token_end = text.len();
                    canonical_tokens.push(CanonicalTokenArtifact {
                        page_index: token.page_index,
                        block_index: token.block_index,
                        line_index: token.line_index,
                        token_index: token.token_index,
                        text: token.text.clone(),
                        range: TextRange {
                            start: token_start,
                            end: token_end,
                        },
                    });
                    token_count += 1;
                }
                let line_end = text.len();
                canonical_lines.push(CanonicalLineArtifact {
                    page_index: line.page_index,
                    block_index: line.block_index,
                    line_index: line.line_index,
                    text: line.text.clone(),
                    range: TextRange {
                        start: line_start,
                        end: line_end,
                    },
                    tokens: canonical_tokens,
                });
                line_count += 1;
            }

            let block_end = text.len();
            canonical_blocks.push(CanonicalBlockArtifact {
                page_index: block.page_index,
                block_index: block.block_index,
                text: block.text.clone(),
                range: TextRange {
                    start: block_start,
                    end: block_end,
                },
                lines: canonical_lines,
            });
            block_count += 1;
        }

        let page_end = text.len();
        pages.push(PageTtsArtifact {
            page_index: page.page_index,
            original_char_count: page_end.saturating_sub(page_start),
            normalized_char_count: page_end.saturating_sub(page_start),
            segment_count: page
                .blocks
                .iter()
                .map(|block| {
                    block
                        .lines
                        .iter()
                        .map(|line| line.tokens.len())
                        .sum::<usize>()
                })
                .sum(),
            duplicate_lines_removed: 0,
            repeated_edge_lines_removed: 0,
            empty_after_normalization: canonical_blocks.is_empty(),
            range: (!canonical_blocks.is_empty()).then_some(TextRange {
                start: page_start,
                end: page_end,
            }),
        });
        canonical_pages.push(CanonicalPageArtifact {
            page_index: page.page_index,
            range: (!canonical_blocks.is_empty()).then_some(TextRange {
                start: page_start,
                end: page_end,
            }),
            blocks: canonical_blocks,
        });
    }

    let mut stats = baseline_stats.clone();
    stats.original_chars = text.chars().count();
    stats.normalized_chars = text.chars().count();
    stats.pages_with_text = pages
        .iter()
        .filter(|page| !page.empty_after_normalization)
        .count();
    stats.empty_pages = pages.len().saturating_sub(stats.pages_with_text);
    stats.extracted_blocks = block_count;
    stats.extracted_lines = line_count;

    (
        CanonicalTtsTextArtifact {
            text,
            pages: canonical_pages,
            block_count,
            line_count,
            token_count,
        },
        pages,
        stats,
    )
}

fn build_sentence_plan(
    canonical_text: &CanonicalTtsTextArtifact,
    fingerprint: &str,
    config: &AppConfig,
) -> Vec<SentencePlan> {
    let mut sentences = split_sentences(&canonical_text.text, config)
        .into_iter()
        .filter(|(range, sentence)| {
            sentence.chars().count() >= config.tts.min_sentence_chars && range.end > range.start
        })
        .map(|(range, sentence)| {
            sentence_plan_entry(
                range,
                sentence,
                SentenceUnitKind::Sentence,
                &canonical_text.pages,
                fingerprint,
            )
        })
        .collect::<Vec<_>>();

    if sentences.len() <= 1 {
        let fallback = canonical_text
            .pages
            .iter()
            .flat_map(|page| page.blocks.iter())
            .filter_map(|block| {
                let text = block.text.trim();
                (text.chars().count() >= config.tts.block_fallback_min_chars).then(|| {
                    sentence_plan_entry(
                        block.range.clone(),
                        text.to_string(),
                        SentenceUnitKind::BlockFallback,
                        &canonical_text.pages,
                        fingerprint,
                    )
                })
            })
            .collect::<Vec<_>>();
        if !fallback.is_empty() {
            sentences = fallback;
        }
    }

    sentences
}

fn sentence_plan_entry(
    range: TextRange,
    sentence: String,
    unit_kind: SentenceUnitKind,
    pages: &[CanonicalPageArtifact],
    fingerprint: &str,
) -> SentencePlan {
    let page_range = sentence_page_range(&range, pages);
    let mut hasher = DefaultHasher::new();
    fingerprint.hash(&mut hasher);
    range.start.hash(&mut hasher);
    range.end.hash(&mut hasher);
    sentence.hash(&mut hasher);
    unit_kind.hash(&mut hasher);
    SentencePlan {
        id: hasher.finish(),
        text: sentence,
        range,
        page_range,
        unit_kind,
    }
}

fn sentence_page_range(range: &TextRange, pages: &[CanonicalPageArtifact]) -> PageRange {
    let mut start_page = 0;
    let mut end_page = 0;

    for page in pages {
        let Some(page_range) = &page.range else {
            continue;
        };
        if page_range.end > range.start {
            start_page = page.page_index;
            break;
        }
    }

    for page in pages.iter().rev() {
        let Some(page_range) = &page.range else {
            continue;
        };
        if page_range.start < range.end {
            end_page = page.page_index;
            break;
        }
    }

    PageRange {
        start_page,
        end_page,
    }
}

fn append_page_to_canonical_text(
    full_text: &mut String,
    page_index: usize,
    normalized: &NormalizedPageText,
    line_count: &mut usize,
    token_count: &mut usize,
) -> Vec<CanonicalBlockArtifact> {
    let page_start = full_text.len();
    let mut blocks = Vec::new();

    for (block_index, block) in normalized.blocks.iter().enumerate() {
        if block.lines.is_empty() {
            continue;
        }
        if full_text.len() > page_start {
            full_text.push_str("\n\n");
        }
        let block_start = full_text.len();
        let mut lines = Vec::new();

        for (line_index, line_text) in block.lines.iter().enumerate() {
            let line_start = full_text.len();
            let mut tokens = Vec::new();
            for (token_index, token) in line_text.split_whitespace().enumerate() {
                if full_text.len() > line_start {
                    full_text.push(' ');
                }
                let token_start = full_text.len();
                full_text.push_str(token);
                let token_end = full_text.len();
                tokens.push(CanonicalTokenArtifact {
                    page_index,
                    block_index,
                    line_index,
                    token_index,
                    text: token.to_string(),
                    range: TextRange {
                        start: token_start,
                        end: token_end,
                    },
                });
                *token_count += 1;
            }

            let line_end = full_text.len();
            lines.push(CanonicalLineArtifact {
                page_index,
                block_index,
                line_index,
                text: line_text.clone(),
                range: TextRange {
                    start: line_start,
                    end: line_end,
                },
                tokens,
            });
            *line_count += 1;
        }

        let block_end = full_text.len();
        blocks.push(CanonicalBlockArtifact {
            page_index,
            block_index,
            text: full_text[block_start..block_end].to_string(),
            range: TextRange {
                start: block_start,
                end: block_end,
            },
            lines,
        });
    }

    blocks
}

fn extract_page_text_for_tts(
    segments: &[crate::pdf::TextSegmentData],
    config: &AppConfig,
) -> ExtractedPageText {
    let mut stats = PageExtractionStats::default();
    let mut filtered = Vec::new();
    let mut seen = HashSet::new();
    let mut scratch = PageNormalizationStats::default();

    for segment in segments {
        let text = collapse_inline_whitespace(segment.text.trim(), &mut scratch);
        if text.is_empty() {
            continue;
        }
        let width = (segment.rect.right - segment.rect.left).abs();
        let height = (segment.rect.top - segment.rect.bottom).abs();
        if config.tts.suppress_rotated_narrow_segments
            && height > 0.0
            && width > 0.0
            && (height / width.max(0.01)) >= config.tts.rotated_segment_aspect_ratio
        {
            stats.rotated_segments_suppressed += 1;
            continue;
        }

        let key = (
            text.to_ascii_lowercase(),
            (segment.rect.left * 2.0).round() as i32,
            (segment.rect.top * 2.0).round() as i32,
            (segment.rect.right * 2.0).round() as i32,
            (segment.rect.bottom * 2.0).round() as i32,
        );
        if !seen.insert(key) {
            stats.duplicate_segments_suppressed += 1;
            continue;
        }

        filtered.push(segment.clone());
    }

    let lines = group_segments_into_lines(&filtered, config);
    let ordered = reorder_lines_by_columns(lines, config, &mut stats);
    let blocks = build_blocks_from_lines(&ordered, config);
    let text = blocks.join("\n\n");

    ExtractedPageText { text, stats }
}

fn group_segments_into_lines(
    segments: &[crate::pdf::TextSegmentData],
    config: &AppConfig,
) -> Vec<PositionedLine> {
    let mut sorted = segments.to_vec();
    sorted.sort_by(|a, b| {
        b.rect
            .top
            .partial_cmp(&a.rect.top)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| {
                a.rect
                    .left
                    .partial_cmp(&b.rect.left)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
    });

    let mut lines: Vec<Vec<crate::pdf::TextSegmentData>> = Vec::new();
    for segment in sorted {
        let center = (segment.rect.top + segment.rect.bottom) * 0.5;
        let mut placed = false;
        for line in &mut lines {
            let reference = line
                .first()
                .map(|entry| (entry.rect.top + entry.rect.bottom) * 0.5)
                .unwrap_or(center);
            let height = line
                .first()
                .map(|entry| (entry.rect.top - entry.rect.bottom).abs())
                .unwrap_or(10.0);
            let line_right = line
                .iter()
                .map(|entry| entry.rect.right)
                .fold(f32::MIN, f32::max);
            if (reference - center).abs()
                <= height.max(6.0) * config.tts.line_merge_vertical_tolerance
                && (segment.rect.left - line_right) <= config.tts.column_split_min_gap * 0.5
            {
                line.push(segment.clone());
                placed = true;
                break;
            }
        }
        if !placed {
            lines.push(vec![segment]);
        }
    }

    lines
        .into_iter()
        .map(|mut line| {
            line.sort_by(|a, b| {
                a.rect
                    .left
                    .partial_cmp(&b.rect.left)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            let text = line
                .iter()
                .map(|segment| segment.text.trim())
                .filter(|text| !text.is_empty())
                .collect::<Vec<_>>()
                .join(" ");
            let left = line.first().map(|segment| segment.rect.left).unwrap_or(0.0);
            let top = line
                .iter()
                .map(|segment| segment.rect.top)
                .fold(f32::MIN, f32::max);
            let bottom = line
                .iter()
                .map(|segment| segment.rect.bottom)
                .fold(f32::MAX, f32::min);
            PositionedLine {
                text,
                left,
                top,
                bottom,
            }
        })
        .filter(|line| !line.text.trim().is_empty())
        .collect()
}

fn reorder_lines_by_columns(
    mut lines: Vec<PositionedLine>,
    config: &AppConfig,
    stats: &mut PageExtractionStats,
) -> Vec<PositionedLine> {
    if lines.len() < config.tts.column_detection_min_lines {
        lines.sort_by(|a, b| {
            b.top
                .partial_cmp(&a.top)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| {
                    a.left
                        .partial_cmp(&b.left)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
        });
        return lines;
    }

    let mut lefts = lines.iter().map(|line| line.left).collect::<Vec<_>>();
    lefts.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mut split_at = None;
    let mut biggest_gap = 0.0f32;
    for pair in lefts.windows(2) {
        let gap = pair[1] - pair[0];
        if gap > biggest_gap {
            biggest_gap = gap;
            split_at = Some(pair[0] + gap * 0.5);
        }
    }

    if biggest_gap < config.tts.column_split_min_gap {
        lines.sort_by(|a, b| {
            b.top
                .partial_cmp(&a.top)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| {
                    a.left
                        .partial_cmp(&b.left)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
        });
        return lines;
    }

    let split = split_at.unwrap_or(0.0);
    let mut left_column = lines
        .iter()
        .filter(|line| line.left <= split)
        .cloned()
        .collect::<Vec<_>>();
    let mut right_column = lines
        .iter()
        .filter(|line| line.left > split)
        .cloned()
        .collect::<Vec<_>>();
    if left_column.len() < 2 || right_column.len() < 2 {
        lines.sort_by(|a, b| {
            b.top
                .partial_cmp(&a.top)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| {
                    a.left
                        .partial_cmp(&b.left)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
        });
        return lines;
    }

    left_column.sort_by(|a, b| {
        b.top
            .partial_cmp(&a.top)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    right_column.sort_by(|a, b| {
        b.top
            .partial_cmp(&a.top)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    stats.column_reorders += 1;
    left_column.into_iter().chain(right_column).collect()
}

fn build_blocks_from_lines(lines: &[PositionedLine], config: &AppConfig) -> Vec<String> {
    let mut blocks = Vec::new();
    let mut current = Vec::new();
    let mut previous: Option<&PositionedLine> = None;

    for line in lines {
        let should_break = previous.is_some_and(|prev| {
            let gap = (prev.bottom - line.top).abs();
            let height = (prev.top - prev.bottom).abs().max(8.0);
            gap > height * config.tts.block_vertical_gap_multiplier
                || (prev.left - line.left).abs() > config.tts.column_split_min_gap * 0.5
        });
        if should_break && !current.is_empty() {
            blocks.push(current.join("\n"));
            current.clear();
        }
        current.push(line.text.clone());
        previous = Some(line);
    }

    if !current.is_empty() {
        blocks.push(current.join("\n"));
    }

    blocks
}

fn split_sentences(text: &str, config: &AppConfig) -> Vec<(TextRange, String)> {
    let mut out = Vec::new();
    let abbreviation_set: HashSet<String> = config
        .tts
        .abbreviations
        .iter()
        .map(|value| value.trim().to_ascii_lowercase())
        .collect();
    let boundary_markers = config
        .tts
        .sentence_boundary_markers
        .iter()
        .filter_map(|marker| marker.chars().next())
        .collect::<HashSet<_>>();
    let bytes = text.as_bytes();
    let mut start = 0usize;
    let mut index = 0usize;

    while index < bytes.len() {
        let ch = text[index..].chars().next().unwrap_or_default();
        let ch_len = ch.len_utf8();
        let should_break = match ch {
            marker if boundary_markers.contains(&marker) => {
                let candidate = text[start..index + ch_len].trim();
                let last_token = candidate
                    .split_whitespace()
                    .last()
                    .unwrap_or_default()
                    .trim_matches(|c: char| {
                        c == '"' || c == '\'' || c == ')' || c == ']' || c == '}'
                    })
                    .to_ascii_lowercase();
                let next_non_ws = text[index + ch_len..].chars().find(|c| !c.is_whitespace());
                !abbreviation_set.contains(&last_token)
                    && next_non_ws.is_some_and(|next| {
                        if config.tts.language.starts_with("en")
                            || config.tts.language.starts_with("es")
                        {
                            next.is_uppercase()
                                || matches!(
                                    next,
                                    '"' | '\'' | '(' | '[' | '{' | '\u{00BF}' | '\u{00A1}'
                                )
                        } else {
                            true
                        }
                    })
            }
            '\n' => {
                config.tts.sentence_break_on_double_newline
                    && text[index + ch_len..].starts_with('\n')
            }
            _ => false,
        };

        if should_break {
            let end = index + ch_len;
            let sentence = text[start..end].trim().to_string();
            if !sentence.is_empty() {
                let trimmed_start = start
                    + text[start..end]
                        .find(|c: char| !c.is_whitespace())
                        .unwrap_or(0);
                let trimmed_end = trimmed_start + sentence.len();
                out.push((
                    TextRange {
                        start: trimmed_start,
                        end: trimmed_end,
                    },
                    sentence,
                ));
            }

            start = end;
            while start < bytes.len() {
                let next = text[start..].chars().next().unwrap_or_default();
                if !next.is_whitespace() {
                    break;
                }
                start += next.len_utf8();
            }
            index = start;
            continue;
        }

        index += ch_len;
    }

    if start < text.len() {
        let sentence = text[start..].trim().to_string();
        if !sentence.is_empty() {
            let trimmed_start = start
                + text[start..]
                    .find(|c: char| !c.is_whitespace())
                    .unwrap_or(0);
            let trimmed_end = trimmed_start + sentence.len();
            out.push((
                TextRange {
                    start: trimmed_start,
                    end: trimmed_end,
                },
                sentence,
            ));
        }
    }

    out
}

#[derive(Debug, Default)]
struct PageNormalizationStats {
    ligatures_replaced: usize,
    soft_hyphens_removed: usize,
    zero_width_chars_removed: usize,
    duplicate_lines_removed: usize,
    repeated_edge_lines_removed: usize,
    joined_hyphenations: usize,
    collapsed_whitespace_runs: usize,
    table_like_blocks: usize,
    caption_like_blocks: usize,
    footnote_like_blocks: usize,
    sidenote_like_blocks: usize,
}

#[derive(Debug)]
struct NormalizedPageText {
    text: String,
    stats: PageNormalizationStats,
    blocks: Vec<NormalizedBlockText>,
}

#[derive(Debug, Clone)]
struct NormalizedBlockText {
    lines: Vec<String>,
}

#[derive(Debug, Default, Clone)]
struct PageExtractionStats {
    rotated_segments_suppressed: usize,
    duplicate_segments_suppressed: usize,
    column_reorders: usize,
}

#[derive(Debug, Clone)]
struct ExtractedPageText {
    text: String,
    stats: PageExtractionStats,
}

#[derive(Debug, Clone)]
struct PositionedLine {
    text: String,
    left: f32,
    top: f32,
    bottom: f32,
}

fn normalize_page_text(
    raw_text: &str,
    repeated_edge_lines: &HashSet<String>,
    config: &AppConfig,
) -> NormalizedPageText {
    let mut stats = PageNormalizationStats::default();
    let mut normalized = raw_text.replace("\r\n", "\n");

    for (ligature, replacement) in PDF_LIGATURES {
        let count = normalized.matches(ligature).count();
        if count > 0 {
            stats.ligatures_replaced += count;
            normalized = normalized.replace(ligature, replacement);
        }
    }

    let soft_hyphens = normalized.matches('\u{00AD}').count();
    if soft_hyphens > 0 {
        stats.soft_hyphens_removed += soft_hyphens;
        normalized = normalized.replace('\u{00AD}', "");
    }

    let zero_widths = ['\u{200B}', '\u{200C}', '\u{200D}', '\u{2060}', '\u{FEFF}'];
    for marker in zero_widths {
        let count = normalized.matches(marker).count();
        if count > 0 {
            stats.zero_width_chars_removed += count;
            normalized = normalized.replace(marker, "");
        }
    }

    let mut filtered_lines = Vec::new();
    let mut previous_normalized_line = String::new();
    for line in normalized.lines() {
        let compact = collapse_inline_whitespace(line.trim(), &mut stats);
        if compact.is_empty() {
            filtered_lines.push(String::new());
            previous_normalized_line.clear();
            continue;
        }
        if repeated_edge_lines.contains(&compact) {
            stats.repeated_edge_lines_removed += 1;
            continue;
        }
        if compact == previous_normalized_line {
            stats.duplicate_lines_removed += 1;
            continue;
        }
        previous_normalized_line = compact.clone();
        filtered_lines.push(compact);
    }

    let mut blocks = Vec::new();
    let mut current_block = Vec::new();
    for line in filtered_lines {
        if line.is_empty() {
            if !current_block.is_empty() {
                blocks.push(NormalizedBlockText {
                    lines: current_block.clone(),
                });
                current_block.clear();
            }
            continue;
        }

        if current_block.is_empty() {
            current_block.push(line);
            continue;
        }

        if current_block
            .last()
            .is_some_and(|current| current.ends_with('-'))
            && line
                .chars()
                .next()
                .is_some_and(|ch| ch.is_ascii_lowercase())
        {
            if let Some(current) = current_block.last_mut() {
                current.pop();
                current.push_str(&line);
            }
            stats.joined_hyphenations += 1;
        } else {
            current_block.push(line);
        }
    }

    if !current_block.is_empty() {
        blocks.push(NormalizedBlockText {
            lines: current_block,
        });
    }

    let blocks = blocks
        .into_iter()
        .filter(|block| block.lines.join(" ").chars().count() >= config.tts.min_chars_per_line_kept)
        .collect::<Vec<_>>();

    for block in &blocks {
        let joined = block.lines.join(" ");
        if is_table_like_block(&joined) {
            stats.table_like_blocks += 1;
        }
        if is_caption_like_block(&joined) {
            stats.caption_like_blocks += 1;
        }
        if is_footnote_like_block(&joined) {
            stats.footnote_like_blocks += 1;
        }
        if is_sidenote_like_block(block) {
            stats.sidenote_like_blocks += 1;
        }
    }

    let text = blocks
        .iter()
        .map(|block| block.lines.join(" "))
        .collect::<Vec<_>>()
        .join("\n\n");

    NormalizedPageText {
        text,
        stats,
        blocks,
    }
}

fn collapse_inline_whitespace(line: &str, stats: &mut PageNormalizationStats) -> String {
    let mut out = String::with_capacity(line.len());
    let mut saw_ws = false;

    for ch in line.chars() {
        if ch.is_whitespace() {
            if !saw_ws {
                out.push(' ');
                saw_ws = true;
            } else {
                stats.collapsed_whitespace_runs += 1;
            }
        } else {
            saw_ws = false;
            out.push(ch);
        }
    }

    out.trim().to_string()
}

fn is_table_like_block(text: &str) -> bool {
    let tokens = text.split_whitespace().collect::<Vec<_>>();
    let numeric_tokens = tokens
        .iter()
        .filter(|token| token.chars().any(|ch| ch.is_ascii_digit()))
        .count();
    tokens.len() >= 4 && numeric_tokens * 2 >= tokens.len()
}

fn is_caption_like_block(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.starts_with("figure ")
        || lower.starts_with("fig. ")
        || lower.starts_with("table ")
        || lower.starts_with("caption ")
}

fn is_footnote_like_block(text: &str) -> bool {
    text.chars()
        .next()
        .is_some_and(|ch| ch.is_ascii_digit() || matches!(ch, '*' | '\u{2020}'))
}

fn is_sidenote_like_block(block: &NormalizedBlockText) -> bool {
    block.lines.len() <= 2
        && block
            .lines
            .iter()
            .all(|line| line.split_whitespace().count() <= 6)
}

fn collect_repeated_edge_lines(
    config: &AppConfig,
    document: &PdfDocument<'_>,
) -> Result<HashSet<String>> {
    let mut counts: HashMap<String, HashSet<usize>> = HashMap::new();

    for page_index in 0..document.metadata.page_count {
        let page_text = document.full_text_for_page(page_index)?;
        let lines = page_text
            .lines()
            .map(|line| line.trim())
            .filter(|line| !line.is_empty())
            .map(str::to_string)
            .collect::<Vec<_>>();

        let mut candidates = Vec::new();
        let edge_depth = config.tts.page_edge_line_scan_depth;
        candidates.extend(lines.iter().take(edge_depth).cloned());
        candidates.extend(lines.iter().rev().take(edge_depth).cloned());

        for candidate in candidates {
            let entry = counts.entry(candidate).or_default();
            entry.insert(page_index);
        }
    }

    Ok(counts
        .into_iter()
        .filter(|(line, pages)| {
            pages.len() >= config.tts.repeated_edge_line_min_pages
                && line.chars().count() <= config.tts.max_edge_line_length
        })
        .map(|(line, _)| line)
        .collect())
}

fn fingerprint_document_path(path: &Path) -> Result<String> {
    let metadata = fs::metadata(path)
        .with_context(|| format!("failed to read metadata for {}", path.display()))?;
    let modified_secs = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs())
        .unwrap_or(0);

    let mut hasher = DefaultHasher::new();
    path.display().to_string().hash(&mut hasher);
    metadata.len().hash(&mut hasher);
    modified_secs.hash(&mut hasher);
    Ok(format!("{:016x}", hasher.finish()))
}

fn unix_timestamp_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
#[path = "../tests/tts_unit.rs"]
mod tests;
