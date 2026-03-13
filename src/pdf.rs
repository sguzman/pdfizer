use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow};
use eframe::egui::{self, ColorImage, TextureHandle, Vec2};
use pdfium_render::prelude::*;
use tracing::{debug, info, instrument};

use crate::config::{AppConfig, library_path_from_config_or_env};

pub struct PdfRuntime {
    pdfium: &'static Pdfium,
}

impl PdfRuntime {
    pub fn new(config: &AppConfig) -> Result<Self> {
        let bindings = if let Some(path) = library_path_from_config_or_env(&config.pdfium) {
            info!(library = %path.display(), "binding Pdfium to configured library");
            Pdfium::bind_to_library(Pdfium::pdfium_platform_library_name_at_path(&path))
                .context("failed to bind Pdfium using configured library path")?
        } else {
            info!("binding Pdfium to system library");
            Pdfium::bind_to_system_library()
                .context("failed to bind Pdfium to a system library; set PDFIUM_DYNAMIC_LIB_PATH or pdfium.library_path")?
        };

        Ok(Self {
            pdfium: Box::leak(Box::new(Pdfium::new(bindings))),
        })
    }

    pub fn open_document(&self, path: impl AsRef<Path>) -> Result<PdfDocument<'static>> {
        let path = path.as_ref().to_path_buf();
        let document = self
            .pdfium
            .load_pdf_from_file(&path, None)
            .with_context(|| format!("failed to open PDF at {}", path.display()))?;
        let page_count = document.pages().len() as usize;

        let metadata = PdfMetadata {
            path,
            page_count,
            title: metadata_value(&document, PdfDocumentMetadataTagType::Title),
            author: metadata_value(&document, PdfDocumentMetadataTagType::Author),
            subject: metadata_value(&document, PdfDocumentMetadataTagType::Subject),
            keywords: metadata_value(&document, PdfDocumentMetadataTagType::Keywords),
            version: Some(format!("{:?}", document.version())),
        };

        Ok(PdfDocument { document, metadata })
    }
}

pub struct PdfDocument<'a> {
    document: pdfium_render::prelude::PdfDocument<'a>,
    pub metadata: PdfMetadata,
}

impl PdfDocument<'_> {
    #[instrument(skip(self, ctx, texture))]
    pub fn render_page(
        &self,
        ctx: &egui::Context,
        texture: Option<&mut TextureHandle>,
        page_index: usize,
        zoom: f32,
        texture_options: egui::TextureOptions,
    ) -> Result<RenderResult> {
        let started = Instant::now();
        let page = self
            .document
            .pages()
            .get(page_index as u16)
            .with_context(|| format!("page index {page_index} is out of bounds"))?;

        let target_width = ((page.width().value * zoom).max(1.0).round() as i32).max(1);
        let render = page.render_with_config(
            &PdfRenderConfig::new()
                .set_target_width(target_width)
                .render_form_data(true),
        );
        let bitmap = render
            .map_err(|err| anyhow!("Pdfium render failed: {err}"))?
            .as_image();

        let size = [bitmap.width() as usize, bitmap.height() as usize];
        let rgba = bitmap.into_rgba8();
        let image = ColorImage::from_rgba_unmultiplied(size, rgba.as_raw());

        let texture = match texture {
            Some(existing) => {
                existing.set(image, texture_options);
                existing.clone()
            }
            None => ctx.load_texture(format!("pdf-page-{page_index}"), image, texture_options),
        };

        let dimensions = Vec2::new(size[0] as f32, size[1] as f32);
        let elapsed = started.elapsed();
        debug!(
            page_index,
            zoom,
            width = dimensions.x,
            height = dimensions.y,
            elapsed_ms = elapsed.as_secs_f64() * 1000.0,
            "rendered PDF page"
        );

        Ok(RenderResult {
            texture,
            dimensions,
            elapsed,
        })
    }
}

fn metadata_value(
    document: &pdfium_render::prelude::PdfDocument<'_>,
    tag: PdfDocumentMetadataTagType,
) -> Option<String> {
    document
        .metadata()
        .get(tag)
        .map(|entry| entry.value().to_owned())
}

#[derive(Debug, Clone)]
pub struct PdfMetadata {
    pub path: PathBuf,
    pub page_count: usize,
    pub title: Option<String>,
    pub author: Option<String>,
    pub subject: Option<String>,
    pub keywords: Option<String>,
    pub version: Option<String>,
}

#[derive(Clone)]
pub struct RenderResult {
    pub texture: TextureHandle,
    pub dimensions: Vec2,
    pub elapsed: Duration,
}
