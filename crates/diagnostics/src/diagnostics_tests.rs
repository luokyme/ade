use crate::diagnostic_renderer;
use editor::{
    Editor, EditorSettings,
    actions::{GoToDiagnostic, GoToPreviousDiagnostic, Hover, MoveToBeginning},
    test::{editor_lsp_test_context::EditorLspTestContext, editor_test_context::EditorTestContext},
};
use gpui::TestAppContext;
use indoc::indoc;
use language::{BufferId, Diagnostic, DiagnosticEntryRef, DiagnosticSourceKind};
use lsp::LanguageServerId;
use pretty_assertions::assert_eq;
use project::project_settings::{GoToDiagnosticSeverity, GoToDiagnosticSeverityFilter};
use settings::{Settings, SettingsStore};
use std::time::Duration;
use util::path;

#[ctor::ctor(unsafe)]
fn init_logger() {
    zlog::init_test();
}

#[gpui::test]
async fn active_diagnostics_dismiss_after_invalidation(cx: &mut TestAppContext) {
    init_test(cx);

    let mut cx = EditorTestContext::new(cx).await;
    let lsp_store =
        cx.update_editor(|editor, _, cx| editor.project().unwrap().read(cx).lsp_store());

    cx.set_state(indoc! {"
        ˇfn func(abc def: i32) -> u32 {
        }
    "});

    let message = "`Something's wrong!`";
    cx.update(|_, cx| {
        lsp_store.update(cx, |lsp_store, cx| {
            lsp_store
                .update_diagnostics(
                    LanguageServerId(0),
                    lsp::PublishDiagnosticsParams {
                        uri: lsp::Uri::from_file_path(path!("/root/file")).unwrap(),
                        version: None,
                        diagnostics: vec![lsp::Diagnostic {
                            range: lsp::Range::new(
                                lsp::Position::new(0, 11),
                                lsp::Position::new(0, 12),
                            ),
                            severity: Some(lsp::DiagnosticSeverity::ERROR),
                            message: lsp::DiagnosticMessage::from(message.to_string()),
                            ..Default::default()
                        }],
                    },
                    None,
                    DiagnosticSourceKind::Pushed,
                    &[],
                    cx,
                )
                .unwrap()
        });
    });
    cx.run_until_parked();

    cx.update_editor(|editor, window, cx| {
        editor.go_to_diagnostic(&GoToDiagnostic::default(), window, cx);
        assert_eq!(
            editor.active_diagnostic_message(),
            Some(message),
            "Should have a diagnostics group activated"
        );
    });
    cx.assert_editor_state(indoc! {"
        fn func(abcˇ def: i32) -> u32 {
        }
    "});

    cx.update(|_, cx| {
        lsp_store.update(cx, |lsp_store, cx| {
            lsp_store
                .update_diagnostics(
                    LanguageServerId(0),
                    lsp::PublishDiagnosticsParams {
                        uri: lsp::Uri::from_file_path(path!("/root/file")).unwrap(),
                        version: None,
                        diagnostics: vec![lsp::Diagnostic {
                            range: lsp::Range::new(
                                lsp::Position::new(0, 11),
                                lsp::Position::new(0, 12),
                            ),
                            severity: Some(lsp::DiagnosticSeverity::ERROR),
                            message: lsp::DiagnosticMessage::from(lsp::MarkupContent {
                                kind: lsp::MarkupKind::Markdown,
                                value: message.to_string(),
                            }),
                            ..Default::default()
                        }],
                    },
                    None,
                    DiagnosticSourceKind::Pushed,
                    &[],
                    cx,
                )
                .unwrap()
        });
    });
    cx.run_until_parked();
    cx.update_editor(|editor, _, _| {
        assert_eq!(
            editor.active_diagnostic_message(),
            None,
            "changing only the diagnostic's Markdown rendering should invalidate the active group"
        );
    });

    cx.update_editor(|editor, window, cx| {
        editor.go_to_diagnostic(&GoToDiagnostic::default(), window, cx);
        assert_eq!(editor.active_diagnostic_message(), Some(message));
    });

    cx.update(|_, cx| {
        lsp_store.update(cx, |lsp_store, cx| {
            lsp_store
                .update_diagnostics(
                    LanguageServerId(0),
                    lsp::PublishDiagnosticsParams {
                        uri: lsp::Uri::from_file_path(path!("/root/file")).unwrap(),
                        version: None,
                        diagnostics: Vec::new(),
                    },
                    None,
                    DiagnosticSourceKind::Pushed,
                    &[],
                    cx,
                )
                .unwrap()
        });
    });
    cx.run_until_parked();
    cx.update_editor(|editor, _, _| {
        assert_eq!(editor.active_diagnostic_message(), None);
    });
    cx.assert_editor_state(indoc! {"
        fn func(abcˇ def: i32) -> u32 {
        }
    "});

    cx.update_editor(|editor, window, cx| {
        editor.go_to_diagnostic(&GoToDiagnostic::default(), window, cx);
        assert_eq!(editor.active_diagnostic_message(), None);
    });
    cx.assert_editor_state(indoc! {"
        fn func(abcˇ def: i32) -> u32 {
        }
    "});
}

#[gpui::test]
async fn cycle_through_same_place_diagnostics(cx: &mut TestAppContext) {
    init_test(cx);

    let mut cx = EditorTestContext::new(cx).await;
    let lsp_store =
        cx.update_editor(|editor, _, cx| editor.project().unwrap().read(cx).lsp_store());

    cx.set_state(indoc! {"
        ˇfn func(abc def: i32) -> u32 {
        }
    "});

    cx.update(|_, cx| {
        lsp_store.update(cx, |lsp_store, cx| {
            lsp_store
                .update_diagnostics(
                    LanguageServerId(0),
                    lsp::PublishDiagnosticsParams {
                        uri: lsp::Uri::from_file_path(path!("/root/file")).unwrap(),
                        version: None,
                        diagnostics: vec![
                            lsp::Diagnostic {
                                range: lsp::Range::new(
                                    lsp::Position::new(0, 11),
                                    lsp::Position::new(0, 12),
                                ),
                                severity: Some(lsp::DiagnosticSeverity::ERROR),
                                ..Default::default()
                            },
                            lsp::Diagnostic {
                                range: lsp::Range::new(
                                    lsp::Position::new(0, 12),
                                    lsp::Position::new(0, 15),
                                ),
                                severity: Some(lsp::DiagnosticSeverity::ERROR),
                                ..Default::default()
                            },
                            lsp::Diagnostic {
                                range: lsp::Range::new(
                                    lsp::Position::new(0, 12),
                                    lsp::Position::new(0, 15),
                                ),
                                severity: Some(lsp::DiagnosticSeverity::ERROR),
                                ..Default::default()
                            },
                            lsp::Diagnostic {
                                range: lsp::Range::new(
                                    lsp::Position::new(0, 25),
                                    lsp::Position::new(0, 28),
                                ),
                                severity: Some(lsp::DiagnosticSeverity::ERROR),
                                ..Default::default()
                            },
                        ],
                    },
                    None,
                    DiagnosticSourceKind::Pushed,
                    &[],
                    cx,
                )
                .unwrap()
        });
    });
    cx.run_until_parked();

    //// Backward

    // Fourth diagnostic
    cx.update_editor(|editor, window, cx| {
        editor.go_to_prev_diagnostic(&GoToPreviousDiagnostic::default(), window, cx);
    });
    cx.assert_editor_state(indoc! {"
        fn func(abc def: i32) -> ˇu32 {
        }
    "});

    // Third diagnostic
    cx.update_editor(|editor, window, cx| {
        editor.go_to_prev_diagnostic(&GoToPreviousDiagnostic::default(), window, cx);
    });
    cx.assert_editor_state(indoc! {"
        fn func(abc ˇdef: i32) -> u32 {
        }
    "});

    // Second diagnostic, same place
    cx.update_editor(|editor, window, cx| {
        editor.go_to_prev_diagnostic(&GoToPreviousDiagnostic::default(), window, cx);
    });
    cx.assert_editor_state(indoc! {"
        fn func(abc ˇdef: i32) -> u32 {
        }
    "});

    // First diagnostic
    cx.update_editor(|editor, window, cx| {
        editor.go_to_prev_diagnostic(&GoToPreviousDiagnostic::default(), window, cx);
    });
    cx.assert_editor_state(indoc! {"
        fn func(abcˇ def: i32) -> u32 {
        }
    "});

    // Wrapped over, fourth diagnostic
    cx.update_editor(|editor, window, cx| {
        editor.go_to_prev_diagnostic(&GoToPreviousDiagnostic::default(), window, cx);
    });
    cx.assert_editor_state(indoc! {"
        fn func(abc def: i32) -> ˇu32 {
        }
    "});

    cx.update_editor(|editor, window, cx| {
        editor.move_to_beginning(&MoveToBeginning, window, cx);
    });
    cx.assert_editor_state(indoc! {"
        ˇfn func(abc def: i32) -> u32 {
        }
    "});

    //// Forward

    // First diagnostic
    cx.update_editor(|editor, window, cx| {
        editor.go_to_diagnostic(&GoToDiagnostic::default(), window, cx);
    });
    cx.assert_editor_state(indoc! {"
        fn func(abcˇ def: i32) -> u32 {
        }
    "});

    // Second diagnostic
    cx.update_editor(|editor, window, cx| {
        editor.go_to_diagnostic(&GoToDiagnostic::default(), window, cx);
    });
    cx.assert_editor_state(indoc! {"
        fn func(abc ˇdef: i32) -> u32 {
        }
    "});

    // Third diagnostic, same place
    cx.update_editor(|editor, window, cx| {
        editor.go_to_diagnostic(&GoToDiagnostic::default(), window, cx);
    });
    cx.assert_editor_state(indoc! {"
        fn func(abc ˇdef: i32) -> u32 {
        }
    "});

    // Fourth diagnostic
    cx.update_editor(|editor, window, cx| {
        editor.go_to_diagnostic(&GoToDiagnostic::default(), window, cx);
    });
    cx.assert_editor_state(indoc! {"
        fn func(abc def: i32) -> ˇu32 {
        }
    "});

    // Wrapped around, first diagnostic
    cx.update_editor(|editor, window, cx| {
        editor.go_to_diagnostic(&GoToDiagnostic::default(), window, cx);
    });
    cx.assert_editor_state(indoc! {"
        fn func(abcˇ def: i32) -> u32 {
        }
    "});
}

#[gpui::test]
async fn test_diagnostics_with_links(cx: &mut TestAppContext) {
    init_test(cx);

    let mut cx = EditorTestContext::new(cx).await;

    cx.set_state(indoc! {"
        fn func(abˇc def: i32) -> u32 {
        }
    "});
    let lsp_store =
        cx.update_editor(|editor, _, cx| editor.project().unwrap().read(cx).lsp_store());

    cx.update(|_, cx| {
        lsp_store.update(cx, |lsp_store, cx| {
            lsp_store.update_diagnostics(
                LanguageServerId(0),
                lsp::PublishDiagnosticsParams {
                    uri: lsp::Uri::from_file_path(path!("/root/file")).unwrap(),
                    version: None,
                    diagnostics: vec![lsp::Diagnostic {
                        range: lsp::Range::new(lsp::Position::new(0, 8), lsp::Position::new(0, 12)),
                        severity: Some(lsp::DiagnosticSeverity::ERROR),
                        message: lsp::DiagnosticMessage::from(
                            "we've had problems with <https://link.one>, and <https://link.two> is broken",
                        ),
                        ..Default::default()
                    }],
                },
                None,
                DiagnosticSourceKind::Pushed,
                &[],
                cx,
            )
        })
    }).unwrap();
    cx.run_until_parked();
    cx.update_editor(|editor, window, cx| {
        editor::hover_popover::hover(editor, &Default::default(), window, cx)
    });
    cx.run_until_parked();
    cx.update_editor(|editor, _, _| assert!(editor.hover_state.diagnostic_popover.is_some()))
}

#[gpui::test]
fn test_nearby_related_diagnostic_links_back_to_primary(cx: &mut TestAppContext) {
    init_test(cx);

    let buffer_id = BufferId::new(1).expect("test buffer ID should be nonzero");
    let group_id = 1;
    let diagnostic_group = [
        DiagnosticEntryRef {
            range: text::Point::new(10, 4)..text::Point::new(10, 16),
            diagnostic: &Diagnostic {
                message: "the trait bound `(): HasTest` is not satisfied".into(),
                is_primary: true,
                group_id,
                ..Default::default()
            },
        },
        DiagnosticEntryRef {
            range: text::Point::new(13, 0)..text::Point::new(13, 22),
            diagnostic: &Diagnostic {
                message: "required by a bound in `Test::TestAssoc`".into(),
                group_id,
                ..Default::default()
            },
        },
    ];

    let primary_index = diagnostic_group
        .iter()
        .position(|entry| entry.diagnostic.is_primary)
        .expect("test diagnostic group should contain a primary diagnostic");
    let expected_back_link =
        format!("([back](file://#diagnostic-{buffer_id}-{group_id}-{primary_index}))");

    cx.update(|cx| {
        let blocks = diagnostic_renderer::DiagnosticRenderer::diagnostic_blocks_for_group(
            diagnostic_group.to_vec(),
            buffer_id,
            None,
            cx,
        );

        let [_primary_block, related_block] = blocks.as_slice() else {
            panic!("expected one primary and one related diagnostic block");
        };

        let markdown = related_block.markdown.read(cx).source();

        assert!(
            markdown.contains(&expected_back_link),
            "related diagnostic should link back to the primary diagnostic, got: {}",
            markdown
        );
    });
}

#[gpui::test]
async fn test_markup_content_diagnostic_messages_render_as_markdown(cx: &mut TestAppContext) {
    init_test(cx);

    let mut cx = EditorTestContext::new(cx).await;

    cx.set_state(indoc! {"
        fn func(abˇc def: i32) -> u32 {
        }
    "});
    let lsp_store =
        cx.update_editor(|editor, _, cx| editor.project().unwrap().read(cx).lsp_store());

    cx.update(|_, cx| {
        lsp_store.update(cx, |lsp_store, cx| {
            lsp_store.update_diagnostics(
                LanguageServerId(0),
                lsp::PublishDiagnosticsParams {
                    uri: lsp::Uri::from_file_path(path!("/root/file")).unwrap(),
                    version: None,
                    diagnostics: vec![
                        lsp::Diagnostic {
                            range: lsp::Range::new(
                                lsp::Position::new(0, 8),
                                lsp::Position::new(0, 12),
                            ),
                            severity: Some(lsp::DiagnosticSeverity::ERROR),
                            message: lsp::DiagnosticMessage::MarkupContent(lsp::MarkupContent {
                                kind: lsp::MarkupKind::Markdown,
                                value: "`abc` is **not** defined".to_string(),
                            }),
                            ..Default::default()
                        },
                        lsp::Diagnostic {
                            range: lsp::Range::new(
                                lsp::Position::new(0, 17),
                                lsp::Position::new(0, 20),
                            ),
                            severity: Some(lsp::DiagnosticSeverity::ERROR),
                            message: lsp::DiagnosticMessage::from("`i32` is **not** a type"),
                            ..Default::default()
                        },
                        lsp::Diagnostic {
                            range: lsp::Range::new(
                                lsp::Position::new(0, 21),
                                lsp::Position::new(0, 24),
                            ),
                            severity: Some(lsp::DiagnosticSeverity::ERROR),
                            source: Some("rustc".to_string()),
                            code: Some(lsp::NumberOrString::String("E0425".to_string())),
                            message: lsp::DiagnosticMessage::from(lsp::MarkupContent {
                                kind: lsp::MarkupKind::Markdown,
                                value: "```rust\nlet x = unknown;\n```".to_string(),
                            }),
                            ..Default::default()
                        },
                        lsp::Diagnostic {
                            range: lsp::Range::new(
                                lsp::Position::new(0, 25),
                                lsp::Position::new(0, 28),
                            ),
                            severity: Some(lsp::DiagnosticSeverity::ERROR),
                            message: lsp::DiagnosticMessage::MarkupContent(lsp::MarkupContent {
                                kind: lsp::MarkupKind::PlainText,
                                value: "`u32` is **not** a type".to_string(),
                            }),
                            ..Default::default()
                        },
                    ],
                },
                None,
                DiagnosticSourceKind::Pushed,
                &[],
                cx,
            )
        })
    })
    .unwrap();
    cx.run_until_parked();

    cx.update_editor(|editor, window, cx| {
        editor::hover_popover::hover(editor, &Default::default(), window, cx)
    });
    cx.run_until_parked();
    cx.update_editor(|editor, _, _| assert!(editor.hover_state.diagnostic_popover.is_some()));

    let buffer_id = cx.update_buffer(|buffer, _| buffer.remote_id());
    cx.update_editor(|editor, window, cx| {
        let snapshot = editor.snapshot(window, cx);

        // Markdown MarkupContent is rendered as the markdown the server sent,
        // while PlainText MarkupContent and plain string messages are escaped.
        let group = snapshot
            .buffer_snapshot()
            .diagnostic_group(buffer_id, 0)
            .collect::<Vec<_>>();
        let blocks = diagnostic_renderer::DiagnosticRenderer::diagnostic_blocks_for_group(
            group, buffer_id, None, cx,
        );
        assert_eq!(
            blocks[0].markdown.read(cx).source(),
            "`abc` is **not** defined"
        );

        let group = snapshot
            .buffer_snapshot()
            .diagnostic_group(buffer_id, 1)
            .collect::<Vec<_>>();
        let blocks = diagnostic_renderer::DiagnosticRenderer::diagnostic_blocks_for_group(
            group, buffer_id, None, cx,
        );
        assert_eq!(
            blocks[0].markdown.read(cx).source(),
            markdown::Markdown::escape("`i32` is **not** a type").as_ref(),
        );

        let group = snapshot
            .buffer_snapshot()
            .diagnostic_group(buffer_id, 2)
            .collect::<Vec<_>>();
        let blocks = diagnostic_renderer::DiagnosticRenderer::diagnostic_blocks_for_group(
            group, buffer_id, None, cx,
        );
        assert_eq!(
            blocks[0].markdown.read(cx).source(),
            "```rust\nlet x = unknown;\n```\n\n(rustc E0425)"
        );

        let group = snapshot
            .buffer_snapshot()
            .diagnostic_group(buffer_id, 3)
            .collect::<Vec<_>>();
        let blocks = diagnostic_renderer::DiagnosticRenderer::diagnostic_blocks_for_group(
            group, buffer_id, None, cx,
        );
        assert_eq!(
            blocks[0].markdown.read(cx).source(),
            markdown::Markdown::escape("`u32` is **not** a type").as_ref(),
        );
    });
}

#[gpui::test]
async fn test_hover_diagnostic_and_info_popovers(cx: &mut gpui::TestAppContext) {
    init_test(cx);

    let mut cx = EditorLspTestContext::new_rust(
        lsp::ServerCapabilities {
            hover_provider: Some(lsp::HoverProviderCapability::Simple(true)),
            ..Default::default()
        },
        cx,
    )
    .await;

    // Hover with just diagnostic, pops DiagnosticPopover immediately and then
    // info popover once request completes
    cx.set_state(indoc! {"
        fn teˇst() { println!(); }
    "});
    // Send diagnostic to client
    let range = cx.lsp_range(indoc! {"
        fn «test»() { println!(); }
    "});
    let lsp_store =
        cx.update_editor(|editor, _, cx| editor.project().unwrap().read(cx).lsp_store());
    cx.update(|_, cx| {
        lsp_store.update(cx, |lsp_store, cx| {
            lsp_store.update_diagnostics(
                LanguageServerId(0),
                lsp::PublishDiagnosticsParams {
                    uri: lsp::Uri::from_file_path(path!("/root/dir/file.rs")).unwrap(),
                    version: None,
                    diagnostics: vec![lsp::Diagnostic {
                        range,
                        severity: Some(lsp::DiagnosticSeverity::ERROR),
                        message: lsp::DiagnosticMessage::from("A test diagnostic message."),
                        ..Default::default()
                    }],
                },
                None,
                DiagnosticSourceKind::Pushed,
                &[],
                cx,
            )
        })
    })
    .unwrap();
    cx.run_until_parked();

    // Hover pops diagnostic immediately
    cx.update_editor(|editor, window, cx| editor::hover_popover::hover(editor, &Hover, window, cx));
    cx.background_executor.run_until_parked();

    cx.editor(|Editor { hover_state, .. }, _, _| {
        assert!(hover_state.diagnostic_popover.is_some());
        assert!(hover_state.info_popovers.is_empty());
    });

    // Info Popover shows after request responded to
    let range = cx.lsp_range(indoc! {"
            fn «test»() { println!(); }
        "});
    cx.set_request_handler::<lsp::request::HoverRequest, _, _>(move |_, _, _| async move {
        Ok(Some(lsp::Hover {
            contents: lsp::HoverContents::Markup(lsp::MarkupContent {
                kind: lsp::MarkupKind::Markdown,
                value: "some new docs".to_string(),
            }),
            range: Some(range),
        }))
    });
    let delay = cx.update(|_, cx| EditorSettings::get_global(cx).hover_popover_delay.0 + 1);
    cx.background_executor
        .advance_clock(Duration::from_millis(delay));

    cx.background_executor.run_until_parked();
    cx.editor(|Editor { hover_state, .. }, _, _| {
        hover_state.diagnostic_popover.is_some() && hover_state.info_task.is_some()
    });
}

#[gpui::test]
async fn go_to_diagnostic_with_severity(cx: &mut TestAppContext) {
    init_test(cx);

    let mut cx = EditorTestContext::new(cx).await;
    let lsp_store =
        cx.update_editor(|editor, _, cx| editor.project().unwrap().read(cx).lsp_store());

    cx.set_state(indoc! {"error warning info hiˇnt"});

    cx.update(|_, cx| {
        lsp_store.update(cx, |lsp_store, cx| {
            lsp_store
                .update_diagnostics(
                    LanguageServerId(0),
                    lsp::PublishDiagnosticsParams {
                        uri: lsp::Uri::from_file_path(path!("/root/file")).unwrap(),
                        version: None,
                        diagnostics: vec![
                            lsp::Diagnostic {
                                range: lsp::Range::new(
                                    lsp::Position::new(0, 0),
                                    lsp::Position::new(0, 5),
                                ),
                                severity: Some(lsp::DiagnosticSeverity::ERROR),
                                ..Default::default()
                            },
                            lsp::Diagnostic {
                                range: lsp::Range::new(
                                    lsp::Position::new(0, 6),
                                    lsp::Position::new(0, 13),
                                ),
                                severity: Some(lsp::DiagnosticSeverity::WARNING),
                                ..Default::default()
                            },
                            lsp::Diagnostic {
                                range: lsp::Range::new(
                                    lsp::Position::new(0, 14),
                                    lsp::Position::new(0, 18),
                                ),
                                severity: Some(lsp::DiagnosticSeverity::INFORMATION),
                                ..Default::default()
                            },
                            lsp::Diagnostic {
                                range: lsp::Range::new(
                                    lsp::Position::new(0, 19),
                                    lsp::Position::new(0, 23),
                                ),
                                severity: Some(lsp::DiagnosticSeverity::HINT),
                                ..Default::default()
                            },
                        ],
                    },
                    None,
                    DiagnosticSourceKind::Pushed,
                    &[],
                    cx,
                )
                .unwrap()
        });
    });
    cx.run_until_parked();

    macro_rules! go {
        ($severity:expr) => {
            cx.update_editor(|editor, window, cx| {
                editor.go_to_diagnostic(
                    &GoToDiagnostic {
                        severity: $severity,
                    },
                    window,
                    cx,
                );
            });
        };
    }

    // Default, should cycle through all diagnostics
    go!(GoToDiagnosticSeverityFilter::default());
    cx.assert_editor_state(indoc! {"error warning info ˇhint"});
    go!(GoToDiagnosticSeverityFilter::default());
    cx.assert_editor_state(indoc! {"ˇerror warning info hint"});
    go!(GoToDiagnosticSeverityFilter::default());
    cx.assert_editor_state(indoc! {"error ˇwarning info hint"});
    go!(GoToDiagnosticSeverityFilter::default());
    cx.assert_editor_state(indoc! {"error warning ˇinfo hint"});
    go!(GoToDiagnosticSeverityFilter::default());
    cx.assert_editor_state(indoc! {"error warning info ˇhint"});
    go!(GoToDiagnosticSeverityFilter::default());
    cx.assert_editor_state(indoc! {"ˇerror warning info hint"});

    let only_info = GoToDiagnosticSeverityFilter::Only(GoToDiagnosticSeverity::Information);
    go!(only_info);
    cx.assert_editor_state(indoc! {"error warning ˇinfo hint"});
    go!(only_info);
    cx.assert_editor_state(indoc! {"error warning ˇinfo hint"});

    let no_hints = GoToDiagnosticSeverityFilter::Range {
        min: GoToDiagnosticSeverity::Information,
        max: GoToDiagnosticSeverity::Error,
    };

    go!(no_hints);
    cx.assert_editor_state(indoc! {"ˇerror warning info hint"});
    go!(no_hints);
    cx.assert_editor_state(indoc! {"error ˇwarning info hint"});
    go!(no_hints);
    cx.assert_editor_state(indoc! {"error warning ˇinfo hint"});
    go!(no_hints);
    cx.assert_editor_state(indoc! {"ˇerror warning info hint"});

    let warning_info = GoToDiagnosticSeverityFilter::Range {
        min: GoToDiagnosticSeverity::Information,
        max: GoToDiagnosticSeverity::Warning,
    };

    go!(warning_info);
    cx.assert_editor_state(indoc! {"error ˇwarning info hint"});
    go!(warning_info);
    cx.assert_editor_state(indoc! {"error warning ˇinfo hint"});
    go!(warning_info);
    cx.assert_editor_state(indoc! {"error ˇwarning info hint"});
}

fn init_test(cx: &mut TestAppContext) {
    cx.update(|cx| {
        zlog::init_test();
        let settings = SettingsStore::test(cx);
        cx.set_global(settings);
        theme_settings::init(theme::LoadThemes::JustBase, cx);
        crate::init(cx);
        editor::init(cx);
    });
}
