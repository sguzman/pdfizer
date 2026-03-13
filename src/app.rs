use std::{
    collections::HashMap,
    fs,
    path::PathBuf,
    sync::mpsc::{self, Receiver, Sender, TryRecvError},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context as _, Result};
use eframe::egui::{
    self, Color32, ColorImage, Key, Label, Pos2, Rect, RichText, ScrollArea, Sense, Slider,
    TextureHandle, TextureOptions, Ui, UiBuilder, Vec2,
};
use rfd::FileDialog;
use rodio::{Decoder, DeviceSinkBuilder, MixerDeviceSink, Player};
use serde::{Deserialize, Serialize};
use tracing::{debug, error, info, instrument, warn};

use crate::{
    config::AppConfig,
    pdf::{
        PageSizePoints, PdfDocument, PdfMetadata, PdfRectData, PdfRuntime, RenderMode,
        RenderPreset, RenderRequest, RenderedPageImage, SearchHit, TextSegmentData,
        TileRenderRequest,
    },
    tts::{
        self, PdfTtsMode, PreparedSentenceClip, SentenceSyncConfidence, SentenceSyncTarget,
        TtsAnalysisArtifacts, TtsEngineKind, TtsPlaybackState, TtsWorkerMessage,
    },
};

pub struct PdfizerApp {
    config: AppConfig,
    runtime: Option<PdfRuntime>,
    runtime_error: Option<String>,
    document: Option<PdfDocument<'static>>,
    last_error: Option<String>,
    current_page: usize,
    zoom: f32,
    view_mode: ViewMode,
    current_preset: RenderPreset,
    compare_enabled: bool,
    compare_preset: RenderPreset,
    primary_view: Option<RenderView>,
    compare_view: Option<RenderView>,
    render_cache: HashMap<RenderCacheKey, CachedRender>,
    thumbnail_cache: HashMap<ThumbnailCacheKey, CachedRender>,
    primary_tile_job: Option<TiledRenderJob>,
    compare_tile_job: Option<TiledRenderJob>,
    render_history: Vec<RenderMetric>,
    config_preview: String,
    config_editor: String,
    status_message: Option<String>,
    pixel_sample: Option<PixelSample>,
    search_query: String,
    search_match_case: bool,
    search_whole_word: bool,
    search_results: Vec<SearchHit>,
    active_search_result: Option<usize>,
    single_scroll_offset: Vec2,
    continuous_scroll_offset: Vec2,
    highlight_text: bool,
    text_rect_cache: HashMap<usize, Vec<PdfRectData>>,
    text_segment_cache: HashMap<usize, Vec<TextSegmentData>>,
    current_document_path: Option<PathBuf>,
    tts_analysis: Option<TtsAnalysisArtifacts>,
    tts_analysis_status: TtsAnalysisStatus,
    tts_policy: Option<tts::TtsRuntimePolicy>,
    pending_tts_sentence_id: Option<u64>,
    tts_worker_tx: Option<Sender<TtsWorkerMessage>>,
    tts_worker_rx: Option<Receiver<TtsWorkerMessage>>,
    tts_request_id: u64,
    tts_engine: TtsEngineKind,
    tts_playback_state: TtsPlaybackState,
    tts_active_sentence_index: usize,
    tts_follow_mode: bool,
    tts_follow_pin_to_center: bool,
    tts_highlights_enabled: bool,
    tts_experimental_sync_enabled: bool,
    tts_verbose_degraded_logging_enabled: bool,
    tts_prepared_clips: HashMap<usize, PreparedSentenceClip>,
    tts_sync_targets: HashMap<usize, SentenceSyncTarget>,
    tts_prefetch_queue: Vec<usize>,
    tts_sync_queue: Vec<usize>,
    tts_failed_prefetch: HashMap<usize, String>,
    tts_failed_sync: HashMap<usize, String>,
    tts_prefetch_request_id: u64,
    tts_cancel_token: u64,
    tts_prefetch_in_flight: usize,
    tts_started_at: Option<Instant>,
    tts_activation_requested_at: Option<Instant>,
    tts_elapsed_before_pause: Duration,
    tts_current_duration: Duration,
    tts_playback_tx: Option<Sender<PlaybackCommand>>,
    tts_playback_rx: Option<Receiver<PlaybackEvent>>,
    tts_playback_command_id: u64,
    single_viewport: ScrollViewport,
    continuous_viewport: ScrollViewport,
    tts_profile: TtsPerformanceProfile,
}

#[derive(Debug, Clone)]
enum PlaybackCommand {
    Play {
        command_id: u64,
        cancel_token: u64,
        audio_path: Option<PathBuf>,
        volume: f32,
        rate: f32,
    },
    Pause {
        command_id: u64,
        cancel_token: u64,
    },
    Resume {
        command_id: u64,
        cancel_token: u64,
    },
    Stop {
        command_id: u64,
        cancel_token: u64,
    },
    Shutdown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PlaybackWorkerState {
    Playing,
    Paused,
    Stopped,
}

#[derive(Debug, Clone)]
enum PlaybackEvent {
    Ack {
        command_id: u64,
        cancel_token: u64,
        state: PlaybackWorkerState,
    },
    Failed {
        command_id: u64,
        cancel_token: u64,
        error: String,
    },
}

#[derive(Debug, Clone, Copy)]
enum TtsAnalysisRequest {
    Windowed,
    FullDocument,
}

#[derive(Debug, Clone, Copy, Default)]
struct ScrollViewport {
    offset: Vec2,
    size: Vec2,
    content_size: Vec2,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TtsHighlightKind {
    ExactRects,
    LineFallback,
    BlockFallback,
    PageFallback,
}

impl PdfizerApp {
    fn ensure_tts_worker_channel(&mut self) -> Sender<TtsWorkerMessage> {
        if let Some(sender) = &self.tts_worker_tx {
            return sender.clone();
        }

        let (tx, rx) = mpsc::channel();
        self.tts_worker_tx = Some(tx.clone());
        self.tts_worker_rx = Some(rx);
        tx
    }

    fn ensure_playback_worker(&mut self) -> Sender<PlaybackCommand> {
        if let Some(sender) = &self.tts_playback_tx {
            return sender.clone();
        }

        let (command_tx, command_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();
        thread::spawn(move || playback_worker_loop(command_rx, event_tx));
        self.tts_playback_tx = Some(command_tx.clone());
        self.tts_playback_rx = Some(event_rx);
        command_tx
    }

    fn next_playback_command_id(&mut self) -> u64 {
        self.tts_playback_command_id = self.tts_playback_command_id.wrapping_add(1);
        self.tts_playback_command_id
    }

    fn dispatch_playback_command(&mut self, command: PlaybackCommand) {
        let sender = self.ensure_playback_worker();
        if let Err(err) = sender.send(command) {
            self.last_error = Some(format!("failed to send playback command: {err}"));
            self.tts_profile.playback_worker_failures += 1;
            self.tts_playback_state = TtsPlaybackState::Failed;
        }
    }

    fn cancel_tts_work(&mut self, reason: &str) -> u64 {
        self.tts_cancel_token = self.tts_cancel_token.wrapping_add(1);
        info!(
            cancel_token = self.tts_cancel_token,
            reason, "cancelled active TTS jobs"
        );
        self.tts_cancel_token
    }

    pub fn new(cc: &eframe::CreationContext<'_>, config: AppConfig) -> Self {
        let tts_enabled = config.tts.enabled;
        let tts_engine = TtsEngineKind::from_name(&config.tts.engine);
        let tts_follow_pin_to_center = config.tts.follow_center_on_target;
        let tts_experimental_sync_enabled = config.tts.experimental_pdf_sync;
        let tts_verbose_degraded_logging_enabled = config.tts.verbose_degraded_logging;
        let runtime = match PdfRuntime::new(&config) {
            Ok(runtime) => Some(runtime),
            Err(err) => {
                error!(error = %err, "failed to initialize Pdfium runtime");
                None
            }
        };

        let runtime_error = runtime
            .as_ref()
            .map(|_| None)
            .unwrap_or_else(|| Some(pdfium_help_text(&config)));

        let mut app = Self {
            zoom: config.rendering.initial_zoom,
            view_mode: ViewMode::SinglePage,
            current_preset: RenderPreset::from_name(&config.rendering.default_preset),
            compare_enabled: config.ui.compare_mode_default,
            compare_preset: config
                .rendering
                .compare_presets
                .get(1)
                .map(|name| RenderPreset::from_name(name))
                .unwrap_or(RenderPreset::Crisp),
            config_preview: config.config_preview(),
            config_editor: config.config_preview(),
            config,
            runtime,
            runtime_error,
            document: None,
            last_error: None,
            current_page: 0,
            primary_view: None,
            compare_view: None,
            render_cache: HashMap::new(),
            thumbnail_cache: HashMap::new(),
            primary_tile_job: None,
            compare_tile_job: None,
            render_history: Vec::new(),
            status_message: None,
            pixel_sample: None,
            search_query: String::new(),
            search_match_case: false,
            search_whole_word: false,
            search_results: Vec::new(),
            active_search_result: None,
            single_scroll_offset: Vec2::ZERO,
            continuous_scroll_offset: Vec2::ZERO,
            highlight_text: false,
            text_rect_cache: HashMap::new(),
            text_segment_cache: HashMap::new(),
            current_document_path: None,
            tts_analysis: None,
            tts_analysis_status: if tts_enabled {
                TtsAnalysisStatus::Idle
            } else {
                TtsAnalysisStatus::Disabled
            },
            tts_policy: None,
            pending_tts_sentence_id: None,
            tts_worker_tx: None,
            tts_worker_rx: None,
            tts_request_id: 0,
            tts_engine,
            tts_playback_state: TtsPlaybackState::Stopped,
            tts_active_sentence_index: 0,
            tts_follow_mode: true,
            tts_follow_pin_to_center,
            tts_highlights_enabled: true,
            tts_experimental_sync_enabled,
            tts_verbose_degraded_logging_enabled,
            tts_prepared_clips: HashMap::new(),
            tts_sync_targets: HashMap::new(),
            tts_prefetch_queue: Vec::new(),
            tts_sync_queue: Vec::new(),
            tts_failed_prefetch: HashMap::new(),
            tts_failed_sync: HashMap::new(),
            tts_prefetch_request_id: 0,
            tts_cancel_token: 0,
            tts_prefetch_in_flight: 0,
            tts_started_at: None,
            tts_activation_requested_at: None,
            tts_elapsed_before_pause: Duration::ZERO,
            tts_current_duration: Duration::ZERO,
            tts_playback_tx: None,
            tts_playback_rx: None,
            tts_playback_command_id: 0,
            single_viewport: ScrollViewport::default(),
            continuous_viewport: ScrollViewport::default(),
            tts_profile: TtsPerformanceProfile::default(),
        };

        app.restore_session(&cc.egui_ctx);
        app
    }

    #[instrument(skip(self, ctx))]
    fn open_pdf(&mut self, ctx: &egui::Context, path: PathBuf) {
        if let Err(err) = self.ensure_runtime() {
            self.last_error = Some(err.to_string());
            return;
        }

        let Some(runtime) = &self.runtime else {
            self.last_error = Some("Pdfium runtime is unavailable".into());
            return;
        };

        match runtime.open_document(&path) {
            Ok(document) => {
                info!(path = %path.display(), pages = document.metadata.page_count, "opened PDF");
                self.document = Some(document);
                self.current_document_path = Some(path.clone());
                self.current_page = 0;
                self.last_error = None;
                self.primary_view = None;
                self.compare_view = None;
                self.primary_tile_job = None;
                self.compare_tile_job = None;
                self.pixel_sample = None;
                self.search_results.clear();
                self.active_search_result = None;
                self.text_rect_cache.clear();
                self.text_segment_cache.clear();
                self.tts_analysis = None;
                self.tts_analysis_status = if self.config.tts.enabled {
                    TtsAnalysisStatus::Idle
                } else {
                    TtsAnalysisStatus::Disabled
                };
                self.tts_policy = None;
                self.pending_tts_sentence_id = None;
                self.reset_tts_runtime();
                self.single_scroll_offset = Vec2::ZERO;
                self.continuous_scroll_offset = Vec2::ZERO;
                self.render_current_page(ctx);
                self.start_tts_analysis(path, TtsAnalysisRequest::Windowed);
                self.persist_session();
            }
            Err(err) => {
                error!(path = %path.display(), error = %err, "failed to open PDF");
                self.last_error = Some(err.to_string());
            }
        }
    }

    fn start_tts_analysis(&mut self, path: PathBuf, request: TtsAnalysisRequest) {
        if !self.config.tts.enabled {
            self.tts_analysis_status = TtsAnalysisStatus::Disabled;
            return;
        }

        if !self.config.tts.auto_analyze_on_open {
            self.tts_analysis_status = TtsAnalysisStatus::Idle;
            return;
        }

        self.tts_request_id = self.tts_request_id.wrapping_add(1);
        let request_id = self.tts_request_id;
        let config = self.config.clone();
        let tx = self.ensure_tts_worker_channel();
        self.tts_analysis_status = TtsAnalysisStatus::Analyzing;
        let analysis_scope = self.planned_tts_analysis_scope(request);

        thread::spawn(move || {
            let analysis_result = match analysis_scope {
                Some((start_page, end_page)) => {
                    tts::analyze_pdf_for_tts_in_scope(&config, &path, start_page, end_page)
                }
                None => tts::analyze_pdf_for_tts(&config, &path),
            };
            let message = match analysis_result {
                Ok(artifacts) => TtsWorkerMessage::Completed {
                    request_id,
                    artifacts,
                },
                Err(err) => TtsWorkerMessage::Failed {
                    request_id,
                    error: err.to_string(),
                },
            };
            let _ = tx.send(message);
        });
    }

    fn rebuild_tts_analysis(&mut self) {
        let Some(path) = self.current_document_path.clone() else {
            self.last_error = Some("open a PDF before rebuilding TTS analysis".into());
            return;
        };
        self.cancel_tts_work("rebuild_analysis");
        self.start_tts_analysis(path, TtsAnalysisRequest::Windowed);
        self.status_message = Some("Rebuilding TTS analysis".into());
    }

    fn rebuild_tts_analysis_full(&mut self) {
        let Some(path) = self.current_document_path.clone() else {
            self.last_error = Some("open a PDF before rebuilding TTS analysis".into());
            return;
        };
        self.cancel_tts_work("rebuild_analysis_full");
        self.start_tts_analysis(path, TtsAnalysisRequest::FullDocument);
        self.status_message = Some("Rebuilding full-document TTS analysis".into());
    }

    fn planned_tts_analysis_scope(&self, request: TtsAnalysisRequest) -> Option<(usize, usize)> {
        let document = self.document.as_ref()?;
        if matches!(request, TtsAnalysisRequest::FullDocument) {
            return None;
        }

        let page_count = document.metadata.page_count;
        let max_pages = self.config.tts.analysis_max_pages.max(1);
        if page_count <= max_pages {
            return None;
        }

        let radius = self.config.tts.analysis_window_radius.max(max_pages / 2);
        let mut start = self.current_page.saturating_sub(radius);
        let mut end = (self.current_page + radius).min(page_count.saturating_sub(1));
        let window_len = end - start + 1;
        if window_len > max_pages {
            end = start + max_pages - 1;
        } else if window_len < max_pages {
            let missing = max_pages - window_len;
            start = start.saturating_sub(missing / 2);
            end = (start + max_pages - 1).min(page_count.saturating_sub(1));
            start = end.saturating_sub(max_pages - 1);
        }

        Some((start, end))
    }

    fn poll_tts_analysis(&mut self) {
        let Some(receiver) = self.tts_worker_rx.take() else {
            return;
        };

        loop {
            match receiver.try_recv() {
                Ok(TtsWorkerMessage::Completed {
                    request_id,
                    artifacts,
                }) if request_id == self.tts_request_id => {
                    let policy = tts::evaluate_runtime_policy(&self.config, &artifacts);
                    if artifacts.mode != PdfTtsMode::HighTextTrust
                        && self.tts_verbose_degraded_logging_enabled
                    {
                        warn!(
                            path = %artifacts.source_path.display(),
                            mode = %artifacts.mode.label(),
                            confidence = artifacts.confidence,
                            "PDF TTS analysis completed in degraded mode"
                        );
                    }
                    self.status_message = Some(format!(
                        "TTS analysis ready: {} sentences ({}, {}, pages {}-{}{})",
                        artifacts.sentences.len(),
                        artifacts.mode.label(),
                        policy.reason,
                        artifacts.analysis_scope.start_page + 1,
                        artifacts.analysis_scope.end_page + 1,
                        if artifacts.analysis_scope.full_document {
                            ""
                        } else {
                            ", windowed"
                        }
                    ));
                    self.tts_policy = Some(policy);
                    self.tts_analysis = Some(artifacts);
                    self.tts_analysis_status = TtsAnalysisStatus::Ready;
                    if let Some(sentence_id) = self.pending_tts_sentence_id.take() {
                        if let Some(analysis) = &self.tts_analysis {
                            if let Some(index) = tts::sentence_index_for_id(analysis, sentence_id) {
                                self.tts_active_sentence_index = index;
                            } else {
                                self.sync_tts_sentence_to_current_page();
                            }
                        }
                    } else {
                        self.sync_tts_sentence_to_current_page();
                    }
                }
                Ok(TtsWorkerMessage::Failed { request_id, error })
                    if request_id == self.tts_request_id =>
                {
                    error!(error = %error, "TTS analysis failed");
                    self.last_error = Some(format!("TTS analysis failed: {error}"));
                    self.tts_analysis_status = TtsAnalysisStatus::Failed(error);
                }
                Ok(TtsWorkerMessage::PrefetchCompleted {
                    request_id,
                    cancel_token,
                    clip,
                    elapsed_ms,
                }) if request_id == self.tts_prefetch_request_id
                    && cancel_token == self.tts_cancel_token =>
                {
                    self.tts_profile.prepare.record(elapsed_ms as f64);
                    if clip.cache_hit {
                        self.tts_profile.prepare_cache_hits += 1;
                    }
                    debug!(
                        sentence_index = clip.sentence_index,
                        cache_hit = clip.cache_hit,
                        queued_remaining = self.tts_prefetch_queue.len().saturating_sub(1),
                        in_flight_before = self.tts_prefetch_in_flight,
                        prepare_cache_hits = self.tts_profile.prepare_cache_hits,
                        elapsed_ms,
                        "processed prepared TTS clip"
                    );
                    self.tts_prepared_clips.insert(clip.sentence_index, clip);
                    self.tts_prefetch_queue
                        .retain(|index| !self.tts_prepared_clips.contains_key(index));
                    self.tts_prefetch_in_flight = self.tts_prefetch_in_flight.saturating_sub(1);
                    self.enforce_tts_runtime_budget();
                }
                Ok(TtsWorkerMessage::PrefetchFailed {
                    request_id,
                    cancel_token,
                    sentence_index,
                    error,
                    elapsed_ms,
                }) if request_id == self.tts_prefetch_request_id
                    && cancel_token == self.tts_cancel_token =>
                {
                    self.tts_profile.prepare.record(elapsed_ms as f64);
                    debug!(
                        sentence_index,
                        queued_remaining = self.tts_prefetch_queue.len().saturating_sub(1),
                        in_flight_before = self.tts_prefetch_in_flight,
                        elapsed_ms,
                        error = %error,
                        "processed failed TTS clip preparation"
                    );
                    self.tts_prefetch_queue
                        .retain(|index| *index != sentence_index);
                    self.tts_prefetch_in_flight = self.tts_prefetch_in_flight.saturating_sub(1);
                    self.tts_failed_prefetch
                        .insert(sentence_index, error.clone());
                    warn!(sentence_index, error = %error, "TTS prefetch failed");
                }
                Ok(TtsWorkerMessage::SyncCompleted {
                    request_id,
                    cancel_token,
                    target,
                    elapsed_ms,
                }) if request_id == self.tts_prefetch_request_id
                    && cancel_token == self.tts_cancel_token =>
                {
                    self.tts_profile.sync.record(elapsed_ms as f64);
                    self.tts_profile.record_sync_confidence(target.confidence);
                    self.record_sync_failure_counters(&target);
                    debug!(
                        sentence_index = target.sentence_index,
                        sentence_id = target.sentence_id,
                        confidence = %target.confidence.label(),
                        score = target.score,
                        text_similarity = target.score_breakdown.text_similarity,
                        reading_order = target.score_breakdown.reading_order,
                        geometry_compactness = target.score_breakdown.geometry_compactness,
                        page_continuity = target.score_breakdown.page_continuity,
                        rects = target.rects.len(),
                        fallback_reason = %target.fallback_reason,
                        "computed TTS sync mapping summary"
                    );
                    self.tts_sync_targets.insert(target.sentence_index, target);
                    self.tts_sync_queue
                        .retain(|index| !self.tts_sync_targets.contains_key(index));
                    self.enforce_tts_runtime_budget();
                }
                Ok(TtsWorkerMessage::SyncFailed {
                    request_id,
                    cancel_token,
                    sentence_index,
                    error,
                    elapsed_ms,
                }) if request_id == self.tts_prefetch_request_id
                    && cancel_token == self.tts_cancel_token =>
                {
                    self.tts_profile.sync.record(elapsed_ms as f64);
                    self.tts_sync_queue.retain(|index| *index != sentence_index);
                    self.tts_failed_sync.insert(sentence_index, error.clone());
                    warn!(sentence_index, error = %error, "TTS sync computation failed");
                }
                Ok(_) => {
                    self.tts_profile.stale_worker_results += 1;
                    self.tts_profile.cancelled_worker_results += 1;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.tts_analysis_status = TtsAnalysisStatus::Failed(
                        "analysis worker disconnected unexpectedly".into(),
                    );
                    self.tts_worker_tx = None;
                    break;
                }
            }
        }

        self.tts_worker_rx = Some(receiver);
    }

    fn reset_tts_runtime(&mut self) {
        self.cancel_tts_work("reset_runtime");
        self.tts_playback_state = TtsPlaybackState::Stopped;
        self.tts_active_sentence_index = 0;
        self.tts_prepared_clips.clear();
        self.tts_sync_targets.clear();
        self.tts_prefetch_queue.clear();
        self.tts_sync_queue.clear();
        self.tts_failed_prefetch.clear();
        self.tts_failed_sync.clear();
        self.tts_prefetch_in_flight = 0;
        self.tts_started_at = None;
        self.tts_activation_requested_at = None;
        self.tts_elapsed_before_pause = Duration::ZERO;
        self.tts_current_duration = Duration::ZERO;
        self.tts_prefetch_request_id = self.tts_prefetch_request_id.wrapping_add(1);
        self.tts_profile = TtsPerformanceProfile::default();
    }

    fn poll_playback_events(&mut self) {
        let Some(receiver) = self.tts_playback_rx.take() else {
            return;
        };

        loop {
            match receiver.try_recv() {
                Ok(PlaybackEvent::Ack {
                    command_id,
                    cancel_token,
                    state,
                }) if cancel_token == self.tts_cancel_token => {
                    debug!(
                        command_id,
                        cancel_token,
                        state = ?state,
                        "processed playback worker ack"
                    );
                }
                Ok(PlaybackEvent::Failed {
                    command_id,
                    cancel_token,
                    error,
                }) if cancel_token == self.tts_cancel_token => {
                    error!(
                        command_id,
                        cancel_token,
                        error = %error,
                        "playback worker command failed"
                    );
                    self.tts_profile.playback_worker_failures += 1;
                    self.tts_playback_state = TtsPlaybackState::Failed;
                    self.last_error = Some(format!("TTS playback failed: {error}"));
                }
                Ok(_) => {
                    self.tts_profile.cancelled_playback_events += 1;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.tts_playback_tx = None;
                    self.tts_profile.playback_worker_failures += 1;
                    self.tts_playback_state = TtsPlaybackState::Failed;
                    self.last_error = Some("TTS playback worker disconnected unexpectedly".into());
                    break;
                }
            }
        }

        self.tts_playback_rx = Some(receiver);
    }

    fn record_sync_failure_counters(&mut self, target: &SentenceSyncTarget) {
        let wrong_page = self
            .tts_analysis
            .as_ref()
            .and_then(|analysis| analysis.sentences.get(target.sentence_index))
            .and_then(|sentence| {
                target.page_index.map(|page_index| {
                    page_index < sentence.page_range.start_page
                        || page_index > sentence.page_range.end_page
                })
            })
            .unwrap_or(false);
        let distant_geometry = target.fallback_reason.contains("visually_implausible")
            || target
                .fallback_reason
                .contains("rejected_visually_implausible_match");
        let unmappable = target.confidence == SentenceSyncConfidence::Missing
            || target.fallback_reason.contains("no_page_candidate")
            || target.fallback_reason.contains("page_location_only");

        self.tts_profile
            .record_sync_failure(wrong_page, distant_geometry, unmappable);
    }

    fn active_sentence(&self) -> Option<&crate::tts::SentencePlan> {
        self.tts_analysis
            .as_ref()
            .and_then(|analysis| analysis.sentences.get(self.tts_active_sentence_index))
    }

    fn sentence_index_for_page(&self, page_index: usize) -> Option<usize> {
        let analysis = self.tts_analysis.as_ref()?;
        analysis
            .sentences
            .iter()
            .enumerate()
            .find(|(_, sentence)| {
                sentence.page_range.start_page <= page_index
                    && sentence.page_range.end_page >= page_index
            })
            .map(|(index, _)| index)
    }

    fn set_tts_cursor_to_sentence_index(&mut self, sentence_index: usize) {
        let Some(analysis) = &self.tts_analysis else {
            return;
        };
        if sentence_index >= analysis.sentences.len() {
            return;
        }
        self.tts_active_sentence_index = sentence_index;
        self.persist_session();
    }

    fn set_tts_cursor_to_nearest_sentence(
        &mut self,
        page_index: usize,
        focus_rect: Option<PdfRectData>,
    ) -> Option<usize> {
        let sentence_index = self
            .sentence_index_for_location(page_index, focus_rect)
            .or_else(|| self.sentence_index_for_page(page_index))?;
        self.set_tts_cursor_to_sentence_index(sentence_index);
        Some(sentence_index)
    }

    fn sentence_index_for_location(
        &self,
        page_index: usize,
        focus_rect: Option<PdfRectData>,
    ) -> Option<usize> {
        let analysis = self.tts_analysis.as_ref()?;
        let focus_rect = focus_rect?;

        let mut best: Option<(usize, f32)> = None;
        for (sentence_index, sentence) in analysis.sentences.iter().enumerate() {
            if sentence.page_range.start_page > page_index
                || sentence.page_range.end_page < page_index
            {
                continue;
            }

            let Some(target) = self.effective_sync_target(sentence_index) else {
                continue;
            };
            if target.page_index != Some(page_index) || target.rects.is_empty() {
                continue;
            }

            let distance = target
                .rects
                .iter()
                .map(|rect| rect_distance_score(*rect, focus_rect))
                .fold(f32::INFINITY, f32::min);

            match best {
                Some((_, best_distance)) if distance >= best_distance => {}
                _ => best = Some((sentence_index, distance)),
            }
        }

        best.map(|(sentence_index, _)| sentence_index)
    }

    fn trace_active_sentence_origin(&self, action: &str) {
        let Some(analysis) = &self.tts_analysis else {
            return;
        };
        let Some(sentence) = self.active_sentence() else {
            return;
        };
        info!(
            action,
            source_fingerprint = %analysis.source_fingerprint,
            sentence_id = sentence.id,
            sentence_index = self.tts_active_sentence_index,
            text_range_start = sentence.range.start,
            text_range_end = sentence.range.end,
            page_start = sentence.page_range.start_page,
            page_end = sentence.page_range.end_page,
            canonical_blocks = analysis.canonical_text.block_count,
            canonical_lines = analysis.canonical_text.line_count,
            canonical_tokens = analysis.canonical_text.token_count,
            "resolved playback step from canonical PDF TTS text"
        );
    }

    fn active_tts_policy(&self) -> Option<&tts::TtsRuntimePolicy> {
        self.tts_policy.as_ref()
    }

    fn effective_sync_target(&self, sentence_index: usize) -> Option<SentenceSyncTarget> {
        let target = self.tts_sync_targets.get(&sentence_index)?;
        let policy = self.active_tts_policy()?;
        let adapted = tts::apply_runtime_policy(target, policy);
        Some(self.apply_local_sync_controls(adapted))
    }

    fn apply_local_sync_controls(&self, target: SentenceSyncTarget) -> SentenceSyncTarget {
        let mut adapted = target;

        if !self.tts_experimental_sync_enabled
            && adapted.confidence == SentenceSyncConfidence::FuzzySentence
        {
            adapted.confidence = SentenceSyncConfidence::BlockFallback;
            adapted.fallback_reason = format!(
                "{}; experimental_fuzzy_sentence_sync_disabled",
                adapted.fallback_reason
            );
        }

        loop {
            let threshold = match adapted.confidence {
                SentenceSyncConfidence::ExactSentence => self.config.tts.exact_sync_min_score,
                SentenceSyncConfidence::FuzzySentence => self.config.tts.fuzzy_sync_min_score,
                SentenceSyncConfidence::BlockFallback => self.config.tts.block_sync_min_score,
                SentenceSyncConfidence::PageFallback | SentenceSyncConfidence::Missing => 0.0,
            };

            if adapted.score >= threshold {
                break;
            }

            let prior = adapted.confidence;
            match adapted.confidence {
                SentenceSyncConfidence::ExactSentence => {
                    adapted.confidence = SentenceSyncConfidence::FuzzySentence;
                }
                SentenceSyncConfidence::FuzzySentence => {
                    adapted.confidence = SentenceSyncConfidence::BlockFallback;
                }
                SentenceSyncConfidence::BlockFallback => {
                    adapted.confidence = SentenceSyncConfidence::PageFallback;
                    adapted.rects.clear();
                    adapted.lineage.clear();
                }
                SentenceSyncConfidence::PageFallback | SentenceSyncConfidence::Missing => break,
            }

            adapted.fallback_reason = format!(
                "{}; {} score_{:.2}_below_threshold_{:.2}",
                adapted.fallback_reason,
                prior.label(),
                adapted.score,
                threshold
            );
        }

        adapted
    }

    fn sync_tts_sentence_to_current_page(&mut self) {
        let Some(analysis) = &self.tts_analysis else {
            return;
        };
        if let Some((index, _)) = analysis.sentences.iter().enumerate().find(|(_, sentence)| {
            sentence.page_range.start_page <= self.current_page
                && sentence.page_range.end_page >= self.current_page
        }) {
            self.set_tts_cursor_to_sentence_index(index);
        }
    }

    fn tts_viewport_focus_sentence_index(&self) -> Option<usize> {
        let analysis = self.tts_analysis.as_ref()?;
        analysis
            .sentences
            .iter()
            .enumerate()
            .find(|(_, sentence)| {
                sentence.page_range.start_page <= self.current_page
                    && sentence.page_range.end_page >= self.current_page
            })
            .map(|(index, _)| index)
            .or_else(|| {
                if analysis.sentences.is_empty() {
                    None
                } else {
                    Some(
                        self.tts_active_sentence_index
                            .min(analysis.sentences.len() - 1),
                    )
                }
            })
    }

    fn enforce_tts_runtime_budget(&mut self) {
        let Some(analysis) = &self.tts_analysis else {
            return;
        };
        let Some(focus_index) = self.tts_viewport_focus_sentence_index() else {
            return;
        };

        let clip_keep = tts::sentence_budget_window(
            focus_index,
            analysis.sentences.len(),
            self.config.tts.clip_budget_sentences,
        );
        let sync_keep = tts::sentence_budget_window(
            focus_index,
            analysis.sentences.len(),
            self.config.tts.sync_budget_sentences,
        );

        let clip_before = self.tts_prepared_clips.len();
        let sync_before = self.tts_sync_targets.len();

        self.tts_prepared_clips
            .retain(|index, _| clip_keep.contains(index));
        self.tts_prefetch_queue
            .retain(|index| clip_keep.contains(index));
        self.tts_failed_prefetch
            .retain(|index, _| clip_keep.contains(index));

        self.tts_sync_targets
            .retain(|index, _| sync_keep.contains(index));
        self.tts_sync_queue
            .retain(|index| sync_keep.contains(index));
        self.tts_failed_sync
            .retain(|index, _| sync_keep.contains(index));

        if self.tts_prepared_clips.len() != clip_before
            || self.tts_sync_targets.len() != sync_before
        {
            debug!(
                focus_index,
                clip_before,
                clip_after = self.tts_prepared_clips.len(),
                sync_before,
                sync_after = self.tts_sync_targets.len(),
                "trimmed TTS runtime state to viewport-local budget"
            );
        }
    }

    fn start_tts_prefetch(&mut self) {
        let Some(analysis) = self.tts_analysis.clone() else {
            return;
        };
        let Some(policy) = self.tts_policy.clone() else {
            return;
        };

        let cancel_token = self.cancel_tts_work("prefetch_restart");
        self.tts_prefetch_request_id = self.tts_prefetch_request_id.wrapping_add(1);
        let request_id = self.tts_prefetch_request_id;
        let settings = tts::TtsSynthesisSettings::from_config(&self.config);
        let plan = tts::build_prefetch_plan_with_budget(
            &analysis,
            self.tts_active_sentence_index,
            self.config.tts.sentence_prefetch,
            self.config.tts.prefetch_duration_budget_ms,
            &settings,
        );

        self.tts_prefetch_queue = plan
            .sentence_indexes
            .iter()
            .copied()
            .filter(|index| !self.tts_prepared_clips.contains_key(index))
            .collect();
        self.tts_failed_prefetch.clear();
        self.tts_prefetch_in_flight = self.tts_prefetch_queue.len();
        self.enforce_tts_runtime_budget();
        self.tts_profile.scheduler_queue_peak = self
            .tts_profile
            .scheduler_queue_peak
            .max((self.tts_prefetch_queue.len() + self.tts_sync_queue.len()) as u64);
        debug!(
            request_id,
            cancel_token,
            active_sentence = self.tts_active_sentence_index,
            queued = self.tts_prefetch_queue.len(),
            duration_budget_ms = self.config.tts.prefetch_duration_budget_ms,
            estimated_duration_ms_total = plan.estimated_duration_ms_total,
            "planned TTS clip prefetch window"
        );
        if self.primary_tile_job.is_some() || self.compare_tile_job.is_some() {
            self.tts_profile.starvation_signals += 1;
            debug!(
                request_id,
                cancel_token,
                queued_prefetch = self.tts_prefetch_queue.len(),
                queued_sync = self.tts_sync_queue.len(),
                "rendering is active; TTS scheduling remains background priority"
            );
        }

        if self.tts_prefetch_queue.is_empty() {
            if !policy.allow_sync_prefetch {
                self.tts_sync_queue.clear();
            }
        }

        let sender = self.ensure_tts_worker_channel();
        for sentence_index in self.tts_prefetch_queue.clone() {
            let config = self.config.clone();
            let analysis = analysis.clone();
            let sender = sender.clone();
            let engine = self.tts_engine;
            thread::spawn(move || {
                let started = Instant::now();
                let message =
                    match tts::prepare_sentence_clip(&config, &analysis, sentence_index, engine) {
                        Ok(clip) => TtsWorkerMessage::PrefetchCompleted {
                            request_id,
                            cancel_token,
                            clip,
                            elapsed_ms: started.elapsed().as_millis() as u64,
                        },
                        Err(err) => TtsWorkerMessage::PrefetchFailed {
                            request_id,
                            cancel_token,
                            sentence_index,
                            error: err.to_string(),
                            elapsed_ms: started.elapsed().as_millis() as u64,
                        },
                    };
                let _ = sender.send(message);
            });
        }

        if !policy.allow_sync_prefetch {
            self.tts_sync_queue.clear();
            return;
        }

        self.tts_sync_queue = tts::build_sync_prefetch_plan(
            &analysis,
            self.tts_active_sentence_index,
            self.config.tts.sentence_prefetch,
        )
        .into_iter()
        .filter(|index| !self.tts_sync_targets.contains_key(index))
        .collect();
        self.tts_profile.scheduler_queue_peak = self
            .tts_profile
            .scheduler_queue_peak
            .max((self.tts_prefetch_queue.len() + self.tts_sync_queue.len()) as u64);

        for sentence_index in self.tts_sync_queue.clone() {
            let config = self.config.clone();
            let analysis = analysis.clone();
            let sender = sender.clone();
            thread::spawn(move || {
                let started = Instant::now();
                let message = match tts::compute_sentence_sync(&config, &analysis, sentence_index) {
                    Ok(target) => TtsWorkerMessage::SyncCompleted {
                        request_id,
                        cancel_token,
                        target,
                        elapsed_ms: started.elapsed().as_millis() as u64,
                    },
                    Err(err) => TtsWorkerMessage::SyncFailed {
                        request_id,
                        cancel_token,
                        sentence_index,
                        error: err.to_string(),
                        elapsed_ms: started.elapsed().as_millis() as u64,
                    },
                };
                let _ = sender.send(message);
            });
        }
    }

    fn start_tts_playback(&mut self, ctx: &egui::Context) {
        if !self.config.tts.enabled {
            self.last_error = Some("TTS is disabled in config".into());
            return;
        }
        if self.tts_analysis.is_none() {
            self.last_error = Some(match &self.tts_analysis_status {
                TtsAnalysisStatus::Analyzing => {
                    "TTS analysis is still running. Use Rebuild TTS for a fast page window or Analyze Full TTS for the whole document.".into()
                }
                TtsAnalysisStatus::Failed(message) => {
                    format!("TTS analysis failed: {message}")
                }
                TtsAnalysisStatus::Disabled => "TTS is disabled in config".into(),
                _ => "TTS analysis is not ready yet".into(),
            });
            return;
        }
        let Some(policy) = self.active_tts_policy() else {
            self.last_error = Some("TTS runtime policy is unavailable".into());
            return;
        };
        if !policy.allow_playback {
            self.last_error = Some(format!("TTS playback is blocked: {}", policy.reason));
            return;
        }

        info!(
            command = %tts::TtsRuntimeCommand::Play.label(),
            sentence_index = self.tts_active_sentence_index,
            engine = %self.tts_engine.label(),
            "received TTS runtime command"
        );
        self.tts_playback_state = TtsPlaybackState::Preparing;
        self.tts_activation_requested_at = Some(Instant::now());
        self.start_tts_prefetch();
        self.try_activate_current_sentence(ctx);
    }

    fn pause_tts_playback(&mut self) {
        if self.tts_playback_state == TtsPlaybackState::Playing {
            info!(
                command = %tts::TtsRuntimeCommand::Pause.label(),
                sentence_index = self.tts_active_sentence_index,
                "received TTS runtime command"
            );
            if let Some(started_at) = self.tts_started_at.take() {
                self.tts_elapsed_before_pause += started_at.elapsed();
            }
            let command_id = self.next_playback_command_id();
            self.dispatch_playback_command(PlaybackCommand::Pause {
                command_id,
                cancel_token: self.tts_cancel_token,
            });
            self.tts_playback_state = TtsPlaybackState::Paused;
        }
    }

    fn resume_tts_playback(&mut self, ctx: &egui::Context) {
        if self.tts_playback_state == TtsPlaybackState::Paused {
            info!(
                command = %tts::TtsRuntimeCommand::Resume.label(),
                sentence_index = self.tts_active_sentence_index,
                "received TTS runtime command"
            );
            let command_id = self.next_playback_command_id();
            self.dispatch_playback_command(PlaybackCommand::Resume {
                command_id,
                cancel_token: self.tts_cancel_token,
            });
            self.tts_started_at = Some(Instant::now());
            self.tts_playback_state = TtsPlaybackState::Playing;
            self.trace_active_sentence_origin("resume_sentence");
            self.focus_active_sentence(ctx);
            ctx.request_repaint_after(Duration::from_millis(40));
        }
    }

    fn stop_tts_playback(&mut self) {
        info!(
            command = %tts::TtsRuntimeCommand::Stop.label(),
            sentence_index = self.tts_active_sentence_index,
            "received TTS runtime command"
        );
        let cancel_token = self.cancel_tts_work("stop_playback");
        let command_id = self.next_playback_command_id();
        self.dispatch_playback_command(PlaybackCommand::Stop {
            command_id,
            cancel_token,
        });
        self.tts_playback_state = TtsPlaybackState::Stopped;
        self.tts_prepared_clips.clear();
        self.tts_sync_targets.clear();
        self.tts_prefetch_queue.clear();
        self.tts_sync_queue.clear();
        self.tts_failed_prefetch.clear();
        self.tts_failed_sync.clear();
        self.tts_prefetch_in_flight = 0;
        self.tts_started_at = None;
        self.tts_activation_requested_at = None;
        self.tts_elapsed_before_pause = Duration::ZERO;
        self.tts_current_duration = Duration::ZERO;
        self.tts_prefetch_request_id = self.tts_prefetch_request_id.wrapping_add(1);
        if self.config.tts.enabled {
            self.tts_analysis_status = if self.tts_analysis.is_some() {
                TtsAnalysisStatus::Ready
            } else {
                TtsAnalysisStatus::Idle
            };
        }
    }

    fn next_tts_sentence(&mut self, ctx: &egui::Context) {
        let Some(analysis) = &self.tts_analysis else {
            return;
        };
        if self.tts_active_sentence_index + 1 < analysis.sentences.len() {
            info!(
                command = %tts::TtsRuntimeCommand::NextSentence.label(),
                sentence_index = self.tts_active_sentence_index + 1,
                "received TTS runtime command"
            );
            self.cancel_tts_work("next_sentence");
            self.set_tts_cursor_to_sentence_index(self.tts_active_sentence_index + 1);
            self.tts_started_at = None;
            self.tts_activation_requested_at = Some(Instant::now());
            self.start_tts_prefetch();
            self.try_activate_current_sentence(ctx);
        } else {
            self.stop_tts_playback();
        }
    }

    fn previous_tts_sentence(&mut self, ctx: &egui::Context) {
        if self.tts_active_sentence_index > 0 {
            info!(
                command = %tts::TtsRuntimeCommand::PreviousSentence.label(),
                sentence_index = self.tts_active_sentence_index.saturating_sub(1),
                "received TTS runtime command"
            );
            self.cancel_tts_work("previous_sentence");
            self.set_tts_cursor_to_sentence_index(self.tts_active_sentence_index - 1);
            self.tts_started_at = None;
            self.tts_activation_requested_at = Some(Instant::now());
            self.start_tts_prefetch();
            self.try_activate_current_sentence(ctx);
        }
    }

    fn try_activate_current_sentence(&mut self, ctx: &egui::Context) {
        if let Some(clip) = self
            .tts_prepared_clips
            .get(&self.tts_active_sentence_index)
            .cloned()
        {
            self.tts_current_duration = Duration::from_millis(clip.estimated_duration_ms);
            self.tts_started_at = Some(Instant::now());
            self.tts_elapsed_before_pause = Duration::ZERO;
            self.tts_playback_state = TtsPlaybackState::Playing;
            let command_id = self.next_playback_command_id();
            self.dispatch_playback_command(PlaybackCommand::Play {
                command_id,
                cancel_token: self.tts_cancel_token,
                audio_path: clip.audio_path.clone(),
                volume: self.config.tts.volume,
                rate: self.config.tts.rate,
            });
            self.trace_active_sentence_origin("activate_sentence");
            if let Some(requested_at) = self.tts_activation_requested_at.take() {
                let activation_ms = requested_at.elapsed().as_secs_f64() * 1000.0;
                self.tts_profile.activation.record(activation_ms);
                if activation_ms > self.config.tts.active_latency_budget_ms as f64 {
                    warn!(
                        activation_ms,
                        budget_ms = self.config.tts.active_latency_budget_ms,
                        sentence_index = self.tts_active_sentence_index,
                        "TTS activation exceeded latency budget"
                    );
                }
            }
            self.focus_active_sentence(ctx);
            self.status_message = Some(format!(
                "TTS playing sentence {}/{} using {} backend",
                self.tts_active_sentence_index + 1,
                self.tts_analysis
                    .as_ref()
                    .map(|analysis| analysis.sentences.len())
                    .unwrap_or(0),
                self.tts_engine.label()
            ));
            ctx.request_repaint_after(Duration::from_millis(40));
        } else if self.tts_prefetch_in_flight > 0 {
            self.tts_playback_state = TtsPlaybackState::Preparing;
            self.tts_profile.playback_underruns += 1;
            debug!(
                sentence_index = self.tts_active_sentence_index,
                prefetch_in_flight = self.tts_prefetch_in_flight,
                prepared_clips = self.tts_prepared_clips.len(),
                queued_clips = self.tts_prefetch_queue.len(),
                "TTS playback waiting for prepared clip"
            );
            ctx.request_repaint_after(Duration::from_millis(40));
        }
    }

    fn focus_active_sentence(&mut self, ctx: &egui::Context) {
        if !self.tts_follow_mode {
            return;
        }
        self.preload_tts_viewport_assets();

        let Some((page_index, focus_rect, scroll_reason)) = self.active_sentence_focus_target()
        else {
            return;
        };

        match self.view_mode {
            ViewMode::SinglePage => {
                self.follow_single_page_target(page_index, focus_rect, scroll_reason, ctx)
            }
            ViewMode::Continuous => {
                self.follow_continuous_target(page_index, focus_rect, scroll_reason, ctx)
            }
        }
    }

    fn active_sentence_focus_target(&self) -> Option<(usize, Option<PdfRectData>, &'static str)> {
        if let Some(target) = self.effective_sync_target(self.tts_active_sentence_index) {
            let page_index = target.page_index.unwrap_or(self.current_page);
            let focus_rect = target.rects.first().copied();
            let reason = match target.confidence {
                SentenceSyncConfidence::ExactSentence => "exact_sentence_sync",
                SentenceSyncConfidence::FuzzySentence => "fuzzy_sentence_sync",
                SentenceSyncConfidence::BlockFallback => "block_fallback_sync",
                SentenceSyncConfidence::PageFallback => "page_fallback_sync",
                SentenceSyncConfidence::Missing => "missing_sync_page_range",
            };
            return Some((page_index, focus_rect, reason));
        }

        self.active_sentence()
            .map(|sentence| (sentence.page_range.start_page, None, "page_range_follow"))
    }

    fn preload_tts_viewport_assets(&mut self) {
        let Some(document) = &self.document else {
            return;
        };
        let Some(sentence) = self.active_sentence() else {
            return;
        };

        let radius = self.config.tts.follow_preload_page_radius;
        let start_page = sentence.page_range.start_page.saturating_sub(radius);
        let end_page = (sentence.page_range.end_page + radius)
            .min(document.metadata.page_count.saturating_sub(1));

        for page_index in start_page..=end_page {
            if !self.text_segment_cache.contains_key(&page_index) {
                if let Ok(segments) = document.text_segments_for_page(page_index) {
                    self.text_segment_cache.insert(page_index, segments);
                }
            }

            if !self.text_rect_cache.contains_key(&page_index) {
                if let Ok(rects) = document.text_rects_for_page(page_index) {
                    self.text_rect_cache.insert(page_index, rects);
                }
            }
        }
    }

    fn follow_single_page_target(
        &mut self,
        page_index: usize,
        focus_rect: Option<PdfRectData>,
        reason: &'static str,
        ctx: &egui::Context,
    ) {
        if self.current_page != page_index {
            self.navigate_to_page(page_index, focus_rect, ctx);
            return;
        }

        let old_offset = self.single_scroll_offset;
        let viewport_size = self.single_viewport.size;
        let Some(target_offset) = focus_rect
            .and_then(|rect| self.page_focus_offset(page_index, rect))
            .or(Some(Vec2::ZERO))
        else {
            return;
        };

        if viewport_contains_target(
            old_offset,
            viewport_size,
            target_offset,
            self.config.tts.follow_visible_margin_ratio,
        ) {
            debug!(
                reason,
                page_index,
                old_x = old_offset.x,
                old_y = old_offset.y,
                target_x = target_offset.x,
                target_y = target_offset.y,
                "skipped TTS auto-scroll because target is already in the stable single-page region"
            );
            return;
        }

        let new_offset = if self.tts_follow_pin_to_center {
            Vec2::new(
                centered_offset(target_offset.x, viewport_size.x),
                centered_offset(target_offset.y, viewport_size.y),
            )
        } else {
            target_offset
        };
        let max_x = (self.single_viewport.content_size.x - viewport_size.x).max(0.0);
        let max_y = (self.single_viewport.content_size.y - viewport_size.y).max(0.0);
        let new_offset = Vec2::new(
            new_offset.x.clamp(0.0, max_x),
            new_offset.y.clamp(0.0, max_y),
        );
        debug!(
            reason,
            page_index,
            old_x = old_offset.x,
            old_y = old_offset.y,
            new_x = new_offset.x,
            new_y = new_offset.y,
            "applied TTS auto-scroll in single-page view"
        );
        self.single_scroll_offset = new_offset;
        self.persist_session();
        ctx.request_repaint();
    }

    fn follow_continuous_target(
        &mut self,
        page_index: usize,
        focus_rect: Option<PdfRectData>,
        reason: &'static str,
        ctx: &egui::Context,
    ) {
        self.current_page = page_index;
        let old_offset = self.continuous_scroll_offset;
        let viewport_size = self.continuous_viewport.size;
        let target_y = focus_rect
            .and_then(|rect| self.continuous_focus_offset(page_index, rect))
            .unwrap_or_else(|| self.continuous_page_top(page_index));

        if axis_contains_target(
            old_offset.y,
            viewport_size.y,
            target_y,
            self.config.tts.follow_visible_margin_ratio,
        ) {
            debug!(
                reason,
                page_index,
                old_y = old_offset.y,
                target_y,
                "skipped TTS auto-scroll because target is already in the stable continuous region"
            );
            self.persist_session();
            return;
        }

        let new_y = if self.tts_follow_pin_to_center {
            centered_offset(target_y, viewport_size.y)
        } else {
            target_y.max(0.0)
        };
        let max_y = (self.continuous_viewport.content_size.y - viewport_size.y).max(0.0);
        let new_y = new_y.clamp(0.0, max_y);
        debug!(
            reason,
            page_index,
            old_y = old_offset.y,
            new_y,
            target_y,
            "applied TTS auto-scroll in continuous view"
        );
        self.continuous_scroll_offset = Vec2::new(0.0, new_y.max(0.0));
        self.persist_session();
        ctx.request_repaint();
    }

    fn advance_tts_clock(&mut self, ctx: &egui::Context) {
        if self.tts_playback_state != TtsPlaybackState::Playing {
            return;
        }

        let elapsed = self.tts_elapsed_before_pause
            + self
                .tts_started_at
                .map(|started_at| started_at.elapsed())
                .unwrap_or_default();

        if elapsed >= self.tts_current_duration {
            self.next_tts_sentence(ctx);
        } else {
            ctx.request_repaint_after(Duration::from_millis(40));
        }
    }

    fn seek_tts_sentence(&mut self, target_sentence_index: usize, ctx: &egui::Context) {
        let Some(analysis) = &self.tts_analysis else {
            return;
        };
        if target_sentence_index >= analysis.sentences.len() {
            return;
        }
        info!(
            command = %tts::TtsRuntimeCommand::SeekToSentence.label(),
            sentence_index = target_sentence_index,
            "received TTS runtime command"
        );
        self.cancel_tts_work("seek_sentence");
        self.set_tts_cursor_to_sentence_index(target_sentence_index);
        self.tts_started_at = None;
        self.tts_activation_requested_at = Some(Instant::now());
        self.tts_elapsed_before_pause = Duration::ZERO;
        self.start_tts_prefetch();
        self.try_activate_current_sentence(ctx);
    }

    fn ensure_runtime(&mut self) -> Result<()> {
        if self.runtime.is_some() {
            return Ok(());
        }

        match PdfRuntime::new(&self.config) {
            Ok(runtime) => {
                self.runtime = Some(runtime);
                self.runtime_error = None;
                Ok(())
            }
            Err(err) => {
                self.runtime = None;
                self.runtime_error = Some(err.to_string());
                Err(err)
            }
        }
    }

    fn render_current_page(&mut self, ctx: &egui::Context) {
        self.render_slot(ctx, ViewSlot::Primary, self.current_preset);

        if self.compare_enabled {
            self.render_slot(ctx, ViewSlot::Compare, self.compare_preset);
        } else {
            self.compare_view = None;
            self.compare_tile_job = None;
        }
    }

    fn render_slot(&mut self, ctx: &egui::Context, slot: ViewSlot, preset: RenderPreset) {
        let Some(document) = &self.document else {
            return;
        };

        let key = RenderCacheKey::new(self.current_page, self.zoom, preset, slot);

        if let Some(cached) = self.render_cache.get(&key).cloned() {
            self.assign_view(slot, RenderView::from_cached(cached.clone()));
            self.record_metric(cached.elapsed_ms, true, cached.mode, preset);
            return;
        }

        let Some(page_size) = document.page_size(self.current_page) else {
            self.last_error = Some("missing page size information".into());
            return;
        };

        let full_width = scaled_page_width(page_size, self.zoom);
        let full_height = scaled_page_height(page_size, self.zoom);

        if full_width >= self.config.rendering.tile_render_min_width {
            self.start_tiled_job(slot, preset, full_width, full_height);
            ctx.request_repaint();
            return;
        }

        match document.render_page_image(&RenderRequest {
            page_index: self.current_page,
            zoom: self.zoom,
            preset,
        }) {
            Ok(rendered) => self.finish_render(ctx, key, rendered, false, preset),
            Err(err) => {
                error!(page = self.current_page, error = %err, "render failed");
                self.last_error = Some(err.to_string());
            }
        }
    }

    fn finish_render(
        &mut self,
        ctx: &egui::Context,
        key: RenderCacheKey,
        rendered: RenderedPageImage,
        from_tiles: bool,
        preset: RenderPreset,
    ) {
        let texture = ctx.load_texture(
            key.texture_name(),
            rendered.image.clone(),
            self.config.rendering.texture_filter.to_texture_options(),
        );
        let cached = CachedRender {
            texture,
            image: rendered.image,
            elapsed_ms: rendered.elapsed.as_secs_f64() * 1000.0,
            mode: if from_tiles {
                RenderMode::Tiled
            } else {
                rendered.mode
            },
        };
        self.render_cache.insert(key, cached.clone());
        self.assign_view(key.slot, RenderView::from_cached(cached.clone()));
        self.record_metric(cached.elapsed_ms, false, cached.mode, preset);
    }

    fn start_tiled_job(
        &mut self,
        slot: ViewSlot,
        preset: RenderPreset,
        full_width: i32,
        full_height: i32,
    ) {
        let tile_size = self.config.rendering.tile_size.max(64);
        let tiles = build_tiles(full_width, full_height, tile_size);
        let composite =
            ColorImage::filled([full_width as usize, full_height as usize], Color32::WHITE);

        let job = TiledRenderJob {
            key: RenderCacheKey::new(self.current_page, self.zoom, preset, slot),
            preset,
            full_width,
            tiles,
            next_tile: 0,
            composite,
            elapsed_ms: 0.0,
        };

        match slot {
            ViewSlot::Primary => self.primary_tile_job = Some(job),
            ViewSlot::Compare => self.compare_tile_job = Some(job),
        }
    }

    fn process_tiled_jobs(&mut self, ctx: &egui::Context) {
        self.process_tiled_job(ctx, ViewSlot::Primary);
        self.process_tiled_job(ctx, ViewSlot::Compare);
    }

    fn process_tiled_job(&mut self, ctx: &egui::Context, slot: ViewSlot) {
        let Some(document) = &self.document else {
            return;
        };

        let job = match slot {
            ViewSlot::Primary => self.primary_tile_job.as_mut(),
            ViewSlot::Compare => self.compare_tile_job.as_mut(),
        };

        let Some(job) = job else {
            return;
        };

        if job.next_tile >= job.tiles.len() {
            return;
        }

        let tile = job.tiles[job.next_tile];

        match document.render_tile(&TileRenderRequest {
            page_index: self.current_page,
            full_width: job.full_width,
            x: tile.x,
            y: tile.y,
            tile_width: tile.width,
            tile_height: tile.height,
            preset: job.preset,
        }) {
            Ok(rendered) => {
                blit_tile(&mut job.composite, &rendered.image, tile.x, tile.y);
                job.elapsed_ms += rendered.elapsed.as_secs_f64() * 1000.0;
                job.next_tile += 1;

                if job.next_tile >= job.tiles.len() {
                    let finished = RenderedPageImage {
                        image: job.composite.clone(),
                        elapsed: std::time::Duration::from_secs_f64(job.elapsed_ms / 1000.0),
                        mode: RenderMode::Tiled,
                    };
                    let key = job.key;
                    let preset = job.preset;
                    match slot {
                        ViewSlot::Primary => self.primary_tile_job = None,
                        ViewSlot::Compare => self.compare_tile_job = None,
                    }
                    self.finish_render(ctx, key, finished, true, preset);
                } else {
                    ctx.request_repaint();
                }
            }
            Err(err) => {
                error!(error = %err, "tiled render failed");
                self.last_error = Some(err.to_string());
                match slot {
                    ViewSlot::Primary => self.primary_tile_job = None,
                    ViewSlot::Compare => self.compare_tile_job = None,
                }
            }
        }
    }

    fn assign_view(&mut self, slot: ViewSlot, view: RenderView) {
        match slot {
            ViewSlot::Primary => self.primary_view = Some(view),
            ViewSlot::Compare => self.compare_view = Some(view),
        }
    }

    fn next_page(&mut self, ctx: &egui::Context) {
        if let Some(document) = &self.document {
            if self.current_page + 1 < document.metadata.page_count {
                self.current_page += 1;
                self.render_current_page(ctx);
                self.persist_session();
            }
        }
    }

    fn previous_page(&mut self, ctx: &egui::Context) {
        if self.current_page > 0 {
            self.current_page -= 1;
            self.render_current_page(ctx);
            self.persist_session();
        }
    }

    fn apply_zoom(&mut self, ctx: &egui::Context, new_zoom: f32) {
        let clamped = new_zoom.clamp(
            self.config.rendering.min_zoom,
            self.config.rendering.max_zoom,
        );

        if (clamped - self.zoom).abs() < f32::EPSILON {
            return;
        }

        let factor = clamped / self.zoom.max(0.0001);
        self.zoom = clamped;
        self.single_scroll_offset *= factor;
        self.continuous_scroll_offset *= factor;
        self.render_current_page(ctx);
        self.persist_session();
    }

    fn scroll_source_for_input(&self, ctx: &egui::Context) -> egui::scroll_area::ScrollSource {
        if ctx.input(|input| input.modifiers.ctrl) {
            egui::scroll_area::ScrollSource {
                mouse_wheel: false,
                ..egui::scroll_area::ScrollSource::ALL
            }
        } else {
            egui::scroll_area::ScrollSource::ALL
        }
    }

    fn navigate_to_page(
        &mut self,
        page_index: usize,
        focus_rect: Option<PdfRectData>,
        ctx: &egui::Context,
    ) {
        self.current_page = page_index;
        if self.tts_playback_state == TtsPlaybackState::Stopped {
            self.sync_tts_sentence_to_current_page();
        }
        self.enforce_tts_runtime_budget();

        match self.view_mode {
            ViewMode::SinglePage => {
                let size = self.page_image_size(page_index).unwrap_or(Vec2::ZERO);
                let target = focus_rect
                    .and_then(|rect| self.page_focus_offset(page_index, rect))
                    .unwrap_or(Vec2::ZERO);
                self.single_scroll_offset = Vec2::new(
                    target.x.clamp(0.0, size.x.max(0.0)),
                    target.y.clamp(0.0, size.y.max(0.0)),
                );
                self.render_current_page(ctx);
            }
            ViewMode::Continuous => {
                let y = focus_rect
                    .and_then(|rect| self.continuous_focus_offset(page_index, rect))
                    .unwrap_or_else(|| self.continuous_page_top(page_index));
                self.continuous_scroll_offset = Vec2::new(0.0, y.max(0.0));
                self.render_current_page(ctx);
            }
        }

        self.persist_session();
        ctx.request_repaint();
    }

    fn navigate_to_sentence(
        &mut self,
        sentence_index: usize,
        focus_rect: Option<PdfRectData>,
        ctx: &egui::Context,
    ) {
        self.set_tts_cursor_to_sentence_index(sentence_index);
        if let Some(target) = self.effective_sync_target(sentence_index) {
            let page_index = target.page_index.unwrap_or(self.current_page);
            let rect = focus_rect.or_else(|| target.rects.first().copied());
            self.navigate_to_page(page_index, rect, ctx);
            return;
        }

        if let Some(sentence) = self
            .tts_analysis
            .as_ref()
            .and_then(|analysis| analysis.sentences.get(sentence_index))
        {
            self.navigate_to_page(sentence.page_range.start_page, focus_rect, ctx);
        }
    }

    fn page_focus_offset(&self, page_index: usize, rect: PdfRectData) -> Option<Vec2> {
        let size = self.page_image_size(page_index)?;
        let document = self.document.as_ref()?;
        let page_size = document.page_size(page_index)?;
        let x = size.x * (rect.left / page_size.width) - 40.0;
        let y = size.y * (1.0 - (rect.top / page_size.height)) - 40.0;
        Some(Vec2::new(x.max(0.0), y.max(0.0)))
    }

    fn continuous_focus_offset(&self, page_index: usize, rect: PdfRectData) -> Option<f32> {
        let page_top = self.continuous_page_top(page_index);
        let size = self.page_image_size(page_index)?;
        let document = self.document.as_ref()?;
        let page_size = document.page_size(page_index)?;
        let y_in_page = size.y * (1.0 - (rect.top / page_size.height)) - 40.0;
        Some((page_top + y_in_page).max(0.0))
    }

    fn continuous_page_top(&self, page_index: usize) -> f32 {
        let mut offset = 0.0;
        for index in 0..page_index {
            if let Some(size) = self.page_image_size(index) {
                offset += size.y + 14.0;
            }
        }
        offset
    }

    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        let open_shortcut = egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, Key::O);
        let save_shortcut = egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, Key::S);

        if ctx.input_mut(|input| input.consume_shortcut(&open_shortcut)) {
            if let Some(path) = FileDialog::new().add_filter("PDF", &["pdf"]).pick_file() {
                self.open_pdf(ctx, path);
            }
        }

        if ctx.input_mut(|input| input.consume_shortcut(&save_shortcut)) {
            self.save_config_from_editor(ctx);
        }

        if ctx.input(|input| input.key_pressed(Key::ArrowRight) || input.key_pressed(Key::PageDown))
        {
            self.next_page(ctx);
        }

        if ctx.input(|input| input.key_pressed(Key::ArrowLeft) || input.key_pressed(Key::PageUp)) {
            self.previous_page(ctx);
        }

        if ctx.input(|input| input.key_pressed(Key::Plus) || input.key_pressed(Key::Equals)) {
            self.apply_zoom(ctx, self.zoom + self.config.rendering.cache_zoom_bucket);
        }

        if ctx.input(|input| input.key_pressed(Key::Minus)) {
            self.apply_zoom(ctx, self.zoom - self.config.rendering.cache_zoom_bucket);
        }

        if ctx.input(|input| input.key_pressed(Key::Num0)) {
            self.apply_zoom(ctx, self.config.rendering.initial_zoom);
        }

        if ctx.input(|input| input.key_pressed(Key::Space)) {
            match self.tts_playback_state {
                TtsPlaybackState::Playing => self.pause_tts_playback(),
                TtsPlaybackState::Paused => self.resume_tts_playback(ctx),
                _ => self.start_tts_playback(ctx),
            }
        }

        if ctx.input(|input| input.key_pressed(Key::Period)) {
            self.next_tts_sentence(ctx);
        }

        if ctx.input(|input| input.key_pressed(Key::Comma)) {
            self.previous_tts_sentence(ctx);
        }

        let ctrl_scroll = ctx.input(|input| {
            if input.modifiers.ctrl {
                input.raw_scroll_delta.y
            } else {
                0.0
            }
        });

        if ctrl_scroll.abs() > f32::EPSILON {
            let factor = if ctrl_scroll > 0.0 { 1.1 } else { 1.0 / 1.1 };
            self.apply_zoom(ctx, self.zoom * factor);
        }
    }

    fn top_bar(&mut self, ctx: &egui::Context, ui: &mut Ui) {
        if self.runtime.is_none() && ui.button("Load Pdfium").clicked() {
            if let Some(path) = FileDialog::new()
                .add_filter("Shared Library", &["so", "dll", "dylib"])
                .pick_file()
            {
                self.config.pdfium.library_path = Some(path.display().to_string());
                self.config_preview = self.config.config_preview();
                self.config_editor = self.config_preview.clone();
                match self.ensure_runtime() {
                    Ok(()) => {
                        self.status_message =
                            Some(format!("Loaded Pdfium from {}", path.display()));
                        self.last_error = None;
                    }
                    Err(err) => {
                        self.last_error = Some(err.to_string());
                    }
                }
            }
        }

        if ui.button("Open PDF").clicked() {
            if let Some(path) = FileDialog::new().add_filter("PDF", &["pdf"]).pick_file() {
                self.open_pdf(ctx, path);
            } else {
                debug!("open file dialog was cancelled");
            }
        }

        let has_document = self.document.is_some();
        if ui
            .add_enabled(has_document, egui::Button::new("Prev"))
            .clicked()
        {
            self.previous_page(ctx);
        }
        if ui
            .add_enabled(has_document, egui::Button::new("Next"))
            .clicked()
        {
            self.next_page(ctx);
        }

        ui.separator();

        let zoom_before = self.zoom;
        let zoom_response = ui.add_enabled(
            has_document,
            Slider::new(
                &mut self.zoom,
                self.config.rendering.min_zoom..=self.config.rendering.max_zoom,
            )
            .logarithmic(true)
            .text("Zoom"),
        );

        if zoom_response.changed() {
            let zoom_after = self.zoom;
            self.zoom = zoom_before;
            self.apply_zoom(ctx, zoom_after);
        }

        ui.separator();
        ui.label("View");
        egui::ComboBox::from_id_salt("view_mode")
            .selected_text(self.view_mode.label())
            .show_ui(ui, |ui| {
                for mode in [ViewMode::SinglePage, ViewMode::Continuous] {
                    if ui
                        .selectable_value(&mut self.view_mode, mode, mode.label())
                        .changed()
                    {
                        if self.view_mode == ViewMode::Continuous {
                            self.compare_enabled = false;
                        }
                        self.render_current_page(ctx);
                    }
                }
            });

        ui.separator();
        ui.label("Preset");
        egui::ComboBox::from_id_salt("primary_preset")
            .selected_text(self.current_preset.label())
            .show_ui(ui, |ui| {
                for preset in RenderPreset::all() {
                    if ui
                        .selectable_value(&mut self.current_preset, *preset, preset.label())
                        .changed()
                    {
                        self.render_current_page(ctx);
                        self.persist_session();
                    }
                }
            });

        ui.add_enabled_ui(self.view_mode == ViewMode::SinglePage, |ui| {
            ui.checkbox(&mut self.compare_enabled, "Compare");
        });
        if self.view_mode == ViewMode::SinglePage && self.compare_enabled {
            egui::ComboBox::from_id_salt("compare_preset")
                .selected_text(self.compare_preset.label())
                .show_ui(ui, |ui| {
                    for preset in RenderPreset::all() {
                        if ui
                            .selectable_value(&mut self.compare_preset, *preset, preset.label())
                            .changed()
                        {
                            self.render_current_page(ctx);
                            self.persist_session();
                        }
                    }
                });
        }

        if ui.button("Re-render").clicked() {
            self.render_current_page(ctx);
        }

        if ui
            .add_enabled(
                self.document.is_some() && self.config.tts.enabled,
                egui::Button::new("Rebuild TTS"),
            )
            .clicked()
        {
            self.rebuild_tts_analysis();
        }
        if ui
            .add_enabled(
                self.document.is_some() && self.config.tts.enabled,
                egui::Button::new("Analyze Full TTS"),
            )
            .clicked()
        {
            self.rebuild_tts_analysis_full();
        }

        ui.separator();
        ui.label("Search");
        let response = ui.add(
            egui::TextEdit::singleline(&mut self.search_query)
                .desired_width(180.0)
                .hint_text("Find text"),
        );
        if response.lost_focus() && ui.input(|input| input.key_pressed(Key::Enter)) {
            self.run_search();
        }
        if ui.button("Find").clicked() {
            self.run_search();
        }
        if ui
            .add_enabled(
                !self.search_results.is_empty(),
                egui::Button::new("Prev hit"),
            )
            .clicked()
        {
            let next_index = match self.active_search_result {
                Some(0) | None => self.search_results.len().saturating_sub(1),
                Some(index) => index.saturating_sub(1),
            };
            self.activate_search_result(next_index, ctx);
        }
        if ui
            .add_enabled(
                !self.search_results.is_empty(),
                egui::Button::new("Next hit"),
            )
            .clicked()
        {
            let next_index = match self.active_search_result {
                Some(index) => (index + 1) % self.search_results.len(),
                None => 0,
            };
            self.activate_search_result(next_index, ctx);
        }
        ui.checkbox(&mut self.search_match_case, "Aa");
        ui.checkbox(&mut self.search_whole_word, "Word");
        ui.checkbox(&mut self.highlight_text, "Highlight text");
        ui.separator();
        ui.label(format!(
            "TTS {}",
            match self.tts_analysis_status {
                TtsAnalysisStatus::Ready => "ready",
                TtsAnalysisStatus::Analyzing => "analyzing",
                TtsAnalysisStatus::Disabled => "disabled",
                TtsAnalysisStatus::Idle => "idle",
                TtsAnalysisStatus::Failed(_) => "failed",
            }
        ));

        if let Some(document) = &self.document {
            ui.separator();
            ui.label(format!(
                "Page {}/{}",
                self.current_page + 1,
                document.metadata.page_count
            ));
        }
    }

    fn tts_player_bar(&mut self, ctx: &egui::Context, ui: &mut Ui) {
        ui.horizontal_wrapped(|ui| {
            ui.strong("TTS");

            let has_analysis = self.tts_analysis.is_some();
            if ui
                .add_enabled(
                    has_analysis,
                    egui::Button::new(match self.tts_playback_state {
                        TtsPlaybackState::Playing => "Pause",
                        TtsPlaybackState::Paused => "Resume",
                        _ => "Play",
                    }),
                )
                .clicked()
            {
                match self.tts_playback_state {
                    TtsPlaybackState::Playing => self.pause_tts_playback(),
                    TtsPlaybackState::Paused => self.resume_tts_playback(ctx),
                    _ => self.start_tts_playback(ctx),
                }
            }

            if ui
                .add_enabled(
                    has_analysis && self.tts_playback_state != TtsPlaybackState::Stopped,
                    egui::Button::new("Stop"),
                )
                .clicked()
            {
                self.stop_tts_playback();
            }

            if ui
                .add_enabled(
                    has_analysis && self.tts_active_sentence_index > 0,
                    egui::Button::new("Prev"),
                )
                .clicked()
            {
                self.previous_tts_sentence(ctx);
            }

            if ui
                .add_enabled(
                    self.tts_analysis.as_ref().is_some_and(|analysis| {
                        self.tts_active_sentence_index + 1 < analysis.sentences.len()
                    }),
                    egui::Button::new("Next"),
                )
                .clicked()
            {
                self.next_tts_sentence(ctx);
            }

            ui.separator();
            ui.checkbox(&mut self.tts_follow_mode, "Follow");
            ui.add_enabled_ui(self.tts_follow_mode, |ui| {
                ui.checkbox(&mut self.tts_follow_pin_to_center, "Pin");
            });
            ui.checkbox(&mut self.tts_highlights_enabled, "Highlight");
            ui.checkbox(&mut self.tts_experimental_sync_enabled, "Experimental sync");
            ui.checkbox(
                &mut self.tts_verbose_degraded_logging_enabled,
                "Verbose degraded",
            );

            if let Some(analysis) = self
                .tts_analysis
                .as_ref()
                .map(|analysis| analysis.sentences.len())
            {
                let mut target_sentence = self.tts_active_sentence_index;
                let response = ui.add(
                    egui::DragValue::new(&mut target_sentence)
                        .speed(1.0)
                        .range(0..=analysis.saturating_sub(1))
                        .prefix("Sentence "),
                );
                if response.changed() {
                    self.seek_tts_sentence(target_sentence, ctx);
                }

                let page_label = self
                    .active_sentence()
                    .map(|sentence| {
                        if sentence.page_range.start_page == sentence.page_range.end_page {
                            format!("Page {}", sentence.page_range.start_page + 1)
                        } else {
                            format!(
                                "Pages {}-{}",
                                sentence.page_range.start_page + 1,
                                sentence.page_range.end_page + 1
                            )
                        }
                    })
                    .unwrap_or_else(|| "Page n/a".into());
                let sync_label = self
                    .effective_sync_target(self.tts_active_sentence_index)
                    .map(|target| target.confidence.label().to_string())
                    .unwrap_or_else(|| "unmapped".into());

                ui.separator();
                ui.label(format!(
                    "{} / {} | {} | sync {} | engine {} | state {}",
                    self.tts_active_sentence_index + 1,
                    analysis,
                    page_label,
                    sync_label,
                    self.tts_engine.label(),
                    self.tts_playback_state.label()
                ));

                if let Some(policy) = self.active_tts_policy() {
                    if self.tts_verbose_degraded_logging_enabled
                        && (!policy.allow_rect_highlights || !policy.allow_sync_prefetch)
                    {
                        ui.colored_label(
                            Color32::YELLOW,
                            format!("Degraded mode: {}", policy.reason),
                        );
                    }
                }
            } else {
                ui.label("No TTS analysis for this document.");
            }
        });
    }

    fn side_panel(&mut self, ctx: &egui::Context, ui: &mut Ui) {
        ui.heading("Study Controls");
        ui.label("Use this shell to inspect PDFium behavior in a native Rust rendering loop.");
        ui.separator();

        if let Some(runtime_error) = &self.runtime_error {
            ui.colored_label(Color32::LIGHT_RED, runtime_error);
            ui.separator();
        }

        render_profiles(ui, self, ctx);
        ui.separator();

        if let Some(document) = &self.document {
            render_metadata(ui, &document.metadata);
        } else {
            ui.label("No document loaded.");
        }
    }

    fn thumbnail_panel(&mut self, ctx: &egui::Context, ui: &mut Ui) {
        let page_count = match &self.document {
            Some(document) => document.metadata.page_count,
            None => {
                ui.centered_and_justified(|ui| {
                    ui.label("No thumbnails");
                });
                return;
            }
        };

        ui.heading("Pages");
        ui.separator();

        ScrollArea::vertical().show(ui, |ui| {
            for page in 0..page_count {
                let key = ThumbnailCacheKey {
                    page,
                    preset: self.current_preset,
                    size: self.config.rendering.thumbnail_size,
                };

                if !self.thumbnail_cache.contains_key(&key) {
                    let rendered = {
                        let Some(document) = &self.document else {
                            return;
                        };

                        document.render_thumbnail(
                            page,
                            self.config.rendering.thumbnail_size,
                            self.current_preset,
                        )
                    };

                    match rendered {
                        Ok(rendered) => {
                            let texture = ctx.load_texture(
                                format!("thumb-{}-{}", page, self.current_preset.as_str()),
                                rendered.image.clone(),
                                TextureOptions::LINEAR,
                            );
                            self.thumbnail_cache.insert(
                                key,
                                CachedRender {
                                    texture,
                                    image: rendered.image,
                                    elapsed_ms: rendered.elapsed.as_secs_f64() * 1000.0,
                                    mode: RenderMode::Thumbnail,
                                },
                            );
                        }
                        Err(err) => {
                            warn!(page, error = %err, "thumbnail render failed");
                        }
                    }
                }

                let cached = self.thumbnail_cache.get(&key).cloned();
                if let Some(cached) = cached {
                    let selected = page == self.current_page;
                    let frame = egui::Frame::group(ui.style()).fill(if selected {
                        Color32::from_gray(48)
                    } else {
                        Color32::TRANSPARENT
                    });
                    frame.show(ui, |ui| {
                        if ui
                            .add(egui::Button::image(
                                egui::Image::new(&cached.texture)
                                    .fit_to_exact_size(cached.texture.size_vec2()),
                            ))
                            .clicked()
                        {
                            self.navigate_to_page(page, None, ctx);
                        }
                        ui.label(format!("Page {}", page + 1));
                    });
                    ui.separator();
                }
            }
        });
    }

    fn bottom_panel(&mut self, ctx: &egui::Context, ui: &mut Ui) {
        ui.heading("Instrumentation");
        if let Some(error) = &self.last_error {
            ui.colored_label(Color32::LIGHT_RED, error);
        }
        if let Some(message) = &self.status_message {
            ui.colored_label(Color32::LIGHT_GREEN, message);
        }

        if self.config.ui.show_metrics {
            let summary = metric_summary(&self.render_history);
            ui.label(format!(
                "Renders: {} | avg {:.2} ms | min {:.2} ms | max {:.2} ms",
                summary.count, summary.average_ms, summary.min_ms, summary.max_ms
            ));

            if let Some(view) = &self.primary_view {
                ui.label(format!(
                    "Primary: {:.2} ms | {} x {} px | {}",
                    view.elapsed_ms,
                    view.image.size[0],
                    view.image.size[1],
                    match view.mode {
                        RenderMode::FullPage => "full",
                        RenderMode::Thumbnail => "thumbnail",
                        RenderMode::Tiled => "tiled",
                    }
                ));
            }
        }

        if let Some(pixel) = &self.pixel_sample {
            ui.label(format!(
                "Pixel: ({}, {}) rgba({}, {}, {}, {})",
                pixel.x, pixel.y, pixel.rgba[0], pixel.rgba[1], pixel.rgba[2], pixel.rgba[3]
            ));
        }

        ui.separator();
        ui.collapsing("Text selection", |ui| {
            ui.label("Select text directly on the PDF page and copy it with the usual clipboard shortcut.");
        });

        ui.collapsing("TTS diagnostics", |ui| {
            ui.label(format!("Status: {}", self.tts_analysis_status.label()));
            ui.label(format!(
                "Playback: {} | engine: {} | active sentence: {} | follow: {} | pin: {} | highlight: {} | experimental sync: {}",
                self.tts_playback_state.label(),
                self.tts_engine.label(),
                self.tts_active_sentence_index + 1,
                self.tts_follow_mode,
                self.tts_follow_pin_to_center,
                self.tts_highlights_enabled,
                self.tts_experimental_sync_enabled
            ));

            if let Some(analysis) = &self.tts_analysis {
                ui.label(format!(
                    "Mode: {} ({:.0}% confidence) | source {:?}",
                    analysis.mode.label(),
                    analysis.confidence * 100.0,
                    analysis.text_source
                ));
                ui.label(format!(
                    "Analysis scope: pages {}-{}{}",
                    analysis.analysis_scope.start_page + 1,
                    analysis.analysis_scope.end_page + 1,
                    if analysis.analysis_scope.full_document {
                        " | full-document"
                    } else {
                        " | windowed"
                    }
                ));
                if let Some(ocr_trust) = analysis.ocr_trust {
                    ui.label(format!(
                        "OCR: trust {} | confidence {:.0}%{}",
                        ocr_trust.label(),
                        analysis.ocr_confidence.unwrap_or_default() * 100.0,
                        analysis
                            .ocr_artifact_path
                            .as_ref()
                            .map(|path| format!(" | artifact {}", path.display()))
                            .unwrap_or_default()
                    ));
                }
                if let Some(policy) = self.active_tts_policy() {
                ui.label(format!(
                    "Policy: playback {} | rect highlight {} | sync prefetch {} | max sync {}",
                        policy.allow_playback,
                        policy.allow_rect_highlights,
                        policy.allow_sync_prefetch,
                        policy.max_sync_confidence.label()
                    ));
                    ui.label(format!("Policy reason: {}", policy.reason));
                    ui.label(format!(
                        "Local thresholds: exact {:.2} | fuzzy {:.2} | block {:.2} | verbose degraded {}",
                        self.config.tts.exact_sync_min_score,
                        self.config.tts.fuzzy_sync_min_score,
                        self.config.tts.block_sync_min_score,
                        self.tts_verbose_degraded_logging_enabled
                    ));
                }
                ui.label(format!(
                    "Sentences: {} | chars: {} | text pages: {} / {}",
                    analysis.sentences.len(),
                    analysis.stats.normalized_chars,
                    analysis.stats.pages_with_text,
                    analysis.pages.len()
                ));
                ui.label(format!(
                    "Canonical: blocks {} | lines {} | tokens {}",
                    analysis.canonical_text.block_count,
                    analysis.canonical_text.line_count,
                    analysis.canonical_text.token_count
                ));
                ui.label(format!(
                    "Classification: coverage {:.2} | duplicate {:.2} | boilerplate {:.2} | reason {}",
                    analysis.classification.coverage_ratio,
                    analysis.classification.duplicate_ratio,
                    analysis.classification.boilerplate_ratio,
                    analysis.classification.reason
                ));
                ui.label(format!(
                    "Normalization: ligatures {} | soft hyphens {} | duplicate lines {} | repeated edge lines {} | joined hyphenations {}",
                    analysis.stats.ligatures_replaced,
                    analysis.stats.soft_hyphens_removed,
                    analysis.stats.duplicate_lines_removed,
                    analysis.stats.repeated_edge_lines_removed,
                    analysis.stats.joined_hyphenations
                ));
                ui.label(format!(
                    "Extraction: blocks {} | lines {} | column reorders {} | duplicate segments {} | rotated segments {}",
                    analysis.stats.extracted_blocks,
                    analysis.stats.extracted_lines,
                    analysis.stats.column_reorders,
                    analysis.stats.duplicate_segments_suppressed,
                    analysis.stats.rotated_segments_suppressed
                ));
                ui.label(format!(
                    "Structure heuristics: table-like {} | captions {} | footnotes {} | sidenotes {} | block fallbacks {}",
                    analysis.stats.table_like_blocks,
                    analysis.stats.caption_like_blocks,
                    analysis.stats.footnote_like_blocks,
                    analysis.stats.sidenote_like_blocks,
                    analysis.stats.block_fallback_units
                ));
                if let Some(path) = &analysis.artifact_path {
                    ui.label(format!("Artifact: {}", path.display()));
                }
                if let Some(sentence) = self.active_sentence() {
                    let mut sentence_text = sentence.text.clone();
                    ui.label(format!("Sentence id: {}", sentence.id));
                    ui.label(format!("Sentence unit: {:?}", sentence.unit_kind));
                    ui.label(format!(
                        "Sentence page range: {}-{}",
                        sentence.page_range.start_page + 1,
                        sentence.page_range.end_page + 1
                    ));
                    if let Some(target) =
                        self.effective_sync_target(self.tts_active_sentence_index)
                    {
                        ui.label(format!(
                            "Sync: {} | score {:.2} | reason {}",
                            target.confidence.label(),
                            target.score,
                            target.fallback_reason
                        ));
                        ui.label(format!(
                            "Sync breakdown: text {:.2} | order {:.2} | geometry {:.2} | page {:.2} | lineage {}",
                            target.score_breakdown.text_similarity,
                            target.score_breakdown.reading_order,
                            target.score_breakdown.geometry_compactness,
                            target.score_breakdown.page_continuity,
                            target.lineage.len()
                        ));
                        if let Some(path) = &target.artifact_path {
                            ui.label(format!("Sync artifact: {}", path.display()));
                        }
                    }
                    ui.add(
                        egui::TextEdit::multiline(&mut sentence_text)
                            .desired_rows(3)
                            .font(egui::TextStyle::Monospace)
                            .interactive(false),
                    );
                }
            } else if matches!(self.tts_analysis_status, TtsAnalysisStatus::Idle) {
                ui.label("No TTS analysis has been built for the current document yet.");
            }

            if let Ok(audio_cache_dir) = self.config.tts_audio_cache_dir() {
                ui.label(format!("Audio cache dir: {}", audio_cache_dir.display()));
            }
            let snapshot = tts::prefetch_snapshot(
                self.tts_active_sentence_index,
                {
                    let mut indexes = self.tts_prepared_clips.keys().copied().collect::<Vec<_>>();
                    indexes.sort_unstable();
                    indexes
                },
                self.tts_prefetch_queue.clone(),
                {
                    let mut indexes = self.tts_failed_prefetch.keys().copied().collect::<Vec<_>>();
                    indexes.sort_unstable();
                    indexes
                },
                self.tts_prefetch_request_id,
                self.tts_engine,
            );
            ui.label(format!(
                "Prepared clips: {} | queued: {} | sync queued: {} | failed prep: {} | failed sync: {} | in flight: {} | prefetch request: {} | cancel token: {}",
                snapshot.prepared_sentence_indexes.len(),
                snapshot.queued_sentence_indexes.len(),
                self.tts_sync_queue.len(),
                snapshot.failed_sentence_indexes.len(),
                self.tts_failed_sync.len(),
                self.tts_prefetch_in_flight,
                snapshot.request_id,
                self.tts_cancel_token
            ));
            ui.label(format!(
                "Perf: prepare avg {:.1} ms max {:.1} | sync avg {:.1} ms max {:.1} | activate avg {:.1} ms max {:.1}",
                self.tts_profile.prepare.average_ms(),
                self.tts_profile.prepare.max_ms,
                self.tts_profile.sync.average_ms(),
                self.tts_profile.sync.max_ms,
                self.tts_profile.activation.average_ms(),
                self.tts_profile.activation.max_ms
            ));
            ui.label(format!(
                "Perf guardrails: latency budget {} ms | cache hits {} | underruns {} | stale worker results {} | cancelled worker results {} | cancelled playback events {} | queue peak {} | starvation signals {} | playback worker failures {} | sync counts exact {} fuzzy {} block {} page {} missing {}",
                self.config.tts.active_latency_budget_ms,
                self.tts_profile.prepare_cache_hits,
                self.tts_profile.playback_underruns,
                self.tts_profile.stale_worker_results,
                self.tts_profile.cancelled_worker_results,
                self.tts_profile.cancelled_playback_events,
                self.tts_profile.scheduler_queue_peak,
                self.tts_profile.starvation_signals,
                self.tts_profile.playback_worker_failures,
                self.tts_profile.exact_sync_count,
                self.tts_profile.fuzzy_sync_count,
                self.tts_profile.block_sync_count,
                self.tts_profile.page_sync_count,
                self.tts_profile.missing_sync_count
            ));
            ui.label(format!(
                "Sync failure counters: wrong-page {} | distant-geometry {} | unmappable {}",
                self.tts_profile.wrong_page_rejects,
                self.tts_profile.distant_geometry_rejects,
                self.tts_profile.unmappable_sentences
            ));
            ui.label(format!(
                "Viewports: single {:.0}x{:.0} @ ({:.0}, {:.0}) | continuous {:.0}x{:.0} @ ({:.0}, {:.0})",
                self.single_viewport.size.x,
                self.single_viewport.size.y,
                self.single_viewport.offset.x,
                self.single_viewport.offset.y,
                self.continuous_viewport.size.x,
                self.continuous_viewport.size.y,
                self.continuous_viewport.offset.x,
                self.continuous_viewport.offset.y
            ));

            if ui
                .add_enabled(self.document.is_some() && self.config.tts.enabled, egui::Button::new("Rebuild TTS analysis"))
                .clicked()
            {
                self.rebuild_tts_analysis();
            }
            if ui
                .add_enabled(
                    self.document.is_some() && self.tts_analysis.is_some(),
                    egui::Button::new("Export TTS debug snapshot"),
                )
                .clicked()
            {
                match self.export_tts_debug_snapshot() {
                    Ok(path) => {
                        self.status_message =
                            Some(format!("Exported TTS debug snapshot to {}", path.display()));
                    }
                    Err(err) => {
                        self.last_error = Some(format!("failed to export TTS debug snapshot: {err}"));
                    }
                }
            }
        });

        ui.collapsing("Search results", |ui| {
            if self.search_results.is_empty() {
                ui.label("No active results.");
            } else {
                let ctx = ctx.clone();
                ScrollArea::vertical().max_height(180.0).show(ui, |ui| {
                    let results: Vec<(usize, String, usize)> = self
                        .search_results
                        .iter()
                        .enumerate()
                        .map(|(index, hit)| {
                            (
                                index,
                                if hit.snippet.is_empty() {
                                    self.search_query.clone()
                                } else {
                                    hit.snippet.clone()
                                },
                                hit.page_index,
                            )
                        })
                        .collect();
                    for (index, snippet, page_index) in results {
                        let selected = Some(index) == self.active_search_result;
                        if ui
                            .selectable_label(selected, format!("p{}: {}", page_index + 1, snippet))
                            .clicked()
                        {
                            self.activate_search_result(index, &ctx);
                        }
                    }
                });
            }
        });

        ui.horizontal(|ui| {
            if ui.button("Export benchmark snapshot").clicked() {
                match self.export_benchmark_snapshot() {
                    Ok(path) => {
                        self.status_message =
                            Some(format!("Saved benchmark snapshot to {}", path.display()));
                    }
                    Err(err) => {
                        self.last_error = Some(err.to_string());
                    }
                }
            }

            if ui.button("Clear render history").clicked() {
                self.render_history.clear();
            }
        });

        if self.config.ui.show_logs_hint {
            ui.separator();
            ui.monospace("Shortcuts: Cmd/Ctrl+O open, arrows/page up/down navigate, +/- zoom, Cmd/Ctrl+S save config");
            ui.monospace("Tracing: RUST_LOG=pdfizer=trace cargo run");
        }

        ui.separator();
        ui.collapsing("Config editor", |ui| {
            ui.label(
                "Edit the resolved config TOML and save it back to the preferred config path.",
            );
            ui.add(
                egui::TextEdit::multiline(&mut self.config_editor)
                    .font(egui::TextStyle::Monospace)
                    .desired_rows(18),
            );

            ui.horizontal(|ui| {
                if ui.button("Save config").clicked() {
                    self.save_config_from_editor(ctx);
                }

                if ui.button("Reset editor").clicked() {
                    self.config_editor = self.config_preview.clone();
                }
            });
        });
    }

    fn save_config_from_editor(&mut self, ctx: &egui::Context) {
        match toml::from_str::<AppConfig>(&self.config_editor) {
            Ok(new_config) => {
                if let Err(err) = self.write_config_and_apply(new_config, ctx) {
                    self.last_error = Some(err.to_string());
                }
            }
            Err(err) => {
                self.last_error = Some(format!("config parse failed: {err}"));
            }
        }
    }

    fn write_config_and_apply(&mut self, new_config: AppConfig, ctx: &egui::Context) -> Result<()> {
        let tts_reconfigured = self.tts_engine != TtsEngineKind::from_name(&new_config.tts.engine)
            || (self.config.tts.rate - new_config.tts.rate).abs() > f32::EPSILON
            || (self.config.tts.volume - new_config.tts.volume).abs() > f32::EPSILON
            || self.config.tts.voice != new_config.tts.voice;
        let path = new_config.preferred_config_path()?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }

        fs::write(&path, toml::to_string_pretty(&new_config)?)
            .with_context(|| format!("failed to write {}", path.display()))?;

        self.config = new_config.clone();
        self.tts_engine = TtsEngineKind::from_name(&new_config.tts.engine);
        self.tts_follow_pin_to_center = new_config.tts.follow_center_on_target;
        self.tts_experimental_sync_enabled = new_config.tts.experimental_pdf_sync;
        self.tts_verbose_degraded_logging_enabled = new_config.tts.verbose_degraded_logging;
        self.config_preview = new_config.config_preview();
        self.config_editor = self.config_preview.clone();
        self.status_message = Some(format!("Saved config to {}", path.display()));

        match PdfRuntime::new(&self.config) {
            Ok(runtime) => {
                self.runtime = Some(runtime);
                self.runtime_error = None;
            }
            Err(err) => {
                self.runtime = None;
                self.runtime_error = Some(err.to_string());
            }
        }

        self.tts_analysis_status = if self.config.tts.enabled {
            TtsAnalysisStatus::Idle
        } else {
            TtsAnalysisStatus::Disabled
        };

        if tts_reconfigured {
            let cancel_token = self.cancel_tts_work("engine_reconfiguration");
            let command_id = self.next_playback_command_id();
            self.dispatch_playback_command(PlaybackCommand::Stop {
                command_id,
                cancel_token,
            });
        }

        if self.document.is_some() && self.config.tts.enabled {
            self.rebuild_tts_analysis();
        }

        self.render_current_page(ctx);

        Ok(())
    }

    fn run_search(&mut self) {
        let Some(document) = &self.document else {
            self.last_error = Some("open a PDF before searching".into());
            return;
        };

        let query = self.search_query.trim();
        if query.is_empty() {
            self.search_results.clear();
            self.active_search_result = None;
            return;
        }

        let mut results = Vec::new();
        for page_index in 0..document.metadata.page_count {
            match document.search_page(
                page_index,
                query,
                self.search_match_case,
                self.search_whole_word,
            ) {
                Ok(mut hits) => results.append(&mut hits),
                Err(err) => {
                    self.last_error =
                        Some(format!("search failed on page {}: {err}", page_index + 1));
                    return;
                }
            }
        }

        self.search_results = results;
        self.active_search_result = (!self.search_results.is_empty()).then_some(0);
        if let Some(index) = self.active_search_result {
            let result = &self.search_results[index];
            self.current_page = result.page_index;
            self.set_tts_cursor_to_nearest_sentence(
                result.page_index,
                result.rects.first().copied(),
            );
        }
        self.status_message = Some(format!("Found {} result(s)", self.search_results.len()));
    }

    fn activate_search_result(&mut self, index: usize, ctx: &egui::Context) {
        if let Some((page_index, focus_rect)) = self
            .search_results
            .get(index)
            .map(|result| (result.page_index, result.rects.first().copied()))
        {
            self.active_search_result = Some(index);
            if let Some(sentence_index) =
                self.set_tts_cursor_to_nearest_sentence(page_index, focus_rect)
            {
                self.navigate_to_sentence(sentence_index, focus_rect, ctx);
            } else {
                self.navigate_to_page(page_index, focus_rect, ctx);
            }
        }
    }

    fn ensure_page_view_cached(
        &mut self,
        ctx: &egui::Context,
        page_index: usize,
        preset: RenderPreset,
    ) -> Option<RenderView> {
        let document = self.document.as_ref()?;
        let key = RenderCacheKey::new(page_index, self.zoom, preset, ViewSlot::Primary);

        if let Some(cached) = self.render_cache.get(&key).cloned() {
            return Some(RenderView::from_cached(cached));
        }

        let rendered = document
            .render_page_image(&RenderRequest {
                page_index,
                zoom: self.zoom,
                preset,
            })
            .ok()?;
        let texture = ctx.load_texture(
            key.texture_name(),
            rendered.image.clone(),
            self.config.rendering.texture_filter.to_texture_options(),
        );
        let cached = CachedRender {
            texture,
            image: rendered.image,
            elapsed_ms: rendered.elapsed.as_secs_f64() * 1000.0,
            mode: rendered.mode,
        };
        self.render_cache.insert(key, cached.clone());
        Some(RenderView::from_cached(cached))
    }

    fn central_panel(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        if self.view_mode == ViewMode::Continuous {
            self.render_continuous_panel(&ctx, ui);
            return;
        }

        match (
            self.primary_view.clone(),
            self.compare_enabled,
            self.compare_view.clone(),
        ) {
            (Some(primary), true, Some(compare)) => {
                ui.columns(2, |columns| {
                    self.render_view_panel(
                        &ctx,
                        &mut columns[0],
                        "Primary",
                        self.current_page,
                        &primary,
                        true,
                    );
                    self.render_view_panel(
                        &ctx,
                        &mut columns[1],
                        "Compare",
                        self.current_page,
                        &compare,
                        false,
                    );
                });
            }
            (Some(primary), _, _) => {
                self.render_view_panel(&ctx, ui, "Primary", self.current_page, &primary, true);
            }
            _ => {
                ui.centered_and_justified(|ui| {
                    ui.label("Open a PDF to render it.");
                });
            }
        }
    }

    fn render_continuous_panel(&mut self, ctx: &egui::Context, ui: &mut Ui) {
        let Some(document) = &self.document else {
            ui.centered_and_justified(|ui| ui.label("Open a PDF to render it."));
            return;
        };

        let page_count = document.metadata.page_count;
        let output = ScrollArea::both()
            .id_salt("continuous_document")
            .scroll_offset(self.continuous_scroll_offset)
            .scroll_source(self.scroll_source_for_input(ctx))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let clip_rect = ui.clip_rect();
                let preload_margin = 1200.0;

                for page_index in 0..page_count {
                    let Some(size) = self.page_image_size(page_index) else {
                        continue;
                    };

                    let available_width = ui.available_width().max(size.x);
                    let x_offset = ((available_width - size.x) * 0.5).max(0.0);
                    let (full_row_rect, _) =
                        ui.allocate_exact_size(Vec2::new(available_width, size.y), Sense::hover());
                    let page_rect = Rect::from_min_size(
                        Pos2::new(full_row_rect.left() + x_offset, full_row_rect.top()),
                        size,
                    );

                    let should_render = page_rect.bottom() >= clip_rect.top() - preload_margin
                        && page_rect.top() <= clip_rect.bottom() + preload_margin;

                    ui.painter()
                        .rect_filled(page_rect, 0.0, self.config.background_color());

                    if should_render {
                        if let Some(view) =
                            self.ensure_page_view_cached(ctx, page_index, self.current_preset)
                        {
                            self.render_continuous_page(ctx, ui, page_index, &view, page_rect);
                        } else {
                            ui.painter().text(
                                page_rect.center(),
                                egui::Align2::CENTER_CENTER,
                                "Render failed",
                                egui::TextStyle::Body.resolve(ui.style()),
                                Color32::LIGHT_RED,
                            );
                        }
                    } else {
                        ui.painter().text(
                            page_rect.center_top() + Vec2::new(0.0, 12.0),
                            egui::Align2::CENTER_TOP,
                            format!("Page {}", page_index + 1),
                            egui::TextStyle::Small.resolve(ui.style()),
                            Color32::GRAY,
                        );
                    }

                    ui.add_space(14.0);
                }
            });
        self.continuous_viewport = ScrollViewport {
            offset: output.state.offset,
            size: output.inner_rect.size(),
            content_size: output.content_size,
        };
        self.continuous_scroll_offset = output.state.offset;
    }

    fn render_continuous_page(
        &mut self,
        ctx: &egui::Context,
        ui: &mut Ui,
        page_index: usize,
        view: &RenderView,
        page_rect: Rect,
    ) {
        let response = ui.put(
            page_rect,
            egui::Image::new(&view.texture)
                .fit_to_exact_size(page_rect.size())
                .sense(Sense::click()),
        );

        if self.config.ui.enable_pixel_inspector {
            self.update_inspector_from_response(&response, &view.image);
        }

        self.paint_selectable_text_layer(ui, page_index, &response, &view.image);
        self.paint_tts_highlights(ui, page_index, &response, &view.image);
        self.paint_text_region_highlights(ui, page_index, &response, &view.image);
        self.paint_search_highlights(ui, page_index, &response, &view.image);

        if response.clicked() {
            self.current_page = page_index;
            let focus_rect = response.interact_pointer_pos().map(|pos| {
                screen_pos_to_pdf_rect(
                    pos,
                    response.rect,
                    &view.image,
                    self.document
                        .as_ref()
                        .and_then(|document| document.page_size(page_index)),
                )
            });
            let focus_rect = focus_rect.flatten();
            self.set_tts_cursor_to_nearest_sentence(page_index, focus_rect);
            self.persist_session();
        }

        if response.hovered() && ctx.input(|input| input.raw_scroll_delta.y != 0.0) {
            self.current_page = page_index;
        }
    }

    fn render_view_panel(
        &mut self,
        ctx: &egui::Context,
        ui: &mut Ui,
        label: &str,
        page_index: usize,
        view: &RenderView,
        enable_inspector: bool,
    ) {
        if label != "Continuous" {
            ui.heading(label);
        }
        egui::Frame::default()
            .fill(self.config.background_color())
            .show(ui, |ui| {
                let output = ScrollArea::both()
                    .id_salt("single_page_view")
                    .scroll_offset(self.single_scroll_offset)
                    .scroll_source(self.scroll_source_for_input(ctx))
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        let image_size =
                            Vec2::new(view.image.size[0] as f32, view.image.size[1] as f32);
                        let response = ui.add(
                            egui::Image::new(&view.texture)
                                .fit_to_exact_size(image_size)
                                .sense(Sense::click()),
                        );

                        if enable_inspector && self.config.ui.enable_pixel_inspector {
                            self.update_inspector_from_response(&response, &view.image);
                        }

                        self.paint_selectable_text_layer(ui, page_index, &response, &view.image);
                        self.paint_tts_highlights(ui, page_index, &response, &view.image);
                        self.paint_text_region_highlights(ui, page_index, &response, &view.image);
                        self.paint_search_highlights(ui, page_index, &response, &view.image);

                        if response.clicked() {
                            self.current_page = page_index;
                            let focus_rect = response
                                .interact_pointer_pos()
                                .map(|pos| {
                                    screen_pos_to_pdf_rect(
                                        pos,
                                        response.rect,
                                        &view.image,
                                        self.document
                                            .as_ref()
                                            .and_then(|document| document.page_size(page_index)),
                                    )
                                })
                                .flatten();
                            self.set_tts_cursor_to_nearest_sentence(page_index, focus_rect);
                            self.persist_session();
                        }

                        if response.hovered() && ctx.input(|input| input.raw_scroll_delta.y != 0.0)
                        {
                            self.current_page = page_index;
                        }
                    });
                self.single_scroll_offset = output.state.offset;
                self.single_viewport = ScrollViewport {
                    offset: output.state.offset,
                    size: output.inner_rect.size(),
                    content_size: output.content_size,
                };
            });
    }

    fn paint_search_highlights(
        &self,
        ui: &Ui,
        page_index: usize,
        response: &egui::Response,
        image: &ColorImage,
    ) {
        let Some(document) = &self.document else {
            return;
        };
        let Some(page_size) = document.page_size(page_index) else {
            return;
        };

        for (result_index, hit) in self.search_results.iter().enumerate() {
            if hit.page_index != page_index {
                continue;
            }
            let stroke_color = if Some(result_index) == self.active_search_result {
                Color32::from_rgb(255, 160, 0)
            } else {
                Color32::from_rgb(255, 220, 0)
            };
            for rect in &hit.rects {
                let highlight = pdf_rect_to_screen_rect(*rect, page_size, response.rect, image);
                ui.painter().rect_stroke(
                    highlight,
                    0.0,
                    egui::Stroke::new(2.0, stroke_color),
                    egui::StrokeKind::Outside,
                );
            }
        }
    }

    fn paint_tts_highlights(
        &self,
        ui: &Ui,
        page_index: usize,
        response: &egui::Response,
        image: &ColorImage,
    ) {
        if !self.tts_highlights_enabled {
            return;
        }
        let Some(target) = self.effective_sync_target(self.tts_active_sentence_index) else {
            return;
        };
        if target.page_index != Some(page_index) {
            return;
        }

        let kind = self.tts_highlight_kind(&target);
        let (fill, stroke) = self.tts_highlight_palette(kind);
        let stroke_width = self.config.tts.highlight_stroke_width;

        for highlight in self.tts_highlight_screen_rects(kind, &target, page_index, response, image)
        {
            ui.painter().rect_filled(highlight, 2.0, fill);
            ui.painter().rect_stroke(
                highlight,
                2.0,
                egui::Stroke::new(stroke_width, stroke),
                egui::StrokeKind::Outside,
            );
        }
    }

    fn tts_highlight_kind(&self, target: &SentenceSyncTarget) -> TtsHighlightKind {
        match target.confidence {
            SentenceSyncConfidence::ExactSentence => TtsHighlightKind::ExactRects,
            SentenceSyncConfidence::FuzzySentence => TtsHighlightKind::LineFallback,
            SentenceSyncConfidence::BlockFallback => TtsHighlightKind::BlockFallback,
            SentenceSyncConfidence::PageFallback | SentenceSyncConfidence::Missing => {
                TtsHighlightKind::PageFallback
            }
        }
    }

    fn tts_highlight_palette(&self, kind: TtsHighlightKind) -> (Color32, Color32) {
        let fill = match kind {
            TtsHighlightKind::ExactRects => &self.config.tts.highlight_exact_rgba,
            TtsHighlightKind::LineFallback => &self.config.tts.highlight_fuzzy_rgba,
            TtsHighlightKind::BlockFallback => &self.config.tts.highlight_block_rgba,
            TtsHighlightKind::PageFallback => &self.config.tts.highlight_page_rgba,
        };
        let fill = parse_rgba_hex(fill).unwrap_or(match kind {
            TtsHighlightKind::ExactRects => Color32::from_rgba_premultiplied(255, 120, 80, 56),
            TtsHighlightKind::LineFallback => Color32::from_rgba_premultiplied(255, 176, 80, 48),
            TtsHighlightKind::BlockFallback => Color32::from_rgba_premultiplied(255, 220, 80, 44),
            TtsHighlightKind::PageFallback => Color32::from_rgba_premultiplied(180, 180, 180, 32),
        });
        let stroke = Color32::from_rgba_premultiplied(fill.r(), fill.g(), fill.b(), 255);
        (fill, stroke)
    }

    fn tts_highlight_screen_rects(
        &self,
        kind: TtsHighlightKind,
        target: &SentenceSyncTarget,
        page_index: usize,
        response: &egui::Response,
        image: &ColorImage,
    ) -> Vec<Rect> {
        let Some(document) = &self.document else {
            return Vec::new();
        };
        let Some(page_size) = document.page_size(page_index) else {
            return Vec::new();
        };

        let rects = target
            .rects
            .iter()
            .map(|rect| pdf_rect_to_screen_rect(*rect, page_size, response.rect, image))
            .collect::<Vec<_>>();

        match kind {
            TtsHighlightKind::ExactRects => rects,
            TtsHighlightKind::LineFallback => coalesce_line_rects(&rects),
            TtsHighlightKind::BlockFallback => bounding_rects(&rects).into_iter().collect(),
            TtsHighlightKind::PageFallback => vec![
                response
                    .rect
                    .shrink2(Vec2::splat(self.config.tts.highlight_page_margin)),
            ],
        }
    }

    fn paint_selectable_text_layer(
        &mut self,
        ui: &mut Ui,
        page_index: usize,
        response: &egui::Response,
        image: &ColorImage,
    ) {
        if !self.text_segment_cache.contains_key(&page_index) {
            if let Some(document) = &self.document {
                match document.text_segments_for_page(page_index) {
                    Ok(segments) => {
                        self.text_segment_cache.insert(page_index, segments);
                    }
                    Err(err) => {
                        self.last_error = Some(err.to_string());
                        return;
                    }
                }
            }
        }

        let Some(document) = &self.document else {
            return;
        };
        let Some(page_size) = document.page_size(page_index) else {
            return;
        };
        let Some(segments) = self.text_segment_cache.get(&page_index) else {
            return;
        };

        ui.scope_builder(
            UiBuilder::new()
                .id_salt(("text-layer", page_index))
                .max_rect(response.rect),
            |ui| {
                for (segment_index, segment) in segments.iter().enumerate() {
                    if segment.text.trim().is_empty() {
                        continue;
                    }

                    let segment_rect =
                        pdf_rect_to_screen_rect(segment.rect, page_size, response.rect, image);
                    if segment_rect.width() <= 1.0 || segment_rect.height() <= 1.0 {
                        continue;
                    }

                    ui.scope_builder(
                        UiBuilder::new()
                            .id_salt(("text-segment", page_index, segment_index))
                            .max_rect(segment_rect),
                        |ui| {
                            let label = Label::new(
                                RichText::new(segment.text.clone())
                                    .color(Color32::from_rgba_premultiplied(0, 0, 0, 1)),
                            )
                            .selectable(true);
                            ui.put(segment_rect, label);
                        },
                    );
                }
            },
        );
    }

    fn paint_text_region_highlights(
        &mut self,
        ui: &Ui,
        page_index: usize,
        response: &egui::Response,
        image: &ColorImage,
    ) {
        if !self.highlight_text {
            return;
        }

        if !self.text_rect_cache.contains_key(&page_index) {
            if let Some(document) = &self.document {
                if let Ok(rects) = document.text_rects_for_page(page_index) {
                    self.text_rect_cache.insert(page_index, rects);
                }
            }
        }

        let Some(document) = &self.document else {
            return;
        };
        let Some(page_size) = document.page_size(page_index) else {
            return;
        };
        let Some(rects) = self.text_rect_cache.get(&page_index) else {
            return;
        };

        for rect in rects {
            let highlight = pdf_rect_to_screen_rect(*rect, page_size, response.rect, image);
            ui.painter().rect_filled(
                highlight,
                0.0,
                Color32::from_rgba_premultiplied(90, 170, 255, 24),
            );
        }
    }

    fn page_image_size(&self, page_index: usize) -> Option<Vec2> {
        let document = self.document.as_ref()?;
        let size = document.page_size(page_index)?;
        Some(Vec2::new(
            scaled_page_width(size, self.zoom) as f32,
            scaled_page_height(size, self.zoom) as f32,
        ))
    }

    fn update_inspector_from_response(&mut self, response: &egui::Response, image: &ColorImage) {
        if let Some(pos) = response.hover_pos() {
            let uv_x = ((pos.x - response.rect.left()) / response.rect.width()).clamp(0.0, 1.0);
            let uv_y = ((pos.y - response.rect.top()) / response.rect.height()).clamp(0.0, 1.0);
            let x = ((image.size[0].saturating_sub(1)) as f32 * uv_x).round() as usize;
            let y = ((image.size[1].saturating_sub(1)) as f32 * uv_y).round() as usize;
            let index = y * image.size[0] + x;
            if let Some(color) = image.pixels.get(index) {
                self.pixel_sample = Some(PixelSample {
                    x,
                    y,
                    rgba: [color.r(), color.g(), color.b(), color.a()],
                });
            }
        }
    }

    fn persist_session(&mut self) {
        if !self.config.storage.persist_session {
            return;
        }

        let Some(document) = &self.document else {
            return;
        };

        let session = SessionState {
            last_document: Some(document.metadata.path.clone()),
            last_page: self.current_page,
            zoom: self.zoom,
            preset: self.current_preset.as_str().into(),
            view_mode: match self.view_mode {
                ViewMode::SinglePage => "single".into(),
                ViewMode::Continuous => "continuous".into(),
            },
            compare_enabled: self.compare_enabled,
            compare_preset: self.compare_preset.as_str().into(),
            tts_sentence_id: self.active_sentence().map(|sentence| sentence.id),
            focus_rect: self
                .effective_sync_target(self.tts_active_sentence_index)
                .and_then(|target| target.rects.first().copied()),
            follow_mode: self.tts_follow_mode,
            follow_pin_to_center: self.tts_follow_pin_to_center,
            highlights_enabled: self.tts_highlights_enabled,
        };

        match self.config.session_path() {
            Ok(path) => {
                if let Err(err) =
                    fs::write(&path, toml::to_string_pretty(&session).unwrap_or_default())
                {
                    warn!(error = %err, path = %path.display(), "failed to persist session");
                }
            }
            Err(err) => warn!(error = %err, "failed to resolve session path"),
        }
    }

    fn restore_session(&mut self, ctx: &egui::Context) {
        if !self.config.startup.reopen_last_document_on_launch {
            return;
        }

        let Ok(path) = self.config.session_path() else {
            return;
        };

        let Ok(contents) = fs::read_to_string(&path) else {
            return;
        };

        let Ok(session) = toml::from_str::<SessionState>(&contents) else {
            return;
        };

        self.zoom = session.zoom.clamp(
            self.config.rendering.min_zoom,
            self.config.rendering.max_zoom,
        );
        self.current_preset = RenderPreset::from_name(&session.preset);
        self.view_mode = if session.view_mode == "continuous" {
            ViewMode::Continuous
        } else {
            ViewMode::SinglePage
        };
        self.compare_enabled = session.compare_enabled;
        self.compare_preset = RenderPreset::from_name(&session.compare_preset);
        self.pending_tts_sentence_id = session.tts_sentence_id;
        self.tts_follow_mode = session.follow_mode;
        self.tts_follow_pin_to_center = session.follow_pin_to_center;
        self.tts_highlights_enabled = session.highlights_enabled;

        if let Some(document_path) = session.last_document {
            if document_path.exists() {
                self.open_pdf(ctx, document_path);
                self.current_page = session.last_page;
                self.navigate_to_page(session.last_page, session.focus_rect, ctx);
            }
        }
    }

    fn export_benchmark_snapshot(&self) -> Result<PathBuf> {
        let dir = self.config.benchmark_dir()?;
        let timestamp = unix_timestamp_secs();
        let path = dir.join(format!(
            "{}-{}.csv",
            self.config.storage.benchmark_prefix, timestamp
        ));
        let mut csv = "page,zoom,preset,elapsed_ms,from_cache,mode\n".to_string();

        for metric in &self.render_history {
            csv.push_str(&format!(
                "{},{:.3},{},{:.3},{},{}\n",
                metric.page + 1,
                metric.zoom,
                metric.preset.as_str(),
                metric.elapsed_ms,
                metric.from_cache,
                match metric.mode {
                    RenderMode::FullPage => "full",
                    RenderMode::Thumbnail => "thumbnail",
                    RenderMode::Tiled => "tiled",
                }
            ));
        }

        fs::write(&path, csv)
            .with_context(|| format!("failed to write benchmark snapshot {}", path.display()))?;
        Ok(path)
    }

    fn export_tts_debug_snapshot(&self) -> Result<PathBuf> {
        let analysis = self
            .tts_analysis
            .as_ref()
            .context("TTS analysis is not available for snapshot export")?;
        let active_sentence = self.active_sentence();
        let active_target = self.effective_sync_target(self.tts_active_sentence_index);
        let dir = self.config.tts_artifacts_dir()?.join("debug-snapshots");
        fs::create_dir_all(&dir).with_context(|| format!("failed to create {}", dir.display()))?;
        let timestamp = unix_timestamp_secs();
        let path = dir.join(format!("tts-debug-snapshot-{timestamp}.toml"));

        let snapshot = TtsDebugSnapshot {
            document_path: analysis.source_path.clone(),
            source_fingerprint: analysis.source_fingerprint.clone(),
            active_sentence_index: self.tts_active_sentence_index,
            active_sentence_id: active_sentence.map(|sentence| sentence.id),
            current_page: self.current_page,
            playback_state: self.tts_playback_state.label().into(),
            follow_mode: self.tts_follow_mode,
            follow_pin_to_center: self.tts_follow_pin_to_center,
            highlights_enabled: self.tts_highlights_enabled,
            experimental_sync_enabled: self.tts_experimental_sync_enabled,
            visible_highlight: active_target.as_ref().and_then(|target| {
                target
                    .page_index
                    .map(|page_index| VisibleHighlightSnapshot {
                        page_index,
                        confidence: target.confidence.label().into(),
                        rect_count: target.rects.len(),
                        fallback_reason: target.fallback_reason.clone(),
                    })
            }),
            prepared_sentence_indexes: {
                let mut indexes = self.tts_prepared_clips.keys().copied().collect::<Vec<_>>();
                indexes.sort_unstable();
                indexes
            },
            queued_sentence_indexes: self.tts_prefetch_queue.clone(),
            queued_sync_indexes: self.tts_sync_queue.clone(),
            active_sync_target: active_target,
            sentence_plan: analysis
                .sentences
                .iter()
                .map(|sentence| SentenceDebugSummary {
                    id: sentence.id,
                    page_start: sentence.page_range.start_page,
                    page_end: sentence.page_range.end_page,
                    unit_kind: format!("{:?}", sentence.unit_kind),
                    char_start: sentence.range.start,
                    char_end: sentence.range.end,
                })
                .collect(),
            sync_targets: self.tts_sync_targets.values().cloned().collect::<Vec<_>>(),
            performance: TtsDebugPerformanceSnapshot::from_profile(&self.tts_profile),
        };

        fs::write(&path, toml::to_string_pretty(&snapshot)?)
            .with_context(|| format!("failed to write {}", path.display()))?;
        info!(path = %path.display(), "exported TTS debug snapshot");
        Ok(path)
    }

    fn record_metric(
        &mut self,
        elapsed_ms: f64,
        from_cache: bool,
        mode: RenderMode,
        preset: RenderPreset,
    ) {
        self.render_history.push(RenderMetric {
            page: self.current_page,
            zoom: self.zoom,
            preset,
            elapsed_ms,
            from_cache,
            mode,
        });

        if self.render_history.len() > 512 {
            let drain = self.render_history.len() - 512;
            self.render_history.drain(0..drain);
        }
    }
}

impl eframe::App for PdfizerApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        ctx.set_pixels_per_point(1.0);
        self.poll_tts_analysis();
        self.poll_playback_events();
        self.advance_tts_clock(ctx);
        self.handle_shortcuts(ctx);
        self.process_tiled_jobs(ctx);

        egui::TopBottomPanel::top("toolbar").show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| self.top_bar(ctx, ui));
        });

        egui::SidePanel::left("study_panel")
            .resizable(true)
            .default_width(self.config.ui.left_panel_width)
            .show(ctx, |ui| self.side_panel(ctx, ui));

        if self.config.ui.show_thumbnails {
            egui::SidePanel::right("thumbnail_panel")
                .resizable(true)
                .default_width(self.config.ui.thumbnail_panel_width)
                .show(ctx, |ui| self.thumbnail_panel(ctx, ui));
        }

        egui::TopBottomPanel::bottom("tts_player_bar")
            .resizable(false)
            .show_separator_line(true)
            .show(ctx, |ui| self.tts_player_bar(ctx, ui));

        egui::TopBottomPanel::bottom("instrumentation_panel")
            .resizable(true)
            .default_height(self.config.ui.bottom_panel_height)
            .show(ctx, |ui| self.bottom_panel(ctx, ui));

        egui::CentralPanel::default().show(ctx, |ui| self.central_panel(ui));
    }
}

impl Drop for PdfizerApp {
    fn drop(&mut self) {
        if let Some(sender) = &self.tts_playback_tx {
            let _ = sender.send(PlaybackCommand::Shutdown);
        }
    }
}

fn playback_worker_loop(command_rx: Receiver<PlaybackCommand>, event_tx: Sender<PlaybackEvent>) {
    let mut output: Option<MixerDeviceSink> = None;
    let mut player: Option<Player> = None;

    while let Ok(command) = command_rx.recv() {
        match command {
            PlaybackCommand::Play {
                command_id,
                cancel_token,
                audio_path,
                volume,
                rate,
            } => {
                let result = (|| -> Result<()> {
                    if output.is_none() || player.is_none() {
                        let sink = DeviceSinkBuilder::open_default_sink()
                            .context("failed to open default audio output device")?;
                        let playback = Player::connect_new(sink.mixer());
                        output = Some(sink);
                        player = Some(playback);
                    }

                    let player = player
                        .as_ref()
                        .context("audio player was not initialized")?;
                    player.clear();
                    player.stop();
                    player.set_volume(volume);
                    player.set_speed(rate);

                    if let Some(audio_path) = &audio_path {
                        let file = fs::File::open(audio_path)
                            .with_context(|| format!("failed to open {}", audio_path.display()))?;
                        let decoder = Decoder::try_from(file).with_context(|| {
                            format!("failed to decode {}", audio_path.display())
                        })?;
                        player.append(decoder);
                    }

                    player.play();
                    Ok(())
                })();

                let _ = match result {
                    Ok(()) => event_tx.send(PlaybackEvent::Ack {
                        command_id,
                        cancel_token,
                        state: PlaybackWorkerState::Playing,
                    }),
                    Err(err) => event_tx.send(PlaybackEvent::Failed {
                        command_id,
                        cancel_token,
                        error: err.to_string(),
                    }),
                };
            }
            PlaybackCommand::Pause {
                command_id,
                cancel_token,
            } => {
                if let Some(player) = &player {
                    player.pause();
                }
                let _ = event_tx.send(PlaybackEvent::Ack {
                    command_id,
                    cancel_token,
                    state: PlaybackWorkerState::Paused,
                });
            }
            PlaybackCommand::Resume {
                command_id,
                cancel_token,
            } => {
                if let Some(player) = &player {
                    player.play();
                }
                let _ = event_tx.send(PlaybackEvent::Ack {
                    command_id,
                    cancel_token,
                    state: PlaybackWorkerState::Playing,
                });
            }
            PlaybackCommand::Stop {
                command_id,
                cancel_token,
            } => {
                if let Some(player) = &player {
                    player.clear();
                    player.stop();
                }
                let _ = event_tx.send(PlaybackEvent::Ack {
                    command_id,
                    cancel_token,
                    state: PlaybackWorkerState::Stopped,
                });
            }
            PlaybackCommand::Shutdown => {
                if let Some(player) = &player {
                    player.clear();
                    player.stop();
                }
                break;
            }
        }
    }
}

#[derive(Clone)]
struct RenderView {
    texture: TextureHandle,
    image: ColorImage,
    elapsed_ms: f64,
    mode: RenderMode,
}

impl RenderView {
    fn from_cached(cached: CachedRender) -> Self {
        Self {
            texture: cached.texture,
            image: cached.image,
            elapsed_ms: cached.elapsed_ms,
            mode: cached.mode,
        }
    }
}

#[derive(Clone)]
struct CachedRender {
    texture: TextureHandle,
    image: ColorImage,
    elapsed_ms: f64,
    mode: RenderMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct RenderCacheKey {
    page: usize,
    zoom_bucket: i32,
    preset: RenderPreset,
    slot: ViewSlot,
}

impl RenderCacheKey {
    fn new(page: usize, zoom: f32, preset: RenderPreset, slot: ViewSlot) -> Self {
        Self {
            page,
            zoom_bucket: (zoom * 100.0).round() as i32,
            preset,
            slot,
        }
    }

    fn texture_name(&self) -> String {
        format!(
            "page-{}-{}-{}-{}",
            self.page,
            self.zoom_bucket,
            self.preset.as_str(),
            match self.slot {
                ViewSlot::Primary => "primary",
                ViewSlot::Compare => "compare",
            }
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct ThumbnailCacheKey {
    page: usize,
    preset: RenderPreset,
    size: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum ViewSlot {
    Primary,
    Compare,
}

#[derive(Debug, Clone)]
struct TiledRenderJob {
    key: RenderCacheKey,
    preset: RenderPreset,
    full_width: i32,
    tiles: Vec<TileSpec>,
    next_tile: usize,
    composite: ColorImage,
    elapsed_ms: f64,
}

#[derive(Debug, Clone, Copy)]
struct TileSpec {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
}

#[derive(Debug, Clone, Copy)]
struct RenderMetric {
    page: usize,
    zoom: f32,
    preset: RenderPreset,
    elapsed_ms: f64,
    from_cache: bool,
    mode: RenderMode,
}

#[derive(Debug, Clone, Copy, Default)]
struct MetricSummary {
    count: usize,
    average_ms: f64,
    min_ms: f64,
    max_ms: f64,
}

#[derive(Debug, Clone, Copy)]
struct PixelSample {
    x: usize,
    y: usize,
    rgba: [u8; 4],
}

#[derive(Debug, Clone, Default)]
struct TtsPerformanceProfile {
    prepare: RollingStat,
    sync: RollingStat,
    activation: RollingStat,
    prepare_cache_hits: u64,
    playback_underruns: u64,
    stale_worker_results: u64,
    cancelled_worker_results: u64,
    cancelled_playback_events: u64,
    scheduler_queue_peak: u64,
    starvation_signals: u64,
    playback_worker_failures: u64,
    wrong_page_rejects: u64,
    distant_geometry_rejects: u64,
    unmappable_sentences: u64,
    exact_sync_count: u64,
    fuzzy_sync_count: u64,
    block_sync_count: u64,
    page_sync_count: u64,
    missing_sync_count: u64,
}

impl TtsPerformanceProfile {
    fn record_sync_confidence(&mut self, confidence: SentenceSyncConfidence) {
        match confidence {
            SentenceSyncConfidence::ExactSentence => self.exact_sync_count += 1,
            SentenceSyncConfidence::FuzzySentence => self.fuzzy_sync_count += 1,
            SentenceSyncConfidence::BlockFallback => self.block_sync_count += 1,
            SentenceSyncConfidence::PageFallback => self.page_sync_count += 1,
            SentenceSyncConfidence::Missing => self.missing_sync_count += 1,
        }
    }

    fn record_sync_failure(&mut self, wrong_page: bool, distant_geometry: bool, unmappable: bool) {
        if wrong_page {
            self.wrong_page_rejects += 1;
        }
        if distant_geometry {
            self.distant_geometry_rejects += 1;
        }
        if unmappable {
            self.unmappable_sentences += 1;
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct RollingStat {
    samples: u64,
    total_ms: f64,
    max_ms: f64,
}

impl RollingStat {
    fn record(&mut self, elapsed_ms: f64) {
        self.samples += 1;
        self.total_ms += elapsed_ms;
        self.max_ms = self.max_ms.max(elapsed_ms);
    }

    fn average_ms(&self) -> f64 {
        if self.samples == 0 {
            0.0
        } else {
            self.total_ms / self.samples as f64
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ViewMode {
    SinglePage,
    Continuous,
}

impl ViewMode {
    fn label(&self) -> &'static str {
        match self {
            Self::SinglePage => "Single",
            Self::Continuous => "Continuous",
        }
    }
}

#[derive(Debug, Clone)]
enum TtsAnalysisStatus {
    Disabled,
    Idle,
    Analyzing,
    Ready,
    Failed(String),
}

impl TtsAnalysisStatus {
    fn label(&self) -> String {
        match self {
            Self::Disabled => "disabled".into(),
            Self::Idle => "idle".into(),
            Self::Analyzing => "analyzing".into(),
            Self::Ready => "ready".into(),
            Self::Failed(message) => format!("failed: {message}"),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
struct SessionState {
    last_document: Option<PathBuf>,
    last_page: usize,
    zoom: f32,
    preset: String,
    view_mode: String,
    compare_enabled: bool,
    compare_preset: String,
    tts_sentence_id: Option<u64>,
    #[serde(default)]
    focus_rect: Option<PdfRectData>,
    #[serde(default = "default_true")]
    follow_mode: bool,
    #[serde(default = "default_true")]
    follow_pin_to_center: bool,
    #[serde(default = "default_true")]
    highlights_enabled: bool,
}

#[derive(Debug, Clone, Serialize)]
struct TtsDebugSnapshot {
    document_path: PathBuf,
    source_fingerprint: String,
    active_sentence_index: usize,
    active_sentence_id: Option<u64>,
    current_page: usize,
    playback_state: String,
    follow_mode: bool,
    follow_pin_to_center: bool,
    highlights_enabled: bool,
    experimental_sync_enabled: bool,
    visible_highlight: Option<VisibleHighlightSnapshot>,
    prepared_sentence_indexes: Vec<usize>,
    queued_sentence_indexes: Vec<usize>,
    queued_sync_indexes: Vec<usize>,
    active_sync_target: Option<SentenceSyncTarget>,
    sentence_plan: Vec<SentenceDebugSummary>,
    sync_targets: Vec<SentenceSyncTarget>,
    performance: TtsDebugPerformanceSnapshot,
}

#[derive(Debug, Clone, Serialize)]
struct VisibleHighlightSnapshot {
    page_index: usize,
    confidence: String,
    rect_count: usize,
    fallback_reason: String,
}

#[derive(Debug, Clone, Serialize)]
struct SentenceDebugSummary {
    id: u64,
    page_start: usize,
    page_end: usize,
    unit_kind: String,
    char_start: usize,
    char_end: usize,
}

#[derive(Debug, Clone, Serialize)]
struct TtsDebugPerformanceSnapshot {
    prepare_cache_hits: u64,
    playback_underruns: u64,
    stale_worker_results: u64,
    cancelled_worker_results: u64,
    cancelled_playback_events: u64,
    scheduler_queue_peak: u64,
    starvation_signals: u64,
    playback_worker_failures: u64,
    wrong_page_rejects: u64,
    distant_geometry_rejects: u64,
    unmappable_sentences: u64,
}

impl TtsDebugPerformanceSnapshot {
    fn from_profile(profile: &TtsPerformanceProfile) -> Self {
        Self {
            prepare_cache_hits: profile.prepare_cache_hits,
            playback_underruns: profile.playback_underruns,
            stale_worker_results: profile.stale_worker_results,
            cancelled_worker_results: profile.cancelled_worker_results,
            cancelled_playback_events: profile.cancelled_playback_events,
            scheduler_queue_peak: profile.scheduler_queue_peak,
            starvation_signals: profile.starvation_signals,
            playback_worker_failures: profile.playback_worker_failures,
            wrong_page_rejects: profile.wrong_page_rejects,
            distant_geometry_rejects: profile.distant_geometry_rejects,
            unmappable_sentences: profile.unmappable_sentences,
        }
    }
}

fn render_metadata(ui: &mut Ui, metadata: &PdfMetadata) {
    ui.heading("Document");
    ui.label(RichText::new(metadata.path.display().to_string()).monospace());
    ui.label(format!("Pages: {}", metadata.page_count));

    if let Some(version) = &metadata.version {
        ui.label(format!("PDF version: {version}"));
    }

    render_optional_row(ui, "Title", metadata.title.as_deref());
    render_optional_row(ui, "Author", metadata.author.as_deref());
    render_optional_row(ui, "Subject", metadata.subject.as_deref());
    render_optional_row(ui, "Keywords", metadata.keywords.as_deref());
}

fn render_optional_row(ui: &mut Ui, label: &str, value: Option<&str>) {
    if let Some(value) = value.filter(|value| !value.trim().is_empty()) {
        ui.label(format!("{label}: {value}"));
    }
}

fn render_profiles(ui: &mut Ui, app: &mut PdfizerApp, ctx: &egui::Context) {
    ui.heading("Experiment Profiles");

    if ui.button("Study Balanced").clicked() {
        app.current_preset = RenderPreset::Balanced;
        app.compare_enabled = false;
        app.render_current_page(ctx);
    }

    if ui.button("Compare Crisp").clicked() {
        app.current_preset = RenderPreset::Balanced;
        app.compare_enabled = true;
        app.compare_preset = RenderPreset::Crisp;
        app.render_current_page(ctx);
    }

    if ui.button("Fast QA").clicked() {
        app.current_preset = RenderPreset::Fast;
        app.compare_enabled = true;
        app.compare_preset = RenderPreset::Grayscale;
        app.render_current_page(ctx);
    }
}

fn pdfium_help_text(config: &AppConfig) -> String {
    let env_var = &config.pdfium.library_env_var;
    warn!("Pdfium is not available yet; the UI will remain in help mode");
    format!(
        "Pdfium failed to bind. Use Load Pdfium, set {env_var}, or set pdfium.library_path in config/pdfizer.toml."
    )
}

fn scaled_page_width(size: PageSizePoints, zoom: f32) -> i32 {
    ((size.width * zoom).round() as i32).max(1)
}

fn scaled_page_height(size: PageSizePoints, zoom: f32) -> i32 {
    ((size.height * zoom).round() as i32).max(1)
}

fn centered_offset(target: f32, viewport_size: f32) -> f32 {
    if viewport_size <= 0.0 {
        return target.max(0.0);
    }
    (target - viewport_size * 0.5).max(0.0)
}

fn viewport_contains_target(
    offset: Vec2,
    viewport_size: Vec2,
    target: Vec2,
    margin_ratio: f32,
) -> bool {
    axis_contains_target(offset.x, viewport_size.x, target.x, margin_ratio)
        && axis_contains_target(offset.y, viewport_size.y, target.y, margin_ratio)
}

fn axis_contains_target(offset: f32, viewport_size: f32, target: f32, margin_ratio: f32) -> bool {
    if viewport_size <= 0.0 {
        return false;
    }
    let margin = (viewport_size * margin_ratio).clamp(24.0, viewport_size * 0.45);
    let min = offset + margin;
    let max = offset + viewport_size - margin;
    target >= min && target <= max
}

fn parse_rgba_hex(value: &str) -> Option<Color32> {
    let hex = value.trim().trim_start_matches('#');
    let bytes = match hex.len() {
        6 => u32::from_str_radix(hex, 16).ok().map(|value| {
            [
                ((value >> 16) & 0xff) as u8,
                ((value >> 8) & 0xff) as u8,
                (value & 0xff) as u8,
                0xff,
            ]
        }),
        8 => u32::from_str_radix(hex, 16).ok().map(|value| {
            [
                ((value >> 24) & 0xff) as u8,
                ((value >> 16) & 0xff) as u8,
                ((value >> 8) & 0xff) as u8,
                (value & 0xff) as u8,
            ]
        }),
        _ => None,
    }?;
    Some(Color32::from_rgba_premultiplied(
        bytes[0], bytes[1], bytes[2], bytes[3],
    ))
}

fn bounding_rects(rects: &[Rect]) -> Option<Rect> {
    let first = *rects.first()?;
    Some(rects.iter().skip(1).fold(first, |acc, rect| {
        Rect::from_min_max(
            Pos2::new(acc.left().min(rect.left()), acc.top().min(rect.top())),
            Pos2::new(
                acc.right().max(rect.right()),
                acc.bottom().max(rect.bottom()),
            ),
        )
    }))
}

fn coalesce_line_rects(rects: &[Rect]) -> Vec<Rect> {
    if rects.is_empty() {
        return Vec::new();
    }

    let mut sorted = rects.to_vec();
    sorted.sort_by(|left, right| {
        left.center()
            .y
            .partial_cmp(&right.center().y)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut lines: Vec<Rect> = Vec::new();
    for rect in sorted {
        if let Some(existing) = lines.last_mut() {
            let tolerance = existing.height().max(rect.height()) * 0.6;
            if (existing.center().y - rect.center().y).abs() <= tolerance {
                *existing = Rect::from_min_max(
                    Pos2::new(
                        existing.left().min(rect.left()),
                        existing.top().min(rect.top()),
                    ),
                    Pos2::new(
                        existing.right().max(rect.right()),
                        existing.bottom().max(rect.bottom()),
                    ),
                );
                continue;
            }
        }
        lines.push(rect);
    }
    lines
}

fn rect_distance_score(left: PdfRectData, right: PdfRectData) -> f32 {
    let left_center_x = (left.left + left.right) * 0.5;
    let left_center_y = (left.top + left.bottom) * 0.5;
    let right_center_x = (right.left + right.right) * 0.5;
    let right_center_y = (right.top + right.bottom) * 0.5;
    let dx = left_center_x - right_center_x;
    let dy = left_center_y - right_center_y;
    (dx * dx + dy * dy).sqrt()
}

fn screen_pos_to_pdf_rect(
    pos: Pos2,
    response_rect: Rect,
    _image: &ColorImage,
    page_size: Option<PageSizePoints>,
) -> Option<PdfRectData> {
    let page_size = page_size?;
    if response_rect.width() <= 0.0 || response_rect.height() <= 0.0 {
        return None;
    }

    let u = ((pos.x - response_rect.left()) / response_rect.width()).clamp(0.0, 1.0);
    let v = ((pos.y - response_rect.top()) / response_rect.height()).clamp(0.0, 1.0);
    let x = page_size.width * u;
    let y = page_size.height * (1.0 - v);
    let pad_x = page_size.width * 0.01;
    let pad_y = page_size.height * 0.01;

    Some(PdfRectData {
        left: (x - pad_x).max(0.0),
        right: (x + pad_x).min(page_size.width),
        top: (y + pad_y).min(page_size.height),
        bottom: (y - pad_y).max(0.0),
    })
}

fn pdf_rect_to_screen_rect(
    rect: PdfRectData,
    page_size: PageSizePoints,
    image_rect: Rect,
    _image: &ColorImage,
) -> Rect {
    let left = image_rect.left() + image_rect.width() * (rect.left / page_size.width);
    let right = image_rect.left() + image_rect.width() * (rect.right / page_size.width);
    let top = image_rect.top() + image_rect.height() * (1.0 - (rect.top / page_size.height));
    let bottom = image_rect.top() + image_rect.height() * (1.0 - (rect.bottom / page_size.height));
    Rect::from_min_max(Pos2::new(left, top), Pos2::new(right, bottom))
}

fn build_tiles(full_width: i32, full_height: i32, tile_size: i32) -> Vec<TileSpec> {
    let mut tiles = Vec::new();
    let mut y = 0;
    while y < full_height {
        let mut x = 0;
        while x < full_width {
            let width = (full_width - x).min(tile_size);
            let height = (full_height - y).min(tile_size);
            tiles.push(TileSpec {
                x,
                y,
                width,
                height,
            });
            x += tile_size;
        }
        y += tile_size;
    }
    tiles
}

fn blit_tile(destination: &mut ColorImage, source: &ColorImage, x: i32, y: i32) {
    for row in 0..source.size[1] {
        for col in 0..source.size[0] {
            let dst_x = x as usize + col;
            let dst_y = y as usize + row;
            if dst_x < destination.size[0] && dst_y < destination.size[1] {
                let dst_index = dst_y * destination.size[0] + dst_x;
                let src_index = row * source.size[0] + col;
                destination.pixels[dst_index] = source.pixels[src_index];
            }
        }
    }
}

fn metric_summary(metrics: &[RenderMetric]) -> MetricSummary {
    if metrics.is_empty() {
        return MetricSummary::default();
    }

    let count = metrics.len();
    let sum: f64 = metrics.iter().map(|metric| metric.elapsed_ms).sum();
    let min_ms = metrics
        .iter()
        .map(|metric| metric.elapsed_ms)
        .fold(f64::INFINITY, f64::min);
    let max_ms = metrics
        .iter()
        .map(|metric| metric.elapsed_ms)
        .fold(f64::NEG_INFINITY, f64::max);

    MetricSummary {
        count,
        average_ms: sum / count as f64,
        min_ms,
        max_ms,
    }
}

fn unix_timestamp_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn default_true() -> bool {
    true
}
