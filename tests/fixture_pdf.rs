use std::path::PathBuf;

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

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/minimal.pdf")
}
