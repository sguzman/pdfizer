use std::path::PathBuf;

use serde::Deserialize;

#[test]
fn fixture_pdf_has_pdf_header() {
    let path = fixture_path();
    let contents = std::fs::read(&path).expect("fixture PDF should exist");

    assert!(contents.starts_with(b"%PDF-"));
}

#[test]
fn fixture_pdf_is_checked_in_under_fixtures() {
    let path = fixture_path();

    assert!(path.exists());
    assert!(path.ends_with("tests/fixtures/minimal.pdf"));
}

#[test]
fn tts_fixture_corpus_manifest_references_existing_ready_fixtures() {
    let path = fixture_dir().join("pdf_tts_fixture_corpus.toml");
    let contents = std::fs::read_to_string(&path).expect("fixture corpus manifest should exist");
    let manifest: FixtureCorpusManifest =
        toml::from_str(&contents).expect("fixture corpus manifest should parse");

    assert_eq!(manifest.version, 1);
    assert!(manifest.fixtures.len() >= 6);
    assert!(
        manifest
            .fixtures
            .iter()
            .any(|fixture| fixture.class == "scanned_image_pdf")
    );

    for fixture in manifest
        .fixtures
        .iter()
        .filter(|fixture| fixture.status == "ready" || fixture.status == "seeded")
    {
        let relative = fixture
            .path
            .as_ref()
            .expect("ready fixtures should include a file path");
        let full_path = fixture_dir().join(relative);
        assert!(
            full_path.exists(),
            "missing fixture {}",
            full_path.display()
        );
        assert!(full_path.extension().is_some_and(|ext| ext == "pdf"));
    }
}

fn fixture_path() -> PathBuf {
    fixture_dir().join("minimal.pdf")
}

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

#[derive(Debug, Deserialize)]
struct FixtureCorpusManifest {
    version: u32,
    fixtures: Vec<FixtureEntry>,
}

#[derive(Debug, Deserialize)]
struct FixtureEntry {
    class: String,
    status: String,
    path: Option<String>,
}
