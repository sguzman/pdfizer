# Pdfizer

`Pdfizer` is a native Rust PDF exploration app for studying PDF rendering behavior with `eframe` / `egui` and PDFium.

The current build is no longer just a minimal viewer. It includes thumbnail navigation, cached page renders, large-page tiled rendering, comparison presets, a pixel inspector, benchmark export, persisted session state, layered TOML config, and file-backed tracing.

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
- Inspect hovered pixels and drag out a selection rectangle
- Show document metadata, render timing, and aggregate render statistics
- Persist session state and save edited config back to disk
- Export benchmark snapshots as CSV
- Write logs to stdout and a file sink

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

## Logging And Artifacts

- Render/session/benchmark artifacts are written under the app data directory by default.
- Benchmark snapshots export as CSV.
- Logs are written to both stdout and `logs/pdfizer.log` unless disabled in config.

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
