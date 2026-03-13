# Pdfizer Roadmap

This roadmap is organized by feature dimension rather than by milestone. Checked items reflect what is implemented in the current tree.

## Core Rendering

- [x] Native desktop shell using `eframe` / `egui`
- [x] PDFium-backed runtime binding with configurable shared-library path
- [x] Open local PDFs from a file picker
- [x] Render a selected page into an `egui` texture
- [x] Navigate pages with previous/next controls
- [x] Adjust render zoom interactively
- [x] Multi-page thumbnail strip
- [x] Incremental tile rendering for very large pages
- [x] Render cache across multiple zoom levels

## Inspection And Study UX

- [x] Surface basic document metadata
- [x] Show render timing and bitmap size
- [x] Show resolved runtime config inside the app
- [x] Keep the app usable even when Pdfium fails to bind
- [x] Keyboard shortcuts for paging and zoom
- [x] Selection rectangle / pixel inspector
- [x] Side-by-side render comparison presets

## Explicit Non-Features For Now

- [ ] TTS playback and speech engine integration
- [ ] Spoken-text highlighting and timing alignment
- [ ] Reading-order modeling for narration
- [ ] Text extraction work intended specifically for TTS pipelines
- [x] Separate TTS planning is tracked in [docs/roadmaps/pdf-tts-integration-roadmap.md](/win/linux/Code/rust/pdfizer/docs/roadmaps/pdf-tts-integration-roadmap.md)

## Configuration And Runtime Controls

- [x] Layered TOML configuration
- [x] Environment variable overrides
- [x] Comprehensive sample config checked into the repo
- [x] Persist last-opened document and page position
- [x] In-app config editing and save-back
- [x] Preset profiles for render experiments

## Observability And Diagnostics

- [x] `tracing` instrumentation on startup, document open, and render paths
- [x] Runtime guidance for missing Pdfium dependencies
- [x] File-based log sink
- [x] Span timing aggregation over multiple renders
- [x] Exportable benchmark snapshots

## Quality And Delivery

- [x] README with setup and runtime notes
- [x] Unit tests for config loading and defaults
- [x] Build verification via `cargo test` and `cargo build`
- [x] Integration test fixture PDFs
- [x] CI automation
- [x] Packaged binaries with Pdfium distribution guidance
