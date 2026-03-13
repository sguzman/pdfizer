use std::{
    env,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow};
use eframe::egui::ColorImage;
use pdfium_render::prelude::*;
use tracing::{debug, info, instrument};

use crate::config::{AppConfig, library_path_from_config_or_env};

pub struct PdfRuntime {
    pdfium: &'static Pdfium,
}

impl PdfRuntime {
    pub fn new(config: &AppConfig) -> Result<Self> {
        let bindings = bind_pdfium(config)?;

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
        let mut page_sizes = Vec::with_capacity(page_count);

        for index in 0..page_count {
            let page = document
                .pages()
                .get(index as u16)
                .with_context(|| format!("failed to access page {index}"))?;
            page_sizes.push(PageSizePoints {
                width: page.width().value,
                height: page.height().value,
            });
        }

        let metadata = PdfMetadata {
            path,
            page_count,
            title: metadata_value(&document, PdfDocumentMetadataTagType::Title),
            author: metadata_value(&document, PdfDocumentMetadataTagType::Author),
            subject: metadata_value(&document, PdfDocumentMetadataTagType::Subject),
            keywords: metadata_value(&document, PdfDocumentMetadataTagType::Keywords),
            version: Some(format!("{:?}", document.version())),
        };

        Ok(PdfDocument {
            document,
            metadata,
            page_sizes,
        })
    }
}

fn bind_pdfium(config: &AppConfig) -> Result<Box<dyn PdfiumLibraryBindings>> {
    if let Some(path) = library_path_from_config_or_env(&config.pdfium) {
        info!(library = %path.display(), "binding Pdfium to configured library");
        return bind_path(&path)
            .with_context(|| format!("failed to bind Pdfium using {}", path.display()));
    }

    for candidate in pdfium_auto_discovery_candidates()? {
        if candidate.exists() {
            info!(library = %candidate.display(), "binding Pdfium to discovered local library");
            if let Ok(bindings) = bind_path(&candidate) {
                return Ok(bindings);
            }
        }
    }

    info!("binding Pdfium to system library");
    Pdfium::bind_to_system_library().context(
        "failed to bind Pdfium from configured path, discovered local paths, or system library",
    )
}

fn bind_path(path: &Path) -> Result<Box<dyn PdfiumLibraryBindings>, PdfiumError> {
    let resolved = if path.is_dir() {
        Pdfium::pdfium_platform_library_name_at_path(path)
    } else {
        path.to_path_buf()
    };

    Pdfium::bind_to_library(resolved)
}

fn pdfium_auto_discovery_candidates() -> Result<Vec<PathBuf>> {
    let mut candidates = Vec::new();
    let lib_name = Pdfium::pdfium_platform_library_name();

    let cwd = env::current_dir().context("failed to get current directory for Pdfium search")?;
    candidates.push(cwd.join(&lib_name));
    candidates.push(cwd.join("lib").join(&lib_name));
    candidates.push(cwd.join("bin").join(&lib_name));

    let exe_dir = env::current_exe()
        .context("failed to get current executable for Pdfium search")?
        .parent()
        .map(Path::to_path_buf);

    if let Some(exe_dir) = exe_dir {
        candidates.push(exe_dir.join(&lib_name));
        candidates.push(exe_dir.join("lib").join(&lib_name));
        candidates.push(exe_dir.join("bin").join(&lib_name));
    }

    Ok(candidates)
}

pub struct PdfDocument<'a> {
    document: pdfium_render::prelude::PdfDocument<'a>,
    pub metadata: PdfMetadata,
    page_sizes: Vec<PageSizePoints>,
}

impl PdfDocument<'_> {
    pub fn page_size(&self, page_index: usize) -> Option<PageSizePoints> {
        self.page_sizes.get(page_index).copied()
    }

    pub fn extract_text_in_rect(&self, page_index: usize, rect: PdfRectData) -> Result<String> {
        let page = self
            .document
            .pages()
            .get(page_index as u16)
            .with_context(|| format!("page index {page_index} is out of bounds"))?;
        let text = page
            .text()
            .map_err(|err| anyhow!("failed to load page text: {err}"))?;

        Ok(text.inside_rect(rect.to_pdf_rect()))
    }

    pub fn search_page(
        &self,
        page_index: usize,
        query: &str,
        match_case: bool,
        match_whole_word: bool,
    ) -> Result<Vec<SearchHit>> {
        let page = self
            .document
            .pages()
            .get(page_index as u16)
            .with_context(|| format!("page index {page_index} is out of bounds"))?;
        let text = page
            .text()
            .map_err(|err| anyhow!("failed to load page text: {err}"))?;
        let search = text
            .search(
                query,
                &PdfSearchOptions::new()
                    .match_case(match_case)
                    .match_whole_word(match_whole_word),
            )
            .map_err(|err| anyhow!("failed to search page text: {err}"))?;
        let mut hits = Vec::new();

        for segments in search.iter(PdfSearchDirection::SearchForward) {
            let mut rects = Vec::new();
            let mut snippet = String::new();

            for segment in segments.iter() {
                let segment_text = segment.text();
                if !snippet.is_empty() {
                    snippet.push(' ');
                }
                snippet.push_str(segment_text.trim());
                rects.push(PdfRectData::from_pdf_rect(segment.bounds()));
            }

            hits.push(SearchHit {
                page_index,
                snippet: snippet.trim().to_owned(),
                rects,
            });
        }

        Ok(hits)
    }

    pub fn text_rects_for_page(&self, page_index: usize) -> Result<Vec<PdfRectData>> {
        let page = self
            .document
            .pages()
            .get(page_index as u16)
            .with_context(|| format!("page index {page_index} is out of bounds"))?;
        let text = page
            .text()
            .map_err(|err| anyhow!("failed to load page text: {err}"))?;

        Ok(text
            .segments()
            .iter()
            .map(|segment| PdfRectData::from_pdf_rect(segment.bounds()))
            .collect())
    }

    #[instrument(skip(self))]
    pub fn render_page_image(&self, request: &RenderRequest) -> Result<RenderedPageImage> {
        let started = Instant::now();
        let page = self
            .document
            .pages()
            .get(request.page_index as u16)
            .with_context(|| format!("page index {} is out of bounds", request.page_index))?;

        let page_size = self
            .page_size(request.page_index)
            .context("missing page size metadata")?;
        let target_width = ((page_size.width * request.zoom).max(1.0).round() as i32).max(1);

        let render = page.render_with_config(
            &request
                .preset
                .apply(
                    PdfRenderConfig::new()
                        .set_target_width(target_width)
                        .clear_before_rendering(true),
                )
                .render_form_data(true),
        );

        let image = dynamic_image_to_color_image(
            render
                .map_err(|err| anyhow!("Pdfium render failed: {err}"))?
                .as_image(),
        );
        let elapsed = started.elapsed();

        debug!(
            page_index = request.page_index,
            zoom = request.zoom,
            preset = request.preset.as_str(),
            elapsed_ms = elapsed.as_secs_f64() * 1000.0,
            "rendered full PDF page"
        );

        Ok(RenderedPageImage {
            image,
            elapsed,
            mode: RenderMode::FullPage,
        })
    }

    #[instrument(skip(self))]
    pub fn render_thumbnail(
        &self,
        page_index: usize,
        size: i32,
        preset: RenderPreset,
    ) -> Result<RenderedPageImage> {
        let started = Instant::now();
        let page = self
            .document
            .pages()
            .get(page_index as u16)
            .with_context(|| format!("page index {page_index} is out of bounds"))?;

        let render = page.render_with_config(&preset.apply(PdfRenderConfig::new().thumbnail(size)));
        let image = dynamic_image_to_color_image(
            render
                .map_err(|err| anyhow!("Pdfium thumbnail render failed: {err}"))?
                .as_image(),
        );

        Ok(RenderedPageImage {
            image,
            elapsed: started.elapsed(),
            mode: RenderMode::Thumbnail,
        })
    }

    #[instrument(skip(self))]
    pub fn render_tile(&self, request: &TileRenderRequest) -> Result<RenderedTileImage> {
        let started = Instant::now();
        let page = self
            .document
            .pages()
            .get(request.page_index as u16)
            .with_context(|| format!("page index {} is out of bounds", request.page_index))?;

        let scale = request.full_width as f32
            / self
                .page_size(request.page_index)
                .context("missing page size metadata")?
                .width;

        let config = request
            .preset
            .apply(
                PdfRenderConfig::new()
                    .set_fixed_size(request.tile_width, request.tile_height)
                    .clear_before_rendering(true),
            )
            .transform(
                scale,
                0.0,
                0.0,
                scale,
                -(request.x as f32),
                -(request.y as f32),
            )
            .map_err(|err| anyhow!("failed to build tile transform: {err}"))?;

        let render = page.render_with_config(&config);
        let image = dynamic_image_to_color_image(
            render
                .map_err(|err| anyhow!("Pdfium tiled render failed: {err}"))?
                .as_image(),
        );

        Ok(RenderedTileImage {
            image,
            elapsed: started.elapsed(),
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

fn dynamic_image_to_color_image(image: image::DynamicImage) -> ColorImage {
    let size = [image.width() as usize, image.height() as usize];
    let rgba = image.into_rgba8();
    ColorImage::from_rgba_unmultiplied(size, rgba.as_raw())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RenderPreset {
    Balanced,
    Crisp,
    Grayscale,
    Fast,
}

impl RenderPreset {
    pub fn all() -> &'static [Self] {
        &[Self::Balanced, Self::Crisp, Self::Grayscale, Self::Fast]
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Balanced => "balanced",
            Self::Crisp => "crisp",
            Self::Grayscale => "grayscale",
            Self::Fast => "fast",
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Balanced => "Balanced",
            Self::Crisp => "Crisp",
            Self::Grayscale => "Grayscale",
            Self::Fast => "Fast",
        }
    }

    pub fn from_name(name: &str) -> Self {
        match name.trim().to_ascii_lowercase().as_str() {
            "crisp" => Self::Crisp,
            "grayscale" => Self::Grayscale,
            "fast" => Self::Fast,
            _ => Self::Balanced,
        }
    }

    fn apply(&self, config: PdfRenderConfig) -> PdfRenderConfig {
        match self {
            Self::Balanced => config
                .use_lcd_text_rendering(true)
                .set_text_smoothing(true)
                .set_image_smoothing(true)
                .set_path_smoothing(true),
            Self::Crisp => config
                .use_lcd_text_rendering(true)
                .set_text_smoothing(false)
                .set_image_smoothing(true)
                .set_path_smoothing(true)
                .disable_native_text_rendering(true),
            Self::Grayscale => config
                .use_grayscale_rendering(true)
                .set_image_smoothing(true)
                .set_path_smoothing(true),
            Self::Fast => config
                .render_annotations(false)
                .render_form_data(false)
                .set_image_smoothing(false)
                .set_path_smoothing(false)
                .set_text_smoothing(false)
                .use_print_quality(false),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct RenderRequest {
    pub page_index: usize,
    pub zoom: f32,
    pub preset: RenderPreset,
}

#[derive(Debug, Clone, Copy)]
pub struct TileRenderRequest {
    pub page_index: usize,
    pub full_width: i32,
    pub x: i32,
    pub y: i32,
    pub tile_width: i32,
    pub tile_height: i32,
    pub preset: RenderPreset,
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

#[derive(Debug, Clone, Copy)]
pub struct PageSizePoints {
    pub width: f32,
    pub height: f32,
}

#[derive(Debug, Clone)]
pub struct SearchHit {
    pub page_index: usize,
    pub snippet: String,
    pub rects: Vec<PdfRectData>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PdfRectData {
    pub bottom: f32,
    pub left: f32,
    pub top: f32,
    pub right: f32,
}

impl PdfRectData {
    pub fn from_pdf_rect(rect: PdfRect) -> Self {
        Self {
            bottom: rect.bottom().value,
            left: rect.left().value,
            top: rect.top().value,
            right: rect.right().value,
        }
    }

    pub fn to_pdf_rect(self) -> PdfRect {
        PdfRect::new_from_values(self.bottom, self.left, self.top, self.right)
    }
}

#[derive(Debug, Clone)]
pub struct RenderedPageImage {
    pub image: ColorImage,
    pub elapsed: Duration,
    pub mode: RenderMode,
}

#[derive(Debug, Clone)]
pub struct RenderedTileImage {
    pub image: ColorImage,
    pub elapsed: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderMode {
    FullPage,
    Thumbnail,
    Tiled,
}
