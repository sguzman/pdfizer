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
        self, PdfTtsMode, PreparedSentenceClip, TtsAnalysisArtifacts, TtsEngineKind,
        TtsPlaybackState, TtsWorkerMessage,
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
    tts_worker_tx: Option<Sender<TtsWorkerMessage>>,
    tts_worker_rx: Option<Receiver<TtsWorkerMessage>>,
    tts_request_id: u64,
    tts_engine: TtsEngineKind,
    tts_playback_state: TtsPlaybackState,
    tts_active_sentence_index: usize,
    tts_follow_mode: bool,
    tts_prepared_clips: HashMap<usize, PreparedSentenceClip>,
    tts_prefetch_queue: Vec<usize>,
    tts_failed_prefetch: HashMap<usize, String>,
    tts_prefetch_request_id: u64,
    tts_prefetch_in_flight: usize,
    tts_started_at: Option<Instant>,
    tts_elapsed_before_pause: Duration,
    tts_current_duration: Duration,
    tts_output: Option<MixerDeviceSink>,
    tts_player: Option<Player>,
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

    pub fn new(cc: &eframe::CreationContext<'_>, config: AppConfig) -> Self {
        let tts_enabled = config.tts.enabled;
        let tts_engine = TtsEngineKind::from_name(&config.tts.engine);
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
            tts_worker_tx: None,
            tts_worker_rx: None,
            tts_request_id: 0,
            tts_engine,
            tts_playback_state: TtsPlaybackState::Stopped,
            tts_active_sentence_index: 0,
            tts_follow_mode: true,
            tts_prepared_clips: HashMap::new(),
            tts_prefetch_queue: Vec::new(),
            tts_failed_prefetch: HashMap::new(),
            tts_prefetch_request_id: 0,
            tts_prefetch_in_flight: 0,
            tts_started_at: None,
            tts_elapsed_before_pause: Duration::ZERO,
            tts_current_duration: Duration::ZERO,
            tts_output: None,
            tts_player: None,
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
                self.reset_tts_runtime();
                self.single_scroll_offset = Vec2::ZERO;
                self.continuous_scroll_offset = Vec2::ZERO;
                self.render_current_page(ctx);
                self.start_tts_analysis(path);
                self.persist_session();
            }
            Err(err) => {
                error!(path = %path.display(), error = %err, "failed to open PDF");
                self.last_error = Some(err.to_string());
            }
        }
    }

    fn start_tts_analysis(&mut self, path: PathBuf) {
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

        thread::spawn(move || {
            let message = match tts::analyze_pdf_for_tts(&config, &path) {
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
        self.start_tts_analysis(path);
        self.status_message = Some("Rebuilding TTS analysis".into());
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
                    if artifacts.mode != PdfTtsMode::HighTextTrust
                        && self.config.tts.verbose_degraded_logging
                    {
                        warn!(
                            path = %artifacts.source_path.display(),
                            mode = %artifacts.mode.label(),
                            confidence = artifacts.confidence,
                            "PDF TTS analysis completed in degraded mode"
                        );
                    }
                    self.status_message = Some(format!(
                        "TTS analysis ready: {} sentences ({})",
                        artifacts.sentences.len(),
                        artifacts.mode.label()
                    ));
                    self.tts_analysis = Some(artifacts);
                    self.tts_analysis_status = TtsAnalysisStatus::Ready;
                    self.sync_tts_sentence_to_current_page();
                }
                Ok(TtsWorkerMessage::Failed { request_id, error })
                    if request_id == self.tts_request_id =>
                {
                    error!(error = %error, "TTS analysis failed");
                    self.last_error = Some(format!("TTS analysis failed: {error}"));
                    self.tts_analysis_status = TtsAnalysisStatus::Failed(error);
                }
                Ok(TtsWorkerMessage::PrefetchCompleted { request_id, clip })
                    if request_id == self.tts_prefetch_request_id =>
                {
                    self.tts_prepared_clips.insert(clip.sentence_index, clip);
                    self.tts_prefetch_queue
                        .retain(|index| !self.tts_prepared_clips.contains_key(index));
                    self.tts_prefetch_in_flight = self.tts_prefetch_in_flight.saturating_sub(1);
                }
                Ok(TtsWorkerMessage::PrefetchFailed {
                    request_id,
                    sentence_index,
                    error,
                }) if request_id == self.tts_prefetch_request_id => {
                    self.tts_prefetch_queue
                        .retain(|index| *index != sentence_index);
                    self.tts_prefetch_in_flight = self.tts_prefetch_in_flight.saturating_sub(1);
                    self.tts_failed_prefetch
                        .insert(sentence_index, error.clone());
                    warn!(sentence_index, error = %error, "TTS prefetch failed");
                }
                Ok(_) => {}
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
        self.tts_playback_state = TtsPlaybackState::Stopped;
        self.tts_active_sentence_index = 0;
        self.tts_prepared_clips.clear();
        self.tts_prefetch_queue.clear();
        self.tts_failed_prefetch.clear();
        self.tts_prefetch_in_flight = 0;
        self.tts_started_at = None;
        self.tts_elapsed_before_pause = Duration::ZERO;
        self.tts_current_duration = Duration::ZERO;
        self.tts_prefetch_request_id = self.tts_prefetch_request_id.wrapping_add(1);
        if let Some(player) = &self.tts_player {
            player.clear();
            player.stop();
        }
    }

    fn active_sentence(&self) -> Option<&crate::tts::SentencePlan> {
        self.tts_analysis
            .as_ref()
            .and_then(|analysis| analysis.sentences.get(self.tts_active_sentence_index))
    }

    fn sync_tts_sentence_to_current_page(&mut self) {
        let Some(analysis) = &self.tts_analysis else {
            return;
        };
        if let Some((index, _)) = analysis.sentences.iter().enumerate().find(|(_, sentence)| {
            sentence.page_range.start_page <= self.current_page
                && sentence.page_range.end_page >= self.current_page
        }) {
            self.tts_active_sentence_index = index;
        }
    }

    fn start_tts_prefetch(&mut self) {
        let Some(analysis) = self.tts_analysis.clone() else {
            return;
        };

        self.tts_prefetch_request_id = self.tts_prefetch_request_id.wrapping_add(1);
        let request_id = self.tts_prefetch_request_id;
        let plan = tts::build_prefetch_plan(
            &analysis,
            self.tts_active_sentence_index,
            self.config.tts.sentence_prefetch,
        );

        self.tts_prefetch_queue = plan
            .iter()
            .copied()
            .filter(|index| !self.tts_prepared_clips.contains_key(index))
            .collect();
        self.tts_failed_prefetch.clear();
        self.tts_prefetch_in_flight = self.tts_prefetch_queue.len();

        if self.tts_prefetch_queue.is_empty() {
            return;
        }

        let sender = self.ensure_tts_worker_channel();
        for sentence_index in self.tts_prefetch_queue.clone() {
            let config = self.config.clone();
            let analysis = analysis.clone();
            let sender = sender.clone();
            let engine = self.tts_engine;
            thread::spawn(move || {
                let message =
                    match tts::prepare_sentence_clip(&config, &analysis, sentence_index, engine) {
                        Ok(clip) => TtsWorkerMessage::PrefetchCompleted { request_id, clip },
                        Err(err) => TtsWorkerMessage::PrefetchFailed {
                            request_id,
                            sentence_index,
                            error: err.to_string(),
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
            self.last_error = Some("TTS analysis is not ready yet".into());
            return;
        }

        self.tts_playback_state = TtsPlaybackState::Preparing;
        self.start_tts_prefetch();
        self.try_activate_current_sentence(ctx);
    }

    fn pause_tts_playback(&mut self) {
        if self.tts_playback_state == TtsPlaybackState::Playing {
            if let Some(started_at) = self.tts_started_at.take() {
                self.tts_elapsed_before_pause += started_at.elapsed();
            }
            if let Some(player) = &self.tts_player {
                player.pause();
            }
            self.tts_playback_state = TtsPlaybackState::Paused;
        }
    }

    fn resume_tts_playback(&mut self, ctx: &egui::Context) {
        if self.tts_playback_state == TtsPlaybackState::Paused {
            if let Some(player) = &self.tts_player {
                player.play();
            }
            self.tts_started_at = Some(Instant::now());
            self.tts_playback_state = TtsPlaybackState::Playing;
            self.focus_active_sentence(ctx);
            ctx.request_repaint_after(Duration::from_millis(40));
        }
    }

    fn stop_tts_playback(&mut self) {
        self.reset_tts_runtime();
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
            self.tts_active_sentence_index += 1;
            self.tts_started_at = None;
            self.start_tts_prefetch();
            self.try_activate_current_sentence(ctx);
        } else {
            self.stop_tts_playback();
        }
    }

    fn previous_tts_sentence(&mut self, ctx: &egui::Context) {
        if self.tts_active_sentence_index > 0 {
            self.tts_active_sentence_index -= 1;
            self.tts_started_at = None;
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
            if let Err(err) = self.play_prepared_clip(&clip) {
                self.last_error = Some(format!("TTS playback failed: {err}"));
                self.tts_playback_state = TtsPlaybackState::Failed;
                return;
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
            ctx.request_repaint_after(Duration::from_millis(40));
        }
    }

    fn focus_active_sentence(&mut self, ctx: &egui::Context) {
        if !self.tts_follow_mode {
            return;
        }
        let Some(sentence) = self.active_sentence() else {
            return;
        };
        self.navigate_to_page(sentence.page_range.start_page, None, ctx);
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
        self.tts_active_sentence_index = target_sentence_index;
        self.tts_started_at = None;
        self.tts_elapsed_before_pause = Duration::ZERO;
        self.start_tts_prefetch();
        self.try_activate_current_sentence(ctx);
    }

    fn ensure_audio_player(&mut self) -> Result<&Player> {
        if self.tts_player.is_none() || self.tts_output.is_none() {
            let output = DeviceSinkBuilder::open_default_sink()
                .context("failed to open default audio output device")?;
            let player = Player::connect_new(output.mixer());
            self.tts_output = Some(output);
            self.tts_player = Some(player);
        }

        self.tts_player
            .as_ref()
            .context("audio player was not initialized")
    }

    fn play_prepared_clip(&mut self, clip: &PreparedSentenceClip) -> Result<()> {
        let volume = self.config.tts.volume;
        let rate = self.config.tts.rate;
        let player = self.ensure_audio_player()?;
        player.clear();
        player.stop();
        player.set_volume(volume);
        player.set_speed(rate);

        if let Some(audio_path) = &clip.audio_path {
            let file = fs::File::open(audio_path)
                .with_context(|| format!("failed to open {}", audio_path.display()))?;
            let decoder = Decoder::try_from(file)
                .with_context(|| format!("failed to decode {}", audio_path.display()))?;
            player.append(decoder);
        }

        player.play();
        Ok(())
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
        ui.label("TTS");
        if ui
            .add_enabled(
                self.tts_analysis.is_some(),
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
                self.tts_analysis.is_some() && self.tts_playback_state != TtsPlaybackState::Stopped,
                egui::Button::new("Stop"),
            )
            .clicked()
        {
            self.stop_tts_playback();
        }
        if ui
            .add_enabled(
                self.tts_analysis.is_some() && self.tts_active_sentence_index > 0,
                egui::Button::new("Prev sentence"),
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
                egui::Button::new("Next sentence"),
            )
            .clicked()
        {
            self.next_tts_sentence(ctx);
        }
        ui.checkbox(&mut self.tts_follow_mode, "Follow");
        if let Some(analysis) = &self.tts_analysis {
            let mut target_sentence = self.tts_active_sentence_index;
            let response = ui.add(
                egui::DragValue::new(&mut target_sentence)
                    .speed(1.0)
                    .range(0..=analysis.sentences.len().saturating_sub(1))
                    .prefix("Sentence "),
            );
            if response.changed() {
                self.seek_tts_sentence(target_sentence, ctx);
            }
        }

        if let Some(document) = &self.document {
            ui.separator();
            ui.label(format!(
                "Page {}/{}",
                self.current_page + 1,
                document.metadata.page_count
            ));
        }
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
                "Playback: {} | engine: {} | active sentence: {} | follow: {}",
                self.tts_playback_state.label(),
                self.tts_engine.label(),
                self.tts_active_sentence_index + 1,
                self.tts_follow_mode
            ));

            if let Some(analysis) = &self.tts_analysis {
                ui.label(format!(
                    "Mode: {} ({:.0}% confidence)",
                    analysis.mode.label(),
                    analysis.confidence * 100.0
                ));
                ui.label(format!(
                    "Sentences: {} | chars: {} | text pages: {} / {}",
                    analysis.sentences.len(),
                    analysis.stats.normalized_chars,
                    analysis.stats.pages_with_text,
                    analysis.pages.len()
                ));
                ui.label(format!(
                    "Normalization: ligatures {} | soft hyphens {} | duplicate lines {} | repeated edge lines {}",
                    analysis.stats.ligatures_replaced,
                    analysis.stats.soft_hyphens_removed,
                    analysis.stats.duplicate_lines_removed,
                    analysis.stats.repeated_edge_lines_removed
                ));
                if let Some(path) = &analysis.artifact_path {
                    ui.label(format!("Artifact: {}", path.display()));
                }
                if let Some(sentence) = self.active_sentence() {
                    let mut sentence_text = sentence.text.clone();
                    ui.label(format!(
                        "Sentence page range: {}-{}",
                        sentence.page_range.start_page + 1,
                        sentence.page_range.end_page + 1
                    ));
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
                "Prepared clips: {} | queued: {} | failed: {} | in flight: {} | prefetch request: {}",
                snapshot.prepared_sentence_indexes.len(),
                snapshot.queued_sentence_indexes.len(),
                snapshot.failed_sentence_indexes.len(),
                self.tts_prefetch_in_flight,
                snapshot.request_id
            ));

            if ui
                .add_enabled(self.document.is_some() && self.config.tts.enabled, egui::Button::new("Rebuild TTS analysis"))
                .clicked()
            {
                self.rebuild_tts_analysis();
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
        let path = new_config.preferred_config_path()?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }

        fs::write(&path, toml::to_string_pretty(&new_config)?)
            .with_context(|| format!("failed to write {}", path.display()))?;

        self.config = new_config.clone();
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
            self.current_page = self.search_results[index].page_index;
        }
        self.status_message = Some(format!("Found {} result(s)", self.search_results.len()));
    }

    fn activate_search_result(&mut self, index: usize, ctx: &egui::Context) {
        if let Some(result) = self.search_results.get(index) {
            self.active_search_result = Some(index);
            let focus_rect = result.rects.first().copied();
            self.navigate_to_page(result.page_index, focus_rect, ctx);
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
        self.paint_text_region_highlights(ui, page_index, &response, &view.image);
        self.paint_search_highlights(ui, page_index, &response, &view.image);

        if response.clicked() {
            self.current_page = page_index;
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
                        self.paint_text_region_highlights(ui, page_index, &response, &view.image);
                        self.paint_search_highlights(ui, page_index, &response, &view.image);

                        if response.clicked() {
                            self.current_page = page_index;
                            self.persist_session();
                        }

                        if response.hovered() && ctx.input(|input| input.raw_scroll_delta.y != 0.0)
                        {
                            self.current_page = page_index;
                        }
                    });
                self.single_scroll_offset = output.state.offset;
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

        if let Some(document_path) = session.last_document {
            if document_path.exists() {
                self.open_pdf(ctx, document_path);
                self.current_page = session.last_page;
                self.render_current_page(ctx);
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

        egui::TopBottomPanel::bottom("instrumentation_panel")
            .resizable(true)
            .default_height(self.config.ui.bottom_panel_height)
            .show(ctx, |ui| self.bottom_panel(ctx, ui));

        egui::CentralPanel::default().show(ctx, |ui| self.central_panel(ui));
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
