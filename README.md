# Pdfizer

`Pdfizer` is a native Rust PDF exploration app for studying PDF rendering behavior with `eframe` / `egui` and PDFium.

The current build is no longer just a minimal viewer. It includes thumbnail navigation, cached page renders, large-page tiled rendering, comparison presets, a pixel inspector, benchmark export, persisted session state, layered TOML config, and file-backed tracing.

It also now includes the first four PDF TTS tranches plus completed Phase 1 and Phase 2 roadmap work: canonical PDF text analysis with page/block/line/token provenance, stable sentence ids, persisted trust classification summaries, a tone-preview playback backend, ahead-of-time clip preparation, cached sentence-to-PDF sync targets, confidence-scored overlays, OCR/degraded-mode runtime policy, viewport-local TTS budgeting, and tranche-level diagnostics. OCR-backed text extraction and stronger alignment still remain future work.

## Stack

- Rust 2024
- `eframe` / `egui`
- `pdfium-render`
- `tracing`, `tracing-subscriber`, `tracing-appender`
- `config`, `serde`, TOML

## Features

- Open local PDFs with a native file picker
- Render pages with PDFium into `egui`
- Navigate with buttons, thumbnails, arrow keys, and page keys
- Zoom interactively and cache render results across zoom levels
- Use tiled rendering for very large pages
- Compare two render presets side by side
- Inspect hovered pixels and select/copy text directly from the PDF
- Show document metadata, render timing, and aggregate render statistics
- Persist session state and save edited config back to disk
- Export benchmark snapshots as CSV
- Write logs to stdout and a file sink
- Build and inspect canonical PDF TTS analysis artifacts in the background when a document opens
- Persist canonical TTS artifacts with sentence provenance and classification summaries in a stable sidecar
- Exercise a tone-preview TTS runtime with play/pause, sentence stepping, direct sentence seek, follow mode, and cached prepared clips
- Highlight the active spoken sentence on the PDF with exact, fuzzy, block, or page-level fallback behavior
- Apply OCR/degraded-mode policy so scan-first PDFs degrade honestly instead of pretending sentence-accurate sync exists
- Track TTS prep, sync, activation latency, and cache-hit diagnostics while keeping runtime state bounded around the viewport
- Keep TTS sentence ids stable across reopen, search activation, zoom, and view-mode changes

## Pdfium Runtime

This app binds PDFium dynamically at runtime. Provide a shared library either with:

- `PDFIUM_DYNAMIC_LIB_PATH`
- `pdfium.library_path` in [config/pdfizer.toml](/win/linux/Code/rust/pdfizer/config/pdfizer.toml)

Examples:

```bash
PDFIUM_DYNAMIC_LIB_PATH=/absolute/path/to/libpdfium.so cargo run
```

```powershell
$env:PDFIUM_DYNAMIC_LIB_PATH="C:\path\to\pdfium.dll"
cargo run
```

If Pdfium is missing, the app still starts and shows runtime guidance in the UI.

## Configuration

Configuration is layered in this order:

1. built-in defaults
2. [config/default.toml](/win/linux/Code/rust/pdfizer/config/default.toml)
3. [config/pdfizer.toml](/win/linux/Code/rust/pdfizer/config/pdfizer.toml)
4. user config directory `pdfizer.toml`
5. environment variables with the prefix `PDFIZER__`

Example override:

```bash
PDFIZER__RENDERING__INITIAL_ZOOM=1.75 cargo run
```

The in-app config editor saves to `startup.preferred_config_name`.

## Modes

The top-level config field `mode` supports:

- `prod`
- `dev`

When `mode = "dev"`, the app always writes timestamped logs to `./logs/` in the current working directory, for example `logs/pdfizer-<timestamp>.log`.

## Logging And Artifacts

- Render/session/benchmark artifacts are written under the app data directory by default.
- Benchmark snapshots export as CSV.
- In `prod`, logs are written to stdout and the configured log sink when `logging.write_to_file = true`.
- In `dev`, logs are always written to stdout plus timestamped files in `./logs/`.

## Packaging

Packaging and Pdfium distribution notes are in [docs/packaging.md](/win/linux/Code/rust/pdfizer/docs/packaging.md).

## Development

Run the checks:

```bash
cargo fmt --check
cargo test
cargo build
```

Useful tracing:

```bash
RUST_LOG=pdfizer=trace cargo run
```

The active implementation checklist lives in [docs/roadmap.md](/win/linux/Code/rust/pdfizer/docs/roadmap.md).
The PDF-specific TTS integration plan lives in [docs/roadmaps/pdf-tts-integration-roadmap.md](/win/linux/Code/rust/pdfizer/docs/roadmaps/pdf-tts-integration-roadmap.md).
Manual QA gates for PDF TTS live in [docs/qa/pdf-tts-manual-checklist.md](/win/linux/Code/rust/pdfizer/docs/qa/pdf-tts-manual-checklist.md) and [docs/qa/pdf-tts-acceptance-gates.md](/win/linux/Code/rust/pdfizer/docs/qa/pdf-tts-acceptance-gates.md).
