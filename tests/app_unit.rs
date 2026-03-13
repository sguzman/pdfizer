use super::*;

#[test]
fn parse_rgba_hex_supports_rgb_and_rgba() {
    let rgb = parse_rgba_hex("#112233").expect("rgb color should parse");
    assert_eq!([rgb.r(), rgb.g(), rgb.b(), rgb.a()], [0x11, 0x22, 0x33, 0xff]);

    let rgba = parse_rgba_hex("#44556677").expect("rgba color should parse");
    assert_eq!([rgba.r(), rgba.g(), rgba.b(), rgba.a()], [0x44, 0x55, 0x66, 0x77]);
}

#[test]
fn coalesce_line_rects_merges_adjacent_rows() {
    let rects = vec![
        Rect::from_min_max(Pos2::new(10.0, 10.0), Pos2::new(30.0, 20.0)),
        Rect::from_min_max(Pos2::new(32.0, 11.0), Pos2::new(60.0, 19.0)),
        Rect::from_min_max(Pos2::new(12.0, 40.0), Pos2::new(42.0, 50.0)),
    ];

    let lines = coalesce_line_rects(&rects);
    assert_eq!(lines.len(), 2);
    assert!(lines[0].left() <= 10.0);
    assert!(lines[0].right() >= 60.0);
}

#[test]
fn centered_offset_clamps_at_zero() {
    assert_eq!(centered_offset(40.0, 200.0), 0.0);
    assert_eq!(centered_offset(400.0, 200.0), 300.0);
}

#[test]
fn rect_distance_score_prefers_nearby_rects() {
    let anchor = PdfRectData {
        left: 10.0,
        right: 20.0,
        top: 20.0,
        bottom: 10.0,
    };
    let near = PdfRectData {
        left: 12.0,
        right: 22.0,
        top: 22.0,
        bottom: 12.0,
    };
    let far = PdfRectData {
        left: 120.0,
        right: 140.0,
        top: 140.0,
        bottom: 120.0,
    };

    assert!(rect_distance_score(anchor, near) < rect_distance_score(anchor, far));
}

#[test]
fn pdf_rect_to_screen_rect_scales_consistently() {
    let page_size = PageSizePoints {
        width: 200.0,
        height: 400.0,
    };
    let rect = PdfRectData {
        left: 50.0,
        right: 100.0,
        top: 300.0,
        bottom: 200.0,
    };
    let image = ColorImage::filled([200, 400], Color32::WHITE);
    let small = pdf_rect_to_screen_rect(
        rect,
        page_size,
        Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(200.0, 400.0)),
        &image,
    );
    let large = pdf_rect_to_screen_rect(
        rect,
        page_size,
        Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(400.0, 800.0)),
        &image,
    );

    assert!((large.width() / small.width() - 2.0).abs() < 0.001);
    assert!((large.height() / small.height() - 2.0).abs() < 0.001);
    assert!((large.left() / small.left() - 2.0).abs() < 0.001);
}

#[test]
fn viewport_contains_target_respects_safe_region() {
    let offset = Vec2::new(100.0, 200.0);
    let viewport = Vec2::new(300.0, 500.0);

    assert!(viewport_contains_target(offset, viewport, Vec2::new(250.0, 450.0), 0.18));
    assert!(!viewport_contains_target(offset, viewport, Vec2::new(110.0, 230.0), 0.18));
}

#[test]
fn session_state_round_trips_tts_resume_fields() {
    let session = SessionState {
        last_document: Some(PathBuf::from("/tmp/test.pdf")),
        last_page: 4,
        zoom: 1.5,
        preset: "balanced".into(),
        view_mode: "continuous".into(),
        compare_enabled: false,
        compare_preset: "crisp".into(),
        tts_sentence_id: Some(42),
        focus_rect: Some(PdfRectData {
            left: 1.0,
            right: 2.0,
            top: 3.0,
            bottom: 0.5,
        }),
        follow_mode: true,
        follow_pin_to_center: false,
        highlights_enabled: true,
    };

    let serialized = toml::to_string(&session).expect("session should serialize");
    let restored: SessionState = toml::from_str(&serialized).expect("session should deserialize");

    assert_eq!(restored.tts_sentence_id, Some(42));
    assert_eq!(restored.focus_rect.expect("focus rect").top, 3.0);
    assert!(restored.follow_mode);
    assert!(!restored.follow_pin_to_center);
    assert!(restored.highlights_enabled);
}
