use super::*;
use crate::components::workspace::PackageRoot;

fn absolute_fixture(relative: &str) -> String {
    std::env::temp_dir()
        .join(relative)
        .to_string_lossy()
        .into_owned()
}

#[test]
fn closed_mesh_editor_error_remains_in_current_attention_after_history_clear() {
    let mut app = App::default();
    app.markers.asset_error = Some("Mesh root approval failed".into());
    app.markers.asset_editor_open = false;
    app.diagnostics.clear();
    assert!(app
        .current_attention()
        .iter()
        .any(|item| item.message.contains("Mesh root approval failed")));
    app.markers.reveal_assets();
    assert!(app.markers.asset_editor_open);
}

fn mapping(name: &str, path: &str) -> PackageRoot {
    PackageRoot {
        package: name.into(),
        path: path.into(),
    }
}
#[test]
fn structured_mapping_preserves_paths_and_rejects_incomplete_duplicate_and_over_budget_rows() {
    let draft = AssetDraft {
        root: String::new(),
        packages: vec![mapping(
            " fixture_robot ",
            &absolute_fixture("local/my robot=demo"),
        )],
    };
    assert_eq!(
        draft.policy(7).unwrap(),
        worker::AssetPolicy::new(
            7,
            None,
            BTreeMap::from([(
                "fixture_robot".into(),
                PathBuf::from(absolute_fixture("local/my robot=demo"))
            )])
        )
        .unwrap()
    );
    let mut invalid = draft.clone();
    invalid
        .packages
        .push(mapping("fixture_robot", &absolute_fixture("other")));
    assert!(invalid.policy(1).unwrap_err().contains("repeats"));
    for name in ["", "bad/name", "bad name", "bad=name"] {
        let mut invalid = draft.clone();
        invalid.packages[0].package = name.into();
        assert!(invalid.policy(1).unwrap_err().contains("name"));
    }
    for path in ["", "relative/path"] {
        let mut invalid = draft.clone();
        invalid.packages[0].path = path.into();
        assert!(invalid.policy(1).unwrap_err().contains("absolute"));
    }
    let invalid = AssetDraft {
        root: "relative".into(),
        packages: Vec::new(),
    };
    assert!(invalid.policy(1).is_err());
    assert!(AssetDraft::default().policy(1).is_err());
    let mut budget = AssetDraft::default();
    for _ in 0..MAX_PACKAGE_ROOTS {
        assert!(budget.add_package());
    }
    assert!(!budget.add_package());
    budget.packages.push(mapping("extra", "/extra"));
    assert!(budget.policy(1).unwrap_err().contains("16"));
    let mut invalid = draft.clone();
    invalid.packages[0].path = absolute_fixture(&"a".repeat(MAX_PATH_BYTES));
    assert!(invalid.policy(1).unwrap_err().contains("limit"));
    let mut invalid = draft;
    invalid.packages[0].package = "a".repeat(MAX_NAME_BYTES + 1);
    assert!(invalid.policy(1).is_err());
}
#[test]
fn draft_approval_failure_reload_discard_and_revoke_preserve_authority_boundaries() {
    let mut runtime = MarkerRuntime::default();
    runtime
        .draft
        .packages
        .push(mapping("robot", &absolute_fixture("approved/robot")));
    runtime.review(runtime.draft.clone()).unwrap();
    let approved = runtime.approved.clone().unwrap();
    let original = runtime.assets.clone().unwrap();
    runtime.draft.packages[0].path = absolute_fixture("draft only/robot=v2");
    let edited = runtime.draft.clone();
    assert!(runtime.asset_draft_changed());
    assert!(Arc::ptr_eq(runtime.assets.as_ref().unwrap(), &original));
    runtime
        .draft
        .packages
        .push(mapping("robot", &absolute_fixture("duplicate")));
    assert!(runtime.review(runtime.draft.clone()).is_err());
    assert!(Arc::ptr_eq(runtime.assets.as_ref().unwrap(), &original));
    assert_eq!(runtime.approved.as_ref(), Some(&approved));
    runtime.draft = edited;
    runtime.reload_assets().unwrap();
    assert_eq!(
        runtime.draft.packages[0].path,
        absolute_fixture("draft only/robot=v2")
    );
    assert_eq!(runtime.approved.as_ref(), Some(&approved));
    assert!(runtime.assets.as_ref().unwrap().epoch() > original.epoch());
    assert_eq!(
        runtime.assets.as_deref(),
        Some(&approved.policy(runtime.asset_epoch).unwrap())
    );
    runtime.discard_asset_draft();
    assert_eq!(runtime.draft, approved);
    assert!(!runtime.asset_draft_changed());
    runtime.revoke_assets();
    assert!(runtime.assets.is_none() && runtime.approved.is_none());
    assert_eq!(runtime.draft, approved);
    runtime.discard_asset_draft();
    assert_eq!(runtime.draft, AssetDraft::default());
    assert!(runtime.reload_assets().is_err());
}
#[test]
fn asset_fields_and_actions_stay_fixed_across_content_and_approval_without_commands() {
    for width in [180.0, 300.0, 360.0] {
        for theme in [egui::Theme::Light, egui::Theme::Dark] {
            let (mut app, commands) = crate::display_tests::connected();
            let ctx = egui::Context::default();
            fonts::configure(&ctx);
            shell::configure(&ctx);
            ctx.enable_accesskit();
            ctx.set_theme(theme);
            let mut reference = None;
            for stage in 0..5 {
                app.markers.draft.root = match stage {
                    0 => "".into(),
                    1 => "/long path=ok/".repeat(100),
                    2 => "relative".into(),
                    _ => absolute_fixture("approved/local"),
                };
                if stage == 3 {
                    app.markers.review(app.markers.draft.clone()).unwrap();
                }
                if stage == 4 {
                    app.markers.revoke_assets();
                }
                for _ in 0..3 {
                    let output = ctx.run(
                        egui::RawInput {
                            screen_rect: Some(egui::Rect::from_min_size(
                                egui::Pos2::ZERO,
                                egui::vec2(800., 600.),
                            )),
                            ..Default::default()
                        },
                        |ctx| {
                            egui::SidePanel::left("test-assets")
                                .exact_width(width)
                                .show(ctx, |ui| app.marker_asset_form_ui(ui, false));
                        },
                    );
                    let tree = output.platform_output.accesskit_update.unwrap();
                    let labels = [
                        "Approve roots",
                        "Discard draft",
                        "Reload meshes",
                        "Revoke access",
                        "Add package",
                    ];
                    let bounds: Vec<_> = labels
                        .iter()
                        .map(|label| {
                            tree.nodes
                                .iter()
                                .map(|(_, n)| n)
                                .find(|n| n.label() == Some(label))
                                .unwrap()
                                .bounds()
                                .unwrap()
                        })
                        .collect();
                    if let Some(expected) = &reference {
                        assert_eq!(&bounds, expected, "{theme:?} width{width} stage{stage}");
                    } else {
                        reference = Some(bounds);
                    }
                }
            }
            assert!(commands.try_recv().is_err());
        }
    }
}
