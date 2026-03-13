# PDF TTS Acceptance Gates

## Reader Responsiveness

- PDF rendering, scrolling, zooming, and page navigation must remain visually responsive while TTS is active.
- If the runtime is resource-constrained, TTS prefetch and sync work must degrade before reader interaction does.
- Activation latency should stay within `tts.active_latency_budget_ms` for ordinary sentence stepping on warmed cache.

## Sync Honesty

- `high_text_trust` and clean `mixed_text_trust` PDFs may use sentence rectangles when confidence supports them.
- `ocr_required` with `ocr_policy = "deferred"` may play audio, but must degrade to page follow without rectangle highlights.
- `ocr_required` with `ocr_policy = "disabled"` or `ocr_policy = "require_artifacts"` must block playback until the policy changes or OCR artifacts exist.
- `render_only_no_sync` PDFs must never present sentence-precise geometry.

## Diagnostics

- Diagnostics must expose PDF mode, runtime policy reason, active sync confidence, fallback reason, and timing metrics.
- Low-confidence or blocked decisions must be traceable from logs without reconstructing hidden state.
- Sync counters should distinguish exact, fuzzy, block, page, and missing outcomes.

## Corpus Expectations

- The fixture corpus manifest in [tests/fixtures/pdf_tts_fixture_corpus.toml](/win/linux/Code/rust/pdfizer/tests/fixtures/pdf_tts_fixture_corpus.toml) must keep explicit slots for clean text, repeated boilerplate, scans, mixed OCR, rotated content, tables, and duplicate text layers.
- Any fixture added for a new PDF failure mode must include a short note describing the expected TTS trust class and allowed fallback floor.
