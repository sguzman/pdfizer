# Pdfizer Roadmap

This roadmap is organized by feature dimension rather than by milestone. Checked items reflect what is implemented in the current tree.

## Core Rendering

- [x] Native desktop shell using `eframe` / `egui`
- [x] PDFium-backed runtime binding with configurable shared-library path
- [x] Open local PDFs from a file picker
- [x] Render a selected page into an `egui` texture
- [x] Navigate pages with previous/next controls
- [x] Adjust render zoom interactively
- [ ] Multi-page thumbnail strip
- [ ] Incremental tile rendering for very large pages
- [ ] Render cache across multiple zoom levels

## Inspection And Study UX

- [x] Surface basic document metadata
- [x] Show render timing and bitmap size
- [x] Show resolved runtime config inside the app
- [x] Keep the app usable even when Pdfium fails to bind
- [ ] Keyboard shortcuts for paging and zoom
- [ ] Selection rectangle / pixel inspector
- [ ] Side-by-side render comparison presets

## Text, Reading Order, And TTS Prep

- [ ] Extract text runs and positional boxes from PDFium/PDF layer data
- [ ] Normalize page text into line and block groupings
- [ ] Build a reading-order model suitable for speech playback
- [ ] Add overlay highlighting tied to normalized text segments
- [ ] Integrate TTS playback controls
- [ ] Track timing alignment quality for spoken highlights

## Configuration And Runtime Controls

- [x] Layered TOML configuration
- [x] Environment variable overrides
- [x] Comprehensive sample config checked into the repo
- [ ] Persist last-opened document and page position
- [ ] In-app config editing and save-back
- [ ] Preset profiles for render experiments

## Observability And Diagnostics

- [x] `tracing` instrumentation on startup, document open, and render paths
- [x] Runtime guidance for missing Pdfium dependencies
- [ ] File-based log sink
- [ ] Span timing aggregation over multiple renders
- [ ] Exportable benchmark snapshots

## Quality And Delivery

- [x] README with setup and runtime notes
- [x] Unit tests for config loading and defaults
- [x] Build verification via `cargo test` and `cargo build`
- [ ] Integration test fixture PDFs
- [ ] CI automation
- [ ] Packaged binaries with Pdfium distribution guidance
