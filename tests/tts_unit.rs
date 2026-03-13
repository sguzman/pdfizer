mod config {
    #![allow(dead_code)]

    include!("../src/config.rs");
}

mod pdf {
    #![allow(dead_code)]

    include!("../src/pdf.rs");
}

mod tts {
    #![allow(dead_code)]

    include!("../src/tts.rs");

    use crate::pdf::TextSegmentData;

    fn sample_canonical_text(text: &str) -> CanonicalTtsTextArtifact {
        CanonicalTtsTextArtifact {
            text: text.into(),
            pages: vec![CanonicalPageArtifact {
                page_index: 0,
                range: Some(TextRange {
                    start: 0,
                    end: text.len(),
                }),
                blocks: vec![CanonicalBlockArtifact {
                    page_index: 0,
                    block_index: 0,
                    text: text.into(),
                    range: TextRange {
                        start: 0,
                        end: text.len(),
                    },
                    lines: vec![CanonicalLineArtifact {
                        page_index: 0,
                        block_index: 0,
                        line_index: 0,
                        text: text.into(),
                        range: TextRange {
                            start: 0,
                            end: text.len(),
                        },
                        tokens: text
                            .split_whitespace()
                            .enumerate()
                            .scan(0usize, |cursor, (token_index, token)| {
                                if *cursor > 0 {
                                    *cursor += 1;
                                }
                                let start = *cursor;
                                *cursor += token.len();
                                Some(CanonicalTokenArtifact {
                                    page_index: 0,
                                    block_index: 0,
                                    line_index: 0,
                                    token_index,
                                    text: token.into(),
                                    range: TextRange {
                                        start,
                                        end: *cursor,
                                    },
                                })
                            })
                            .collect(),
                    }],
                }],
            }],
            block_count: 1,
            line_count: 1,
            token_count: text.split_whitespace().count(),
        }
    }

    fn sample_classification(mode: PdfTtsMode, _confidence: f32) -> ClassificationSummary {
        ClassificationSummary {
            coverage_ratio: 1.0,
            duplicate_ratio: 0.0,
            boilerplate_ratio: 0.0,
            avg_chars_per_text_page: 100.0,
            avg_segments_per_text_page: 32.0,
            reason: mode.label().into(),
        }
    }

    fn sample_settings() -> TtsSynthesisSettings {
        TtsSynthesisSettings {
            language: "en".into(),
            voice: "default".into(),
            rate: 1.0,
            volume: 1.0,
            sentence_pause_ms: 140,
        }
    }

    fn sample_analysis(
        mode: PdfTtsMode,
        confidence: f32,
        text: &str,
        sentences: Vec<SentencePlan>,
    ) -> TtsAnalysisArtifacts {
        TtsAnalysisArtifacts {
            source_path: PathBuf::from("fixture.pdf"),
            source_fingerprint: "abc".into(),
            generated_at_unix_secs: 0,
            text_source: TtsTextSourceKind::Embedded,
            ocr_trust: None,
            ocr_confidence: None,
            ocr_artifact_path: None,
            mode,
            confidence,
            classification: sample_classification(mode, confidence),
            tts_text: text.into(),
            canonical_text: sample_canonical_text(text),
            sentences,
            pages: Vec::new(),
            stats: NormalizationStats::default(),
            analysis_scope: AnalysisScope {
                start_page: 0,
                end_page: 0,
                full_document: true,
            },
            artifact_path: None,
        }
    }

    fn sample_ocr_artifacts(path: PathBuf) -> OcrArtifacts {
        OcrArtifacts {
            source_path: PathBuf::from("fixture.pdf"),
            source_fingerprint: "abc".into(),
            generated_at_unix_secs: 0,
            confidence: 0.91,
            trust_class: OcrTrustClass::OcrMixedTrust,
            pages: vec![OcrPageArtifact {
                page_index: 0,
                confidence: 0.91,
                blocks: vec![OcrBlockArtifact {
                    page_index: 0,
                    block_index: 0,
                    text: "Scanned text line".into(),
                    bounds: PdfRectData {
                        left: 10.0,
                        right: 90.0,
                        top: 100.0,
                        bottom: 80.0,
                    },
                    confidence: 0.91,
                    lines: vec![OcrLineArtifact {
                        page_index: 0,
                        block_index: 0,
                        line_index: 0,
                        text: "Scanned text line".into(),
                        bounds: PdfRectData {
                            left: 10.0,
                            right: 90.0,
                            top: 100.0,
                            bottom: 80.0,
                        },
                        confidence: 0.91,
                        tokens: vec![
                            OcrTokenArtifact {
                                page_index: 0,
                                block_index: 0,
                                line_index: 0,
                                token_index: 0,
                                text: "Scanned".into(),
                                bounds: PdfRectData {
                                    left: 10.0,
                                    right: 42.0,
                                    top: 100.0,
                                    bottom: 80.0,
                                },
                                confidence: 0.9,
                            },
                            OcrTokenArtifact {
                                page_index: 0,
                                block_index: 0,
                                line_index: 0,
                                token_index: 1,
                                text: "text".into(),
                                bounds: PdfRectData {
                                    left: 44.0,
                                    right: 62.0,
                                    top: 100.0,
                                    bottom: 80.0,
                                },
                                confidence: 0.92,
                            },
                            OcrTokenArtifact {
                                page_index: 0,
                                block_index: 0,
                                line_index: 0,
                                token_index: 2,
                                text: "line".into(),
                                bounds: PdfRectData {
                                    left: 64.0,
                                    right: 90.0,
                                    top: 100.0,
                                    bottom: 80.0,
                                },
                                confidence: 0.91,
                            },
                        ],
                    }],
                }],
            }],
            artifact_path: Some(path),
        }
    }

    #[test]
    fn normalization_replaces_ligatures_and_soft_hyphens() {
        let config = AppConfig::default();
        let repeated = HashSet::new();
        let normalized = normalize_page_text(
            "of\u{FB01}ce co\u{00AD}operate\n\nHeader",
            &repeated,
            &config,
        );

        assert!(normalized.text.contains("office"));
        assert!(normalized.text.contains("cooperate"));
        assert_eq!(normalized.stats.ligatures_replaced, 1);
        assert_eq!(normalized.stats.soft_hyphens_removed, 1);
    }

    #[test]
    fn normalization_suppresses_repeated_edge_lines_and_duplicates() {
        let config = AppConfig::default();
        let repeated = HashSet::from([String::from("Header")]);
        let normalized = normalize_page_text("Header\nAlpha\nAlpha\nBeta", &repeated, &config);

        assert_eq!(normalized.text, "Alpha Beta");
        assert_eq!(normalized.stats.repeated_edge_lines_removed, 1);
        assert_eq!(normalized.stats.duplicate_lines_removed, 1);
    }

    #[test]
    fn sentence_splitter_respects_abbreviations() {
        let mut config = AppConfig::default();
        config.tts.abbreviations = vec!["dr.".into()];

        let sentences = build_sentence_plan(
            &sample_canonical_text("Dr. Smith arrived. Then he left."),
            "fixture",
            &config,
        );

        assert_eq!(sentences.len(), 2);
        assert_eq!(sentences[0].text, "Dr. Smith arrived.");
        assert_eq!(sentences[1].text, "Then he left.");
    }

    #[test]
    fn classifier_marks_empty_documents_as_ocr_required() {
        let config = AppConfig::default();
        let stats = NormalizationStats::default();
        let classification = classify_pdf_for_tts(4, &[], &stats, &config);

        assert_eq!(classification.mode, PdfTtsMode::OcrRequired);
        assert!(classification.confidence < 0.1);
        assert_eq!(classification.summary.reason, "no_usable_embedded_text");
    }

    #[test]
    fn classifier_marks_clean_text_as_high_trust() {
        let config = AppConfig::default();
        let pages = vec![
            PageTtsArtifact {
                page_index: 0,
                original_char_count: 400,
                normalized_char_count: 380,
                segment_count: 120,
                duplicate_lines_removed: 0,
                repeated_edge_lines_removed: 0,
                empty_after_normalization: false,
                range: Some(TextRange { start: 0, end: 380 }),
            },
            PageTtsArtifact {
                page_index: 1,
                original_char_count: 420,
                normalized_char_count: 390,
                segment_count: 130,
                duplicate_lines_removed: 0,
                repeated_edge_lines_removed: 0,
                empty_after_normalization: false,
                range: Some(TextRange {
                    start: 382,
                    end: 772,
                }),
            },
        ];
        let stats = NormalizationStats {
            pages_with_text: 2,
            empty_pages: 0,
            original_chars: 820,
            normalized_chars: 770,
            sentence_count: 18,
            ..NormalizationStats::default()
        };

        let classification = classify_pdf_for_tts(2, &pages, &stats, &config);
        assert_eq!(classification.mode, PdfTtsMode::HighTextTrust);
        assert!(classification.confidence > 0.7);
        assert_eq!(
            classification.summary.reason,
            "embedded_text_meets_high_trust_thresholds"
        );
    }

    #[test]
    fn prefetch_plan_respects_window() {
        let analysis = sample_analysis(
            PdfTtsMode::HighTextTrust,
            0.9,
            "a b c",
            (0..5)
                .map(|index| SentencePlan {
                    id: index as u64,
                    text: format!("Sentence {index}."),
                    range: TextRange {
                        start: index,
                        end: index + 1,
                    },
                    page_range: PageRange {
                        start_page: 0,
                        end_page: 0,
                    },
                    unit_kind: SentenceUnitKind::Sentence,
                })
                .collect(),
        );

        assert_eq!(build_prefetch_plan(&analysis, 2, 2), vec![2, 3]);
        assert_eq!(build_prefetch_plan(&analysis, 4, 8), vec![4]);

        let budgeted = build_prefetch_plan_with_budget(&analysis, 0, 5, 2_000, &sample_settings());
        assert!(!budgeted.sentence_indexes.is_empty());
        assert!(budgeted.estimated_duration_ms_total <= 2_000);
    }

    #[test]
    fn sentence_duration_estimate_has_reasonable_floor() {
        let settings = sample_settings();
        assert!(estimate_sentence_duration_ms("Hi.", &settings) >= 900);
        assert!(
            estimate_sentence_duration_ms("This is a somewhat longer sentence.", &settings) > 900
        );
    }

    #[test]
    fn engine_cache_key_changes_with_settings_and_text() {
        let analysis = sample_analysis(
            PdfTtsMode::HighTextTrust,
            0.9,
            "Sentence zero.",
            vec![SentencePlan {
                id: 42,
                text: "Sentence zero.".into(),
                range: TextRange { start: 0, end: 14 },
                page_range: PageRange {
                    start_page: 0,
                    end_page: 0,
                },
                unit_kind: SentenceUnitKind::Sentence,
            }],
        );
        let engine = create_tts_engine(TtsEngineKind::TonePreview);
        let sentence = &analysis.sentences[0];
        let key_a = engine.build_cache_key(&analysis, sentence, &sample_settings());
        let mut changed_voice = sample_settings();
        changed_voice.voice = "narrator".into();
        let key_b = engine.build_cache_key(&analysis, sentence, &changed_voice);
        let mut changed_rate = sample_settings();
        changed_rate.rate = 1.15;
        let key_c = engine.build_cache_key(&analysis, sentence, &changed_rate);

        assert_ne!(key_a, key_b);
        assert_ne!(key_a, key_c);
        assert!(key_a.stem().contains("tone_preview"));
    }

    #[test]
    fn create_tts_engine_returns_expected_backend_kind() {
        assert_eq!(
            create_tts_engine(TtsEngineKind::DryRun).kind(),
            TtsEngineKind::DryRun
        );
        assert_eq!(
            create_tts_engine(TtsEngineKind::TonePreview).kind(),
            TtsEngineKind::TonePreview
        );
    }

    #[test]
    fn sentence_ids_are_stable_for_same_canonical_text() {
        let config = AppConfig::default();
        let canonical = sample_canonical_text("Alpha. Beta.");

        let first = build_sentence_plan(&canonical, "fixture", &config);
        let second = build_sentence_plan(&canonical, "fixture", &config);

        assert_eq!(
            first.iter().map(|sentence| sentence.id).collect::<Vec<_>>(),
            second
                .iter()
                .map(|sentence| sentence.id)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn canonical_text_artifact_preserves_token_provenance() {
        let canonical = sample_canonical_text("Alpha beta");
        assert_eq!(canonical.block_count, 1);
        assert_eq!(canonical.line_count, 1);
        assert_eq!(canonical.token_count, 2);
        assert_eq!(canonical.pages[0].blocks[0].lines[0].tokens[1].text, "beta");
    }

    #[test]
    fn extractor_reorders_simple_two_column_layout() {
        let config = AppConfig::default();
        let extracted = extract_page_text_for_tts(
            &[
                TextSegmentData {
                    text: "Right one".into(),
                    rect: PdfRectData {
                        left: 300.0,
                        right: 360.0,
                        top: 780.0,
                        bottom: 768.0,
                    },
                },
                TextSegmentData {
                    text: "Left one".into(),
                    rect: PdfRectData {
                        left: 40.0,
                        right: 96.0,
                        top: 780.0,
                        bottom: 768.0,
                    },
                },
                TextSegmentData {
                    text: "Left two".into(),
                    rect: PdfRectData {
                        left: 40.0,
                        right: 96.0,
                        top: 744.0,
                        bottom: 732.0,
                    },
                },
                TextSegmentData {
                    text: "Right two".into(),
                    rect: PdfRectData {
                        left: 300.0,
                        right: 362.0,
                        top: 744.0,
                        bottom: 732.0,
                    },
                },
            ],
            &config,
        );

        assert!(extracted.text.starts_with("Left one"));
        let left_one = extracted.text.find("Left one").unwrap_or(usize::MAX);
        let left_two = extracted.text.find("Left two").unwrap_or(usize::MAX);
        let right_one = extracted.text.find("Right one").unwrap_or(usize::MAX);
        let right_two = extracted.text.find("Right two").unwrap_or(usize::MAX);
        assert!(left_one < left_two);
        assert!(left_two < right_one);
        assert!(right_one < right_two);
        assert_eq!(extracted.stats.column_reorders, 1);
    }

    #[test]
    fn extractor_suppresses_rotated_and_duplicate_segments() {
        let config = AppConfig::default();
        let extracted = extract_page_text_for_tts(
            &[
                TextSegmentData {
                    text: "Alpha".into(),
                    rect: PdfRectData {
                        left: 10.0,
                        right: 60.0,
                        top: 100.0,
                        bottom: 90.0,
                    },
                },
                TextSegmentData {
                    text: "Alpha".into(),
                    rect: PdfRectData {
                        left: 10.0,
                        right: 60.0,
                        top: 100.0,
                        bottom: 90.0,
                    },
                },
                TextSegmentData {
                    text: "Rotated".into(),
                    rect: PdfRectData {
                        left: 200.0,
                        right: 205.0,
                        top: 160.0,
                        bottom: 100.0,
                    },
                },
            ],
            &config,
        );

        assert_eq!(extracted.text, "Alpha");
        assert_eq!(extracted.stats.duplicate_segments_suppressed, 1);
        assert_eq!(extracted.stats.rotated_segments_suppressed, 1);
    }

    #[test]
    fn sentence_splitter_respects_citations_and_lowercase_continuations() {
        let config = AppConfig::default();
        let sentences = split_sentences("See Smith et al. for context. then continue.", &config);

        assert_eq!(sentences.len(), 1);
    }

    #[test]
    fn sentence_planner_falls_back_to_blocks_when_punctuation_is_weak() {
        let mut config = AppConfig::default();
        config.tts.sentence_break_on_double_newline = false;
        config.tts.block_fallback_min_chars = 8;
        let canonical = CanonicalTtsTextArtifact {
            text: "alpha beta gamma delta epsilon\n\nzeta eta theta iota kappa".into(),
            pages: vec![CanonicalPageArtifact {
                page_index: 0,
                range: Some(TextRange { start: 0, end: 58 }),
                blocks: vec![
                    CanonicalBlockArtifact {
                        page_index: 0,
                        block_index: 0,
                        text: "alpha beta gamma delta epsilon".into(),
                        range: TextRange { start: 0, end: 30 },
                        lines: vec![],
                    },
                    CanonicalBlockArtifact {
                        page_index: 0,
                        block_index: 1,
                        text: "zeta eta theta iota kappa".into(),
                        range: TextRange { start: 32, end: 58 },
                        lines: vec![],
                    },
                ],
            }],
            block_count: 2,
            line_count: 2,
            token_count: 10,
        };

        let planned = build_sentence_plan(&canonical, "fixture", &config);
        assert_eq!(planned.len(), 2);
        assert!(
            planned
                .iter()
                .all(|sentence| sentence.unit_kind == SentenceUnitKind::BlockFallback)
        );
    }

    #[test]
    fn normalization_marks_table_and_caption_like_blocks() {
        let config = AppConfig::default();
        let repeated = HashSet::new();
        let normalized = normalize_page_text(
            "Table 1 Revenue 2024 2025\n10 20 30 40\n\nFigure 1 Sample caption",
            &repeated,
            &config,
        );

        assert!(normalized.stats.table_like_blocks >= 1);
        assert!(normalized.stats.caption_like_blocks >= 1);
    }

    #[test]
    fn normalization_joins_hyphenated_line_wraps() {
        let config = AppConfig::default();
        let repeated = HashSet::new();
        let normalized = normalize_page_text("coordi-\nnated text", &repeated, &config);

        assert!(normalized.text.contains("coordinated text"));
        assert_eq!(normalized.stats.joined_hyphenations, 1);
    }

    #[test]
    fn prepare_sentence_clip_writes_tone_preview_files() {
        let temp = tempfile::tempdir().unwrap();
        let source_path = temp.path().join("fixture.pdf");
        std::fs::write(&source_path, b"%PDF-1.4").unwrap();

        let mut config = AppConfig::default();
        config.tts.audio_cache_dir = temp.path().join("audio").display().to_string();

        let analysis = sample_analysis(
            PdfTtsMode::HighTextTrust,
            0.9,
            "Sentence zero.",
            vec![SentencePlan {
                id: 42,
                text: "Sentence zero.".into(),
                range: TextRange { start: 0, end: 14 },
                page_range: PageRange {
                    start_page: 0,
                    end_page: 0,
                },
                unit_kind: SentenceUnitKind::Sentence,
            }],
        );
        let mut analysis = analysis;
        analysis.source_path = source_path;

        let clip =
            prepare_sentence_clip(&config, &analysis, 0, TtsEngineKind::TonePreview).unwrap();
        assert!(clip.manifest_path.exists());
        assert!(clip.audio_path.as_ref().is_some_and(|path| path.exists()));
    }

    #[test]
    fn sync_target_exact_match_collects_rects() {
        let segments = vec![
            TextSegmentData {
                text: "Hello".into(),
                rect: PdfRectData {
                    bottom: 10.0,
                    left: 10.0,
                    top: 20.0,
                    right: 40.0,
                },
            },
            TextSegmentData {
                text: "world".into(),
                rect: PdfRectData {
                    bottom: 10.0,
                    left: 42.0,
                    top: 20.0,
                    right: 70.0,
                },
            },
        ];

        let target = sync_target_for_page(
            0,
            0,
            42,
            &build_sync_tokens("hello world"),
            "hello world",
            &segments,
        );

        assert_eq!(target.confidence, SentenceSyncConfidence::ExactSentence);
        assert_eq!(target.rects.len(), 2);
        assert_eq!(target.lineage.len(), 2);
        assert!(target.score_breakdown.geometry_compactness > 0.0);
    }

    #[test]
    fn sync_target_degrades_to_block_fallback() {
        let segments = vec![
            TextSegmentData {
                text: "The quick brown".into(),
                rect: PdfRectData {
                    bottom: 10.0,
                    left: 10.0,
                    top: 20.0,
                    right: 80.0,
                },
            },
            TextSegmentData {
                text: "fox jumps".into(),
                rect: PdfRectData {
                    bottom: 24.0,
                    left: 10.0,
                    top: 34.0,
                    right: 65.0,
                },
            },
        ];

        let target = sync_target_for_page(
            0,
            0,
            42,
            &build_sync_tokens("quick fox leaps"),
            "quick fox leaps",
            &segments,
        );

        assert!(matches!(
            target.confidence,
            SentenceSyncConfidence::BlockFallback | SentenceSyncConfidence::FuzzySentence
        ));
        assert_eq!(target.page_index, Some(0));
    }

    #[test]
    fn sentence_budget_window_keeps_local_radius() {
        let window = sentence_budget_window(5, 12, 2);
        assert_eq!(window, HashSet::from([3, 4, 5, 6, 7]));

        let near_start = sentence_budget_window(0, 3, 2);
        assert_eq!(near_start, HashSet::from([0, 1, 2]));
    }

    #[test]
    fn deferred_ocr_policy_degrades_to_page_follow() {
        let mut config = AppConfig::default();
        config.tts.ocr_policy = TtsOcrPolicy::Deferred;
        let analysis = sample_analysis(
            PdfTtsMode::OcrRequired,
            0.2,
            "Scanned fallback",
            vec![SentencePlan {
                id: 1,
                text: "Scanned fallback".into(),
                range: TextRange { start: 0, end: 16 },
                page_range: PageRange {
                    start_page: 0,
                    end_page: 0,
                },
                unit_kind: SentenceUnitKind::Sentence,
            }],
        );

        let policy = evaluate_runtime_policy(&config, &analysis);
        assert!(policy.allow_playback);
        assert!(!policy.allow_rect_highlights);
        assert!(!policy.allow_sync_prefetch);
        assert_eq!(
            policy.max_sync_confidence,
            SentenceSyncConfidence::PageFallback
        );

        let adapted = apply_runtime_policy(
            &SentenceSyncTarget {
                sentence_index: 0,
                sentence_id: 1,
                confidence: SentenceSyncConfidence::ExactSentence,
                page_index: Some(0),
                rects: vec![PdfRectData {
                    left: 1.0,
                    right: 2.0,
                    top: 3.0,
                    bottom: 0.5,
                }],
                fallback_reason: "exact_substring_match".into(),
                score: 1.0,
                score_breakdown: SyncScoreBreakdown {
                    text_similarity: 1.0,
                    reading_order: 1.0,
                    geometry_compactness: 1.0,
                    page_continuity: 1.0,
                    total: 1.0,
                },
                lineage: vec![SyncTokenLineage {
                    token: "Scanned".into(),
                    page_index: 0,
                    block_index: 0,
                    line_index: 0,
                    token_index: 0,
                    rect: PdfRectData {
                        left: 1.0,
                        right: 2.0,
                        top: 3.0,
                        bottom: 0.5,
                    },
                }],
                artifact_path: None,
            },
            &policy,
        );
        assert_eq!(adapted.confidence, SentenceSyncConfidence::PageFallback);
        assert!(adapted.rects.is_empty());
    }

    #[test]
    fn disabled_ocr_policy_blocks_playback() {
        let mut config = AppConfig::default();
        config.tts.ocr_policy = TtsOcrPolicy::Disabled;
        let analysis = sample_analysis(PdfTtsMode::OcrRequired, 0.1, "", Vec::new());

        let policy = evaluate_runtime_policy(&config, &analysis);
        assert!(!policy.allow_playback);
        assert_eq!(policy.max_sync_confidence, SentenceSyncConfidence::Missing);
    }

    #[test]
    fn load_ocr_artifacts_reads_separate_sidecar_contract() {
        let temp = tempfile::tempdir().unwrap();
        let source_path = temp.path().join("fixture.pdf");
        std::fs::write(&source_path, b"%PDF-1.4").unwrap();
        let mut config = AppConfig::default();
        config.tts.ocr_artifacts_dir = temp.path().join("ocr").display().to_string();

        let ocr_path = config.tts_ocr_artifact_path(&source_path).unwrap();
        std::fs::create_dir_all(ocr_path.parent().unwrap()).unwrap();
        std::fs::write(
            &ocr_path,
            toml::to_string_pretty(&sample_ocr_artifacts(ocr_path.clone())).unwrap(),
        )
        .unwrap();

        let loaded = load_ocr_artifacts(&config, &source_path).unwrap().unwrap();
        assert_eq!(loaded.trust_class, OcrTrustClass::OcrMixedTrust);
        assert_eq!(loaded.pages[0].blocks[0].lines[0].tokens.len(), 3);
    }

    #[test]
    fn persist_sync_target_writes_artifact_file() {
        let temp = tempfile::tempdir().unwrap();
        let source_path = temp.path().join("fixture.pdf");
        std::fs::write(&source_path, b"%PDF-1.4").unwrap();
        let mut config = AppConfig::default();
        config.tts.sync_artifacts_dir = temp.path().join("sync").display().to_string();
        let mut analysis = sample_analysis(
            PdfTtsMode::HighTextTrust,
            0.9,
            "Hello world",
            vec![SentencePlan {
                id: 42,
                text: "Hello world".into(),
                range: TextRange { start: 0, end: 11 },
                page_range: PageRange {
                    start_page: 0,
                    end_page: 0,
                },
                unit_kind: SentenceUnitKind::Sentence,
            }],
        );
        analysis.source_path = source_path.clone();
        let target = SentenceSyncTarget {
            sentence_index: 0,
            sentence_id: 42,
            confidence: SentenceSyncConfidence::ExactSentence,
            page_index: Some(0),
            rects: vec![PdfRectData {
                left: 1.0,
                right: 2.0,
                top: 3.0,
                bottom: 0.5,
            }],
            fallback_reason: "exact_substring_match".into(),
            score: 1.0,
            score_breakdown: SyncScoreBreakdown {
                text_similarity: 1.0,
                reading_order: 1.0,
                geometry_compactness: 1.0,
                page_continuity: 1.0,
                total: 1.0,
            },
            lineage: vec![SyncTokenLineage {
                token: "Hello".into(),
                page_index: 0,
                block_index: 0,
                line_index: 0,
                token_index: 0,
                rect: PdfRectData {
                    left: 1.0,
                    right: 2.0,
                    top: 3.0,
                    bottom: 0.5,
                },
            }],
            artifact_path: None,
        };

        let path = persist_sync_target(&config, &analysis, &target).unwrap();
        assert!(path.exists());
        let contents = std::fs::read_to_string(path).unwrap();
        assert!(contents.contains("score_breakdown"));
        assert!(contents.contains("lineage"));
    }
}
