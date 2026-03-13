# PDF TTS Integration Roadmap

This roadmap is PDF-specific and is intentionally stricter than the current Lantern Leaf implementation. Lantern Leaf is useful as a reference for architecture slices such as `tts_text` ownership, prefetch, viewport budgeting, and overlay testing, but it is not treated as a correctness proof. In particular, its current PDF TTS and sync behavior is unstable, so this roadmap assumes that PDF text geometry is probabilistic and must be quality-classified before it is trusted.

Checked items reflect capabilities already present in this repository that reduce integration risk. Unchecked items are still to be designed or implemented.

## Product Contract

- [ ] Deliver PDF TTS with a stable play/pause experience in the native `eframe` / `egui` reader.
- [ ] Keep PDF viewing responsive while audio playback, pre-generation, and sync work are active.
- [ ] Make canonical playback ownership come from normalized PDF-derived text rather than from viewport state.
- [ ] Highlight the currently spoken sentence at the correct geometric PDF position when confidence is high enough.
- [ ] Degrade honestly when geometry is weak instead of faking sentence-level precision.
- [ ] Keep search, resume, bookmarks, and TTS cursor semantics anchored to the same canonical text plan.

## Existing Foundation In Pdfizer

- [x] Native PDF rendering with `eframe` / `egui` and PDFium
- [x] Continuous multi-page view
- [x] Zoom and page navigation
- [x] Search across extracted PDF text
- [x] Selectable text layer from PDF text segments
- [x] Highlight overlays on top of rendered PDF pages
- [x] Cached rendering and tiled rendering for large pages
- [x] Session persistence
- [x] TOML configuration and runtime overrides
- [x] Extensive `tracing` instrumentation and file-backed logs

## Non-Negotiable Design Rules

- [ ] Treat PDF TTS as a dual-pipeline problem: canonical text/audio ownership plus viewport-local visual sync.
- [ ] Never let the rendered viewport or page-local DOM state become the source of truth for playback position.
- [ ] Never present low-confidence sentence geometry as exact sync.
- [ ] Keep fallback decisions monotonic: sentence -> line -> block -> page -> no sync.
- [ ] Separate rendering-thread work from TTS preparation work.
- [ ] Keep every sync decision auditable with confidence, fallback reason, and source artifact identifiers.

## Phase 1: Canonical Text Ownership

- [ ] Define a canonical `tts_text` artifact for PDFs.
- [ ] Define a canonical `sentence_plan` artifact built only from normalized `tts_text`.
- [ ] Preserve page, block, line, and token provenance when deriving `tts_text`.
- [ ] Store stable sentence ids so playback, search, resume, and bookmarks all target the same units.
- [ ] Ensure zoom, page changes, and view-mode changes do not alter sentence ids.
- [ ] Add explicit tracing proving each playback step originated from canonical `tts_text`.

## Phase 2: PDF Type Classification

- [ ] Classify each opened PDF into runtime modes before enabling fine-grained sync.
- [ ] Support `high_text_trust` for clean embedded text and strong geometry.
- [ ] Support `mixed_text_trust` for usable text with imperfect reading order or geometry.
- [ ] Support `ocr_required` for scanned or image-first PDFs.
- [ ] Support `render_only_no_sync` when text or geometry cannot be trusted.
- [ ] Persist classification results and confidence summaries in cache.
- [ ] Gate TTS and highlight behavior based on classification rather than optimistic assumptions.

## Phase 3: Text Extraction And Normalization

- [ ] Build a dedicated PDF-to-`tts_text` pipeline separate from the current UI text-selection helpers.
- [ ] Normalize ligatures, soft hyphens, repeated headers/footers, duplicate glyph streams, and whitespace noise.
- [ ] Preserve paragraph boundaries when they are recoverable.
- [ ] Handle multi-column extraction explicitly instead of assuming PDF stream order is readable.
- [ ] Detect and suppress hidden or duplicated OCR text layers.
- [ ] Add regression handling for tables, footnotes, captions, sidenotes, and rotated text.
- [ ] Emit diagnostics for every normalization edit class.

## Phase 4: Sentence Segmentation And Audio Units

- [ ] Split `tts_text` into stable sentences.
- [ ] Support language-aware sentence segmentation rules in config.
- [ ] Keep sentence ids stable across reopen and cache reuse.
- [ ] Define sentence-to-page provenance ranges even before geometry mapping is complete.
- [ ] Support fallback to paragraph or block playback units when sentence segmentation is weak.
- [ ] Add tests for abbreviations, citations, tables, and line-wrap edge cases.

## Phase 5: Geometry Artifact And Sync Mapping

- [ ] Define a persistent sync artifact: `sentence_id -> page_idx + rects[] + confidence + fallback_reason`.
- [ ] Support one sentence mapping to multiple disjoint rectangles.
- [ ] Support one sentence spanning multiple lines or blocks.
- [ ] Keep mapping deterministic even when extraction required cleanup.
- [ ] Score matches using text similarity, reading-order continuity, local geometry compactness, and page continuity.
- [ ] Reject visually implausible matches even if text similarity looks high.
- [ ] Persist token lineage so bad highlights can be debugged later.
- [ ] Add a nearest-safe fallback when exact sentence geometry is unavailable.

## Phase 6: OCR Strategy For PDFs

- [ ] Decide whether OCR is optional, deferred, or first-class in this project.
- [ ] Define an OCR output contract with page, block, line, token, bounding box, and confidence fields.
- [ ] Support OCR-derived `tts_text` for scanned PDFs only when confidence passes a minimum threshold.
- [ ] Keep OCR text confidence distinct from embedded-text confidence.
- [ ] Support OCR geometry classes such as `ocr_high_trust`, `ocr_mixed_trust`, and `ocr_text_only`.
- [ ] Refuse sentence-precise overlay sync for OCR outputs that only justify block-level mapping.
- [ ] Persist OCR artifacts separately from embedded-text artifacts.

## Phase 7: TTS Engine Abstraction

- [ ] Introduce a Rust-side `TtsEngine` abstraction decoupled from UI and PDF rendering code.
- [ ] Support play, pause, resume, stop, seek-to-sentence, next-sentence, and previous-sentence operations.
- [ ] Support pluggable backends so engine choice does not leak into the reader domain model.
- [ ] Define output artifact policy for generated audio clips, durations, and cache keys.
- [ ] Define voice, rate, volume, and sentence pause configuration in TOML.
- [ ] Add tracing spans for engine startup, synthesis, playback state transitions, and errors.

## Phase 8: Ahead-Of-Time Audio Generation

- [ ] Generate sentence audio ahead of current playback position.
- [ ] Use a bounded prefetch window configurable by sentence count and audio duration budget.
- [ ] Keep pre-generation cancellable when the user seeks, changes document, or changes voice/rate.
- [ ] Persist generated audio clips in a cache keyed by source fingerprint, sentence id, voice, and synthesis settings.
- [ ] Reuse generated clips across pause/resume and reopen when cache entries are still valid.
- [ ] Avoid blocking UI rendering on synthesis completion.
- [ ] Add cache invalidation rules for changed normalization, changed sentence plan, and changed TTS settings.

## Phase 9: Threading And Scheduling

- [ ] Run synthesis preparation off the UI thread.
- [ ] Run audio playback control off the UI thread.
- [ ] Introduce explicit cancellation tokens for document close, seek, jump, and engine reconfiguration.
- [ ] Keep PDF rendering, text extraction, sync mapping, and audio preparation in separate work lanes.
- [ ] Bound concurrency so prefetch does not starve page rendering or search responsiveness.
- [ ] Add a scheduler policy that prioritizes current-page rendering over future audio generation when resources are tight.
- [ ] Log queue depths, task latency, cancellation outcomes, and starvation signals.

## Phase 10: Playback UI And Controls

- [ ] Add a dedicated player bar suited to the current `egui` layout rather than copying the Lantern Leaf widget literally.
- [ ] Support play/pause.
- [ ] Support previous/next sentence.
- [ ] Support stop.
- [ ] Surface current sentence index, page, and sync confidence tier.
- [ ] Surface degraded-mode messaging when sentence-accurate sync is unavailable.
- [ ] Keep control state updates independent from expensive page rerenders.
- [ ] Support keyboard shortcuts for playback and sentence navigation.

## Phase 11: Sentence Highlight Overlay

- [ ] Render the active spoken sentence as a PDF overlay on top of the page image.
- [ ] Support multi-rect sentence highlights.
- [ ] Support line-level fallback highlight.
- [ ] Support block-level fallback highlight.
- [ ] Support page-active fallback when geometry is too weak for local highlights.
- [ ] Keep highlight alignment stable through zoom, scroll, DPI changes, and rerender cycles.
- [ ] Remove stale highlights immediately on seek, page jump, zoom change, or cache invalidation.
- [ ] Allow highlight styling to be configured separately from search highlights.

## Phase 12: Scroll Following And Viewport Stability

- [ ] Auto-scroll only when playback moves outside a stable visible region.
- [ ] Keep playback from fighting manual user scroll unless follow mode is enabled.
- [ ] Support a pinned follow mode centered on the active sentence region.
- [ ] Keep continuous view smooth while playback advances across pages.
- [ ] Preload nearby page text layers and highlight artifacts around the active playback region.
- [ ] Avoid full-document relayout or full-cache invalidation on each sentence advance.
- [ ] Emit tracing for scroll trigger reason, old viewport, new viewport, and skipped auto-scroll decisions.

## Phase 13: Rendering Performance Protection

- [ ] Keep canvas rendering and text-layer/highlight updates incremental.
- [ ] Restrict text-layer work to visible and near-visible pages.
- [ ] Cache geometry artifacts separately from page bitmaps.
- [ ] Avoid rebuilding sentence overlays for the whole document on each playback tick.
- [ ] Add explicit budgets for visible-page canvases, text layers, and sync overlays.
- [ ] Add a playback-performance profile that measures render latency while audio is active.
- [ ] Define acceptable budgets for sentence-advance-to-highlight latency and scroll jitter.

## Phase 14: Search, Resume, And Navigation Semantics

- [ ] Make TTS sentence ids, search hits, and resume positions interoperable.
- [ ] Allow search result activation to update the TTS cursor.
- [ ] Allow TTS cursor changes to update the visible PDF page and highlight region.
- [ ] Persist resume state as canonical sentence id plus best-known PDF location metadata.
- [ ] Support reverse navigation from PDF interaction to nearest sentence id.
- [ ] Keep reopen deterministic after cache reuse or rebuild.

## Phase 15: Config And Runtime Controls

- [ ] Add a dedicated `[tts]` config section with engine, voice, rate, volume, prefetch, and cache settings.
- [ ] Add feature gates for experimental PDF sync modes.
- [ ] Add explicit thresholds for geometry confidence and fallback transitions.
- [ ] Add separate config for OCR behavior and quality thresholds if OCR is enabled.
- [ ] Expose runtime toggles for follow mode, highlight mode, and degraded-mode verbosity.

## Phase 16: Observability And Diagnostics

- [ ] Trace classification decisions for every PDF opened for TTS use.
- [ ] Trace normalization edits and mapping confidence summaries.
- [ ] Trace synthesis queue fill level, clip cache hit rates, and playback underruns.
- [ ] Trace highlight target resolution and fallback transitions.
- [ ] Add a developer diagnostics panel for active sentence id, page, rect count, confidence, and fallback reason.
- [ ] Add exportable debug snapshots that capture sentence plan, geometry matches, and visible highlight state.
- [ ] Add failure counters for wrong-page rejects, distant-geometry rejects, and unmappable sentences.

## Phase 17: Test Fixtures And Regression Coverage

- [ ] Build a PDF fixture corpus with at least these classes:
- [ ] clean selectable-text books
- [ ] academic two-column papers
- [ ] PDFs with repeated headers and footers
- [ ] PDFs with footnotes and captions
- [ ] table-heavy PDFs
- [ ] rotated pages and rotated text
- [ ] scanned image PDFs
- [ ] mixed OCR plus embedded-text PDFs
- [ ] corrupted or duplicate text-layer PDFs
- [ ] Add unit tests for normalization and sentence planning.
- [ ] Add unit tests for sentence-to-geometry mapping and confidence scoring.
- [ ] Add integration tests for playback stepping, seek, pause/resume, and reopen.
- [ ] Add regression tests for highlight alignment during zoom and continuous scrolling.
- [ ] Add performance tests that exercise playback during active scrolling and zoom.

## Phase 18: Manual QA

- [ ] Create a manual checklist for PDF TTS playback, sync, and degraded modes.
- [ ] Verify sentence-following highlight accuracy on high-trust PDFs.
- [ ] Verify honest degradation on mixed-trust and OCR PDFs.
- [ ] Verify playback remains responsive during continuous scrolling and zoom changes.
- [ ] Verify seek, search, page jump, and reopen semantics.
- [ ] Capture logs and screenshots for all low-confidence sync decisions during QA.

## Milestone Order

- [ ] Milestone 1: canonical `tts_text`, sentence plan, and playback state machine
- [ ] Milestone 2: threaded TTS engine with ahead-of-time clip generation and cache
- [ ] Milestone 3: PDF classification, normalization, and geometry artifact
- [ ] Milestone 4: sentence highlight overlays with honest fallback behavior
- [ ] Milestone 5: scroll-follow and performance budgeting in continuous view
- [ ] Milestone 6: OCR and degraded-mode policy
- [ ] Milestone 7: fixture corpus, regression suite, and diagnostics hardening

## Acceptance Criteria

- [ ] Play/pause works reliably without stalling rendering.
- [ ] Ahead-of-time generation keeps playback fed without blocking UI work.
- [ ] Sentence stepping and seek are deterministic across reopen and cache reuse.
- [ ] The active sentence highlight lands on the correct PDF geometry for high-trust PDFs.
- [ ] Mixed-trust and OCR PDFs degrade to line, block, page, or no-sync modes without false precision.
- [ ] Continuous PDF viewing remains visually responsive while TTS is active.
- [ ] Logs and diagnostics are sufficient to explain every sync and fallback decision.
