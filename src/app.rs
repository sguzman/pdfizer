use std::path::PathBuf;

use eframe::egui::{self, RichText, Sense, Slider, TextureHandle, Ui};
use rfd::FileDialog;
use tracing::{debug, error, info, instrument, warn};

use crate::{
    config::AppConfig,
    pdf::{PdfDocument, PdfMetadata, PdfRuntime, RenderResult},
};

pub struct PdfizerApp {
    config: AppConfig,
    runtime: Option<PdfRuntime>,
    runtime_error: Option<String>,
    document: Option<PdfDocument<'static>>,
    last_error: Option<String>,
    current_page: usize,
    zoom: f32,
    texture: Option<TextureHandle>,
    last_render_ms: Option<f64>,
    last_render_size: Option<(usize, usize)>,
    config_preview: String,
}

impl PdfizerApp {
    pub fn new(_cc: &eframe::CreationContext<'_>, config: AppConfig) -> Self {
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

        Self {
            zoom: config.rendering.initial_zoom,
            config_preview: config.config_preview(),
            config,
            runtime,
            runtime_error,
            document: None,
            last_error: None,
            current_page: 0,
            texture: None,
            last_render_ms: None,
            last_render_size: None,
        }
    }

    #[instrument(skip(self, ctx))]
    fn open_pdf(&mut self, ctx: &egui::Context, path: PathBuf) {
        let Some(runtime) = &self.runtime else {
            self.last_error = Some("Pdfium runtime is unavailable".into());
            return;
        };

        match runtime.open_document(&path) {
            Ok(document) => {
                info!(path = %path.display(), pages = document.metadata.page_count, "opened PDF");
                self.document = Some(document);
                self.current_page = 0;
                self.last_error = None;
                self.texture = None;
                self.render_current_page(ctx);
            }
            Err(err) => {
                error!(path = %path.display(), error = %err, "failed to open PDF");
                self.last_error = Some(err.to_string());
            }
        }
    }

    fn render_current_page(&mut self, ctx: &egui::Context) {
        let Some(document) = &self.document else {
            return;
        };

        match document.render_page(
            ctx,
            self.texture.as_mut(),
            self.current_page,
            self.zoom,
            self.config.rendering.texture_filter.to_texture_options(),
        ) {
            Ok(RenderResult {
                texture,
                dimensions,
                elapsed,
            }) => {
                self.texture = Some(texture);
                self.last_render_ms = Some(elapsed.as_secs_f64() * 1000.0);
                self.last_render_size = Some((dimensions.x as usize, dimensions.y as usize));
            }
            Err(err) => {
                error!(page = self.current_page, error = %err, "render failed");
                self.last_error = Some(err.to_string());
            }
        }
    }

    fn next_page(&mut self, ctx: &egui::Context) {
        if let Some(document) = &self.document {
            if self.current_page + 1 < document.metadata.page_count {
                self.current_page += 1;
                self.render_current_page(ctx);
            }
        }
    }

    fn previous_page(&mut self, ctx: &egui::Context) {
        if self.current_page > 0 {
            self.current_page -= 1;
            self.render_current_page(ctx);
        }
    }

    fn top_bar(&mut self, ctx: &egui::Context, ui: &mut Ui) {
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
            self.render_current_page(ctx);
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

    fn side_panel(&mut self, ui: &mut Ui) {
        ui.heading("Study Controls");
        ui.label("Use this shell to inspect PDFium behavior in a native Rust rendering loop.");
        ui.separator();

        if let Some(runtime_error) = &self.runtime_error {
            ui.colored_label(egui::Color32::LIGHT_RED, runtime_error);
            ui.separator();
        }

        if let Some(document) = &self.document {
            render_metadata(ui, &document.metadata);
        } else {
            ui.label("No document loaded.");
        }
    }

    fn bottom_panel(&mut self, ui: &mut Ui) {
        ui.heading("Instrumentation");
        if let Some(error) = &self.last_error {
            ui.colored_label(egui::Color32::LIGHT_RED, error);
        }

        if self.config.ui.show_metrics {
            if let Some(ms) = self.last_render_ms {
                ui.label(format!("Last render: {ms:.2} ms"));
            } else {
                ui.label("Last render: n/a");
            }

            if let Some((w, h)) = self.last_render_size {
                ui.label(format!("Bitmap: {w} x {h}px"));
            } else {
                ui.label("Bitmap: n/a");
            }
        }

        if self.config.ui.show_logs_hint {
            ui.separator();
            ui.monospace("Tracing: RUST_LOG=pdfizer=trace cargo run");
        }

        ui.separator();
        ui.collapsing("Resolved config", |ui| {
            let editor = egui::TextEdit::multiline(&mut self.config_preview)
                .font(egui::TextStyle::Monospace)
                .desired_rows(16)
                .interactive(false);
            ui.add(editor);
        });
    }

    fn central_panel(&mut self, ui: &mut Ui) {
        let Some(texture) = &self.texture else {
            ui.centered_and_justified(|ui| {
                ui.label("Open a PDF to render it.");
            });
            return;
        };

        let image_size = texture.size_vec2();

        egui::Frame::default()
            .fill(self.config.background_color())
            .show(ui, |ui| {
                egui::ScrollArea::both()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        let image = egui::Image::new(texture)
                            .fit_to_exact_size(image_size)
                            .sense(Sense::hover());
                        ui.add(image);
                    });
            });
    }
}

impl eframe::App for PdfizerApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        ctx.set_pixels_per_point(1.0);

        egui::TopBottomPanel::top("toolbar").show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| self.top_bar(ctx, ui));
        });

        egui::SidePanel::left("study_panel")
            .resizable(true)
            .default_width(self.config.ui.left_panel_width)
            .show(ctx, |ui| self.side_panel(ui));

        egui::TopBottomPanel::bottom("instrumentation_panel")
            .resizable(true)
            .default_height(self.config.ui.bottom_panel_height)
            .show(ctx, |ui| self.bottom_panel(ui));

        egui::CentralPanel::default().show(ctx, |ui| self.central_panel(ui));
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

fn pdfium_help_text(config: &AppConfig) -> String {
    let env_var = &config.pdfium.library_env_var;
    warn!("Pdfium is not available yet; the UI will remain in help mode");
    format!(
        "Pdfium failed to bind. Set {env_var} or pdfium.library_path in config/pdfizer.toml to your Pdfium shared library."
    )
}
