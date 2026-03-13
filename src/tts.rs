use std::{
    collections::{HashMap, HashSet},
    fs,
    hash::{DefaultHasher, Hash, Hasher},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tracing::{info, instrument};

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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SentencePlan {
    pub id: u64,
    pub text: String,
    pub range: TextRange,
    pub page_range: PageRange,
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
    pub sentence_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TtsAnalysisArtifacts {
    pub source_path: PathBuf,
    pub source_fingerprint: String,
    pub generated_at_unix_secs: u64,
    pub mode: PdfTtsMode,
    pub confidence: f32,
    pub classification: ClassificationSummary,
    pub tts_text: String,
    pub canonical_text: CanonicalTtsTextArtifact,
    pub sentences: Vec<SentencePlan>,
    pub pages: Vec<PageTtsArtifact>,
    pub stats: NormalizationStats,
    pub artifact_path: Option<PathBuf>,
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
}

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
        clip: PreparedSentenceClip,
        elapsed_ms: u64,
    },
    PrefetchFailed {
        request_id: u64,
        sentence_index: usize,
        error: String,
        elapsed_ms: u64,
    },
    SyncCompleted {
        request_id: u64,
        target: SentenceSyncTarget,
        elapsed_ms: u64,
    },
    SyncFailed {
        request_id: u64,
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

pub fn compute_sentence_sync(
    config: &AppConfig,
    analysis: &TtsAnalysisArtifacts,
    sentence_index: usize,
) -> Result<SentenceSyncTarget> {
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
    compute_sentence_sync_from_document(&document, analysis, sentence_index)
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
    }
}

pub fn prepare_sentence_clip(
    config: &AppConfig,
    analysis: &TtsAnalysisArtifacts,
    sentence_index: usize,
    engine: TtsEngineKind,
) -> Result<PreparedSentenceClip> {
    let sentence = analysis
        .sentences
        .get(sentence_index)
        .with_context(|| format!("sentence index {sentence_index} is out of bounds"))?;
    let stem = format!(
        "{}-{}-{}-{:.2}",
        analysis.source_fingerprint,
        sentence.id,
        engine.label(),
        config.tts.rate
    );
    let manifest_path = config.tts_audio_cache_dir()?.join(format!("{stem}.toml"));
    let audio_path = match engine {
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
        if (cached.rate - config.tts.rate).abs() < f32::EPSILON && audio_ok {
            cached.cache_hit = true;
            return Ok(cached);
        }
    }

    if let Some(parent) = manifest_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }

    let estimated_duration_ms = estimate_sentence_duration_ms(&sentence.text, config.tts.rate);
    if let Some(audio_path) = &audio_path {
        generate_tone_preview_clip(audio_path, sentence_index, estimated_duration_ms)?;
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
        rate: config.tts.rate,
        generated_at_unix_secs: unix_timestamp_secs(),
        cache_hit: false,
    };

    fs::write(&manifest_path, toml::to_string_pretty(&clip)?)
        .with_context(|| format!("failed to write {}", manifest_path.display()))?;

    Ok(clip)
}

pub fn estimate_sentence_duration_ms(text: &str, rate: f32) -> u64 {
    let word_count = text.split_whitespace().count().max(1) as u64;
    let punctuation_pause = text
        .chars()
        .filter(|ch| matches!(ch, '.' | ',' | ';' | ':' | '?' | '!'))
        .count() as u64
        * 120;
    let base = (word_count * 360 + punctuation_pause).clamp(900, 12_000) as f32;
    (base / rate.max(0.25)).round() as u64
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
                return SentenceSyncTarget {
                    sentence_index,
                    sentence_id,
                    confidence: SentenceSyncConfidence::ExactSentence,
                    page_index: Some(page_index),
                    rects,
                    fallback_reason: "exact_substring_match".into(),
                    score: 1.0,
                };
            }
        }
    }

    let mut best_window: Option<(usize, usize, f32)> = None;
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
            let score = token_coverage_score(sentence_tokens, &window_text);
            if best_window
                .as_ref()
                .is_none_or(|(_, _, current_score)| score > *current_score)
            {
                best_window = Some((start, end, score));
            }
        }
    }

    if let Some((start, end, score)) = best_window {
        let rects = segments[start..=end]
            .iter()
            .map(|segment| segment.rect)
            .collect::<Vec<_>>();
        let confidence = if score >= 0.9 {
            SentenceSyncConfidence::FuzzySentence
        } else if score >= 0.45 {
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
                SentenceSyncConfidence::PageFallback => "page_location_only".into(),
                _ => "unknown".into(),
            },
            score,
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

#[instrument(skip(config, document))]
pub fn build_artifacts_from_document(
    config: &AppConfig,
    source_path: &Path,
    document: &PdfDocument<'_>,
) -> Result<TtsAnalysisArtifacts> {
    let fingerprint = fingerprint_document_path(source_path)?;
    let repeated_edge_lines = collect_repeated_edge_lines(config, document)?;
    let mut stats = NormalizationStats::default();
    let mut full_text = String::new();
    let mut pages = Vec::with_capacity(document.metadata.page_count);
    let mut canonical_pages = Vec::with_capacity(document.metadata.page_count);
    let mut block_count = 0usize;
    let mut line_count = 0usize;
    let mut token_count = 0usize;

    for page_index in 0..document.metadata.page_count {
        let page_text = document.full_text_for_page(page_index)?;
        let segment_count = document.text_segments_for_page(page_index)?.len();
        let normalized = normalize_page_text(&page_text, &repeated_edge_lines, config);

        stats.original_chars += page_text.chars().count();
        stats.normalized_chars += normalized.text.chars().count();
        stats.ligatures_replaced += normalized.stats.ligatures_replaced;
        stats.soft_hyphens_removed += normalized.stats.soft_hyphens_removed;
        stats.zero_width_chars_removed += normalized.stats.zero_width_chars_removed;
        stats.duplicate_lines_removed += normalized.stats.duplicate_lines_removed;
        stats.repeated_edge_lines_removed += normalized.stats.repeated_edge_lines_removed;
        stats.joined_hyphenations += normalized.stats.joined_hyphenations;
        stats.collapsed_whitespace_runs += normalized.stats.collapsed_whitespace_runs;

        if normalized.text.trim().is_empty() {
            stats.empty_pages += 1;
            pages.push(PageTtsArtifact {
                page_index,
                original_char_count: page_text.chars().count(),
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
            original_char_count: page_text.chars().count(),
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

    let classification = classify_pdf_for_tts(document.metadata.page_count, &pages, &stats, config);

    let mut artifacts = TtsAnalysisArtifacts {
        source_path: source_path.to_path_buf(),
        source_fingerprint: fingerprint,
        generated_at_unix_secs: unix_timestamp_secs(),
        mode: classification.mode,
        confidence: classification.confidence,
        classification: classification.summary,
        tts_text: canonical_text.text.clone(),
        canonical_text,
        sentences,
        pages,
        stats,
        artifact_path: None,
    };

    let artifact_path = persist_artifacts(config, &artifacts)?;
    artifacts.artifact_path = Some(artifact_path.clone());

    info!(
        path = %source_path.display(),
        mode = %artifacts.mode.label(),
        confidence = artifacts.confidence,
        sentences = artifacts.sentences.len(),
        chars = artifacts.stats.normalized_chars,
        blocks = artifacts.canonical_text.block_count,
        lines = artifacts.canonical_text.line_count,
        tokens = artifacts.canonical_text.token_count,
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

fn build_sentence_plan(
    canonical_text: &CanonicalTtsTextArtifact,
    fingerprint: &str,
    config: &AppConfig,
) -> Vec<SentencePlan> {
    split_sentences(&canonical_text.text, &config.tts.abbreviations)
        .into_iter()
        .map(|(range, sentence)| {
            let page_range = sentence_page_range(&range, &canonical_text.pages);
            let mut hasher = DefaultHasher::new();
            fingerprint.hash(&mut hasher);
            range.start.hash(&mut hasher);
            range.end.hash(&mut hasher);
            sentence.hash(&mut hasher);
            SentencePlan {
                id: hasher.finish(),
                text: sentence,
                range,
                page_range,
            }
        })
        .collect()
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

fn split_sentences(text: &str, abbreviations: &[String]) -> Vec<(TextRange, String)> {
    let mut out = Vec::new();
    let abbreviation_set: HashSet<String> = abbreviations
        .iter()
        .map(|value| value.trim().to_ascii_lowercase())
        .collect();
    let bytes = text.as_bytes();
    let mut start = 0usize;
    let mut index = 0usize;

    while index < bytes.len() {
        let ch = text[index..].chars().next().unwrap_or_default();
        let ch_len = ch.len_utf8();
        let should_break = match ch {
            '.' | '!' | '?' => {
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
                !abbreviation_set.contains(&last_token) && next_non_ws.is_some()
            }
            '\n' => text[index + ch_len..].starts_with('\n'),
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
mod tests {
    use super::*;
    use crate::config::AppConfig;
    use crate::pdf::TextSegmentData;

    fn sample_canonical_text(text: &str) -> CanonicalTtsTextArtifact {
        CanonicalTtsTextArtifact {
            text: text.into(),
            pages: vec![CanonicalPageArtifact {
                page_index: 0,
                range: Some(TextRange {
                    start: 0,
                    end: text.len(),
                }),
                blocks: vec![CanonicalBlockArtifact {
                    page_index: 0,
                    block_index: 0,
                    text: text.into(),
                    range: TextRange {
                        start: 0,
                        end: text.len(),
                    },
                    lines: vec![CanonicalLineArtifact {
                        page_index: 0,
                        block_index: 0,
                        line_index: 0,
                        text: text.into(),
                        range: TextRange {
                            start: 0,
                            end: text.len(),
                        },
                        tokens: text
                            .split_whitespace()
                            .enumerate()
                            .scan(0usize, |cursor, (token_index, token)| {
                                if *cursor > 0 {
                                    *cursor += 1;
                                }
                                let start = *cursor;
                                *cursor += token.len();
                                Some(CanonicalTokenArtifact {
                                    page_index: 0,
                                    block_index: 0,
                                    line_index: 0,
                                    token_index,
                                    text: token.into(),
                                    range: TextRange {
                                        start,
                                        end: *cursor,
                                    },
                                })
                            })
                            .collect(),
                    }],
                }],
            }],
            block_count: 1,
            line_count: 1,
            token_count: text.split_whitespace().count(),
        }
    }

    fn sample_classification(mode: PdfTtsMode, _confidence: f32) -> ClassificationSummary {
        ClassificationSummary {
            coverage_ratio: 1.0,
            duplicate_ratio: 0.0,
            boilerplate_ratio: 0.0,
            avg_chars_per_text_page: 100.0,
            avg_segments_per_text_page: 32.0,
            reason: mode.label().into(),
        }
    }

    #[test]
    fn normalization_replaces_ligatures_and_soft_hyphens() {
        let config = AppConfig::default();
        let repeated = HashSet::new();
        let normalized = normalize_page_text(
            "of\u{FB01}ce co\u{00AD}operate\n\nHeader",
            &repeated,
            &config,
        );

        assert!(normalized.text.contains("office"));
        assert!(normalized.text.contains("cooperate"));
        assert_eq!(normalized.stats.ligatures_replaced, 1);
        assert_eq!(normalized.stats.soft_hyphens_removed, 1);
    }

    #[test]
    fn normalization_suppresses_repeated_edge_lines_and_duplicates() {
        let config = AppConfig::default();
        let repeated = HashSet::from([String::from("Header")]);
        let normalized = normalize_page_text("Header\nAlpha\nAlpha\nBeta", &repeated, &config);

        assert_eq!(normalized.text, "Alpha Beta");
        assert_eq!(normalized.stats.repeated_edge_lines_removed, 1);
        assert_eq!(normalized.stats.duplicate_lines_removed, 1);
    }

    #[test]
    fn sentence_splitter_respects_abbreviations() {
        let mut config = AppConfig::default();
        config.tts.abbreviations = vec!["dr.".into()];

        let sentences = build_sentence_plan(
            &sample_canonical_text("Dr. Smith arrived. Then he left."),
            "fixture",
            &config,
        );

        assert_eq!(sentences.len(), 2);
        assert_eq!(sentences[0].text, "Dr. Smith arrived.");
        assert_eq!(sentences[1].text, "Then he left.");
    }

    #[test]
    fn classifier_marks_empty_documents_as_ocr_required() {
        let config = AppConfig::default();
        let stats = NormalizationStats::default();
        let classification = classify_pdf_for_tts(4, &[], &stats, &config);

        assert_eq!(classification.mode, PdfTtsMode::OcrRequired);
        assert!(classification.confidence < 0.1);
        assert_eq!(classification.summary.reason, "no_usable_embedded_text");
    }

    #[test]
    fn classifier_marks_clean_text_as_high_trust() {
        let config = AppConfig::default();
        let pages = vec![
            PageTtsArtifact {
                page_index: 0,
                original_char_count: 400,
                normalized_char_count: 380,
                segment_count: 120,
                duplicate_lines_removed: 0,
                repeated_edge_lines_removed: 0,
                empty_after_normalization: false,
                range: Some(TextRange { start: 0, end: 380 }),
            },
            PageTtsArtifact {
                page_index: 1,
                original_char_count: 420,
                normalized_char_count: 390,
                segment_count: 130,
                duplicate_lines_removed: 0,
                repeated_edge_lines_removed: 0,
                empty_after_normalization: false,
                range: Some(TextRange {
                    start: 382,
                    end: 772,
                }),
            },
        ];
        let stats = NormalizationStats {
            pages_with_text: 2,
            empty_pages: 0,
            original_chars: 820,
            normalized_chars: 770,
            sentence_count: 18,
            ..NormalizationStats::default()
        };

        let classification = classify_pdf_for_tts(2, &pages, &stats, &config);
        assert_eq!(classification.mode, PdfTtsMode::HighTextTrust);
        assert!(classification.confidence > 0.7);
        assert_eq!(
            classification.summary.reason,
            "embedded_text_meets_high_trust_thresholds"
        );
    }

    #[test]
    fn prefetch_plan_respects_window() {
        let analysis = TtsAnalysisArtifacts {
            source_path: PathBuf::from("fixture.pdf"),
            source_fingerprint: "abc".into(),
            generated_at_unix_secs: 0,
            mode: PdfTtsMode::HighTextTrust,
            confidence: 0.9,
            classification: sample_classification(PdfTtsMode::HighTextTrust, 0.9),
            tts_text: "a b c".into(),
            canonical_text: sample_canonical_text("a b c"),
            sentences: (0..5)
                .map(|index| SentencePlan {
                    id: index as u64,
                    text: format!("Sentence {index}."),
                    range: TextRange {
                        start: index,
                        end: index + 1,
                    },
                    page_range: PageRange {
                        start_page: 0,
                        end_page: 0,
                    },
                })
                .collect(),
            pages: Vec::new(),
            stats: NormalizationStats::default(),
            artifact_path: None,
        };

        assert_eq!(build_prefetch_plan(&analysis, 2, 2), vec![2, 3]);
        assert_eq!(build_prefetch_plan(&analysis, 4, 8), vec![4]);
    }

    #[test]
    fn sentence_duration_estimate_has_reasonable_floor() {
        assert!(estimate_sentence_duration_ms("Hi.", 1.0) >= 900);
        assert!(estimate_sentence_duration_ms("This is a somewhat longer sentence.", 1.0) > 900);
    }

    #[test]
    fn sentence_ids_are_stable_for_same_canonical_text() {
        let config = AppConfig::default();
        let canonical = sample_canonical_text("Alpha. Beta.");

        let first = build_sentence_plan(&canonical, "fixture", &config);
        let second = build_sentence_plan(&canonical, "fixture", &config);

        assert_eq!(
            first.iter().map(|sentence| sentence.id).collect::<Vec<_>>(),
            second
                .iter()
                .map(|sentence| sentence.id)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn canonical_text_artifact_preserves_token_provenance() {
        let canonical = sample_canonical_text("Alpha beta");
        assert_eq!(canonical.block_count, 1);
        assert_eq!(canonical.line_count, 1);
        assert_eq!(canonical.token_count, 2);
        assert_eq!(canonical.pages[0].blocks[0].lines[0].tokens[1].text, "beta");
    }

    #[test]
    fn prepare_sentence_clip_writes_tone_preview_files() {
        let temp = tempfile::tempdir().unwrap();
        let source_path = temp.path().join("fixture.pdf");
        fs::write(&source_path, b"%PDF-1.4").unwrap();

        let mut config = AppConfig::default();
        config.tts.audio_cache_dir = temp.path().join("audio").display().to_string();

        let analysis = TtsAnalysisArtifacts {
            source_path,
            source_fingerprint: "abc".into(),
            generated_at_unix_secs: 0,
            mode: PdfTtsMode::HighTextTrust,
            confidence: 0.9,
            tts_text: "Sentence zero.".into(),
            canonical_text: sample_canonical_text("Sentence zero."),
            classification: sample_classification(PdfTtsMode::HighTextTrust, 0.9),
            sentences: vec![SentencePlan {
                id: 42,
                text: "Sentence zero.".into(),
                range: TextRange { start: 0, end: 14 },
                page_range: PageRange {
                    start_page: 0,
                    end_page: 0,
                },
            }],
            pages: Vec::new(),
            stats: NormalizationStats::default(),
            artifact_path: None,
        };

        let clip =
            prepare_sentence_clip(&config, &analysis, 0, TtsEngineKind::TonePreview).unwrap();
        assert!(clip.manifest_path.exists());
        assert!(clip.audio_path.as_ref().is_some_and(|path| path.exists()));
    }

    #[test]
    fn sync_target_exact_match_collects_rects() {
        let segments = vec![
            TextSegmentData {
                text: "Hello".into(),
                rect: PdfRectData {
                    bottom: 10.0,
                    left: 10.0,
                    top: 20.0,
                    right: 40.0,
                },
            },
            TextSegmentData {
                text: "world".into(),
                rect: PdfRectData {
                    bottom: 10.0,
                    left: 42.0,
                    top: 20.0,
                    right: 70.0,
                },
            },
        ];

        let target = sync_target_for_page(
            0,
            0,
            42,
            &build_sync_tokens("hello world"),
            "hello world",
            &segments,
        );

        assert_eq!(target.confidence, SentenceSyncConfidence::ExactSentence);
        assert_eq!(target.rects.len(), 2);
    }

    #[test]
    fn sync_target_degrades_to_block_fallback() {
        let segments = vec![
            TextSegmentData {
                text: "The quick brown".into(),
                rect: PdfRectData {
                    bottom: 10.0,
                    left: 10.0,
                    top: 20.0,
                    right: 80.0,
                },
            },
            TextSegmentData {
                text: "fox jumps".into(),
                rect: PdfRectData {
                    bottom: 24.0,
                    left: 10.0,
                    top: 34.0,
                    right: 65.0,
                },
            },
        ];

        let target = sync_target_for_page(
            0,
            0,
            42,
            &build_sync_tokens("quick fox leaps"),
            "quick fox leaps",
            &segments,
        );

        assert!(matches!(
            target.confidence,
            SentenceSyncConfidence::BlockFallback | SentenceSyncConfidence::FuzzySentence
        ));
        assert_eq!(target.page_index, Some(0));
    }

    #[test]
    fn sentence_budget_window_keeps_local_radius() {
        let window = sentence_budget_window(5, 12, 2);
        assert_eq!(window, HashSet::from([3, 4, 5, 6, 7]));

        let near_start = sentence_budget_window(0, 3, 2);
        assert_eq!(near_start, HashSet::from([0, 1, 2]));
    }

    #[test]
    fn deferred_ocr_policy_degrades_to_page_follow() {
        let mut config = AppConfig::default();
        config.tts.ocr_policy = TtsOcrPolicy::Deferred;
        let analysis = TtsAnalysisArtifacts {
            source_path: PathBuf::from("fixture.pdf"),
            source_fingerprint: "abc".into(),
            generated_at_unix_secs: 0,
            mode: PdfTtsMode::OcrRequired,
            confidence: 0.2,
            tts_text: "Scanned fallback".into(),
            canonical_text: sample_canonical_text("Scanned fallback"),
            classification: sample_classification(PdfTtsMode::OcrRequired, 0.2),
            sentences: vec![SentencePlan {
                id: 1,
                text: "Scanned fallback".into(),
                range: TextRange { start: 0, end: 16 },
                page_range: PageRange {
                    start_page: 0,
                    end_page: 0,
                },
            }],
            pages: Vec::new(),
            stats: NormalizationStats::default(),
            artifact_path: None,
        };

        let policy = evaluate_runtime_policy(&config, &analysis);
        assert!(policy.allow_playback);
        assert!(!policy.allow_rect_highlights);
        assert!(!policy.allow_sync_prefetch);
        assert_eq!(
            policy.max_sync_confidence,
            SentenceSyncConfidence::PageFallback
        );

        let adapted = apply_runtime_policy(
            &SentenceSyncTarget {
                sentence_index: 0,
                sentence_id: 1,
                confidence: SentenceSyncConfidence::ExactSentence,
                page_index: Some(0),
                rects: vec![PdfRectData {
                    left: 1.0,
                    right: 2.0,
                    top: 3.0,
                    bottom: 0.5,
                }],
                fallback_reason: "exact_substring_match".into(),
                score: 1.0,
            },
            &policy,
        );
        assert_eq!(adapted.confidence, SentenceSyncConfidence::PageFallback);
        assert!(adapted.rects.is_empty());
    }

    #[test]
    fn disabled_ocr_policy_blocks_playback() {
        let mut config = AppConfig::default();
        config.tts.ocr_policy = TtsOcrPolicy::Disabled;
        let analysis = TtsAnalysisArtifacts {
            source_path: PathBuf::from("fixture.pdf"),
            source_fingerprint: "abc".into(),
            generated_at_unix_secs: 0,
            mode: PdfTtsMode::OcrRequired,
            confidence: 0.1,
            tts_text: String::new(),
            canonical_text: sample_canonical_text(""),
            classification: sample_classification(PdfTtsMode::OcrRequired, 0.1),
            sentences: Vec::new(),
            pages: Vec::new(),
            stats: NormalizationStats::default(),
            artifact_path: None,
        };

        let policy = evaluate_runtime_policy(&config, &analysis);
        assert!(!policy.allow_playback);
        assert_eq!(policy.max_sync_confidence, SentenceSyncConfidence::Missing);
    }
}
