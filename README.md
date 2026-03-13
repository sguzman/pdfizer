# Pdfizer

`Pdfizer` is a native Rust PDF exploration app focused on studying fast PDF rendering behavior with `eframe`/`egui` and PDFium.

The current build is intentionally narrow: it opens local PDFs, renders pages through PDFium, exposes render timing and bitmap dimensions, surfaces document metadata, and keeps runtime behavior configurable with layered TOML plus environment overrides. The reference note in `tmp/project.md` is treated as direction, not as a fixed spec.

## Stack

- Rust 2024
- `eframe` / `egui` for the desktop shell
- `pdfium-render` for PDFium bindings
- `tracing` + `tracing-subscriber` for instrumentation
- `config` + `serde` + TOML for runtime configuration

## What It Does

- Opens local PDF files with a native file picker
- Renders pages with PDFium into the `egui` viewport
- Supports previous/next navigation and zoom control
- Shows document metadata when the PDF provides it
- Displays render timing and raster dimensions for quick inspection
- Emits tracing around runtime init, document load, and page rendering

## Pdfium Runtime Requirement

This app binds PDFium dynamically at runtime. You need a Pdfium shared library available either:

- via `PDFIUM_DYNAMIC_LIB_PATH`
- or via `pdfium.library_path` in `config/pdfizer.toml`

Examples:

```bash
PDFIUM_DYNAMIC_LIB_PATH=/absolute/path/to/libpdfium.so cargo run
```

```powershell
$env:PDFIUM_DYNAMIC_LIB_PATH="C:\path\to\pdfium.dll"
cargo run
```

If PDFium is not available, the app still starts and shows a runtime help message instead of crashing immediately.

## Configuration

The app reads configuration in this order:

1. built-in defaults
2. `config/pdfizer.toml`
3. `config/default.toml`
4. user config directory `pdfizer.toml`
5. environment variables with the prefix `PDFIZER__`

The included sample config lives at [config/default.toml](/win/linux/Code/rust/pdfizer/config/default.toml).

Example override:

```bash
PDFIZER__RENDERING__INITIAL_ZOOM=1.75 cargo run
```

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

## Next Work

The active implementation roadmap is tracked in [docs/roadmap.md](/win/linux/Code/rust/pdfizer/docs/roadmap.md). That file is meant to be updated as feature dimensions evolve.
