use super::*;
use crate::display_tests::{connected, ingest, started, stopped};

fn wait_file(app: &mut App) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while app.workspace.controller.is_busy() {
        app.workspace_poll(&egui::Context::default());
        assert!(Instant::now() < deadline, "workspace I/O timed out");
        std::thread::sleep(Duration::from_millis(5));
    }
    app.workspace_poll(&egui::Context::default());
}
#[test]
fn path_failure_correction_and_explicit_restore_preserve_current_intent() {
    let (mut app, commands) = connected();
    let path = std::env::temp_dir().join(format!(
        "studio-workspace-recovery-{}.json",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);
    app.workspace.name = "Unsaved before failure".into();
    app.workspace.path_entry = "relative.json".into();
    app.workspace_action("load_path");
    assert!(app.workspace_needs_attention());
    assert_eq!(app.workspace.name, "Unsaved before failure");
    app.workspace.path_entry = path.to_string_lossy().into_owned();
    app.workspace_action("load_path");
    wait_file(&mut app);
    assert!(app.workspace_needs_attention());
    assert!(app.workspace.controller.candidate().is_none());
    assert_eq!(app.workspace.name, "Unsaved before failure");
    assert!(app.workspace_dirty());
    app.workspace.path_entry = "relative.json".into();
    app.workspace_action("load_path");
    assert!(app
        .workspace_error()
        .unwrap()
        .contains("absolute workspace file path"));
    app.workspace.path_entry = path.to_string_lossy().into_owned();
    app.workspace_action("save_path");
    wait_file(&mut app);
    assert!(!app.workspace_needs_attention());
    assert!(!app.workspace_dirty());
    assert_eq!(app.workspace.controller.path(), Some(path.as_path()));
    app.workspace.path_entry = "relative.json".into();
    app.workspace_action("load_path");
    assert!(app.workspace_needs_attention());
    app.workspace_file(FileAction::Save).unwrap();
    wait_file(&mut app);
    assert!(!app.workspace_needs_attention());
    app.workspace.path_entry = path.to_string_lossy().into_owned();
    app.workspace.name = "Current unsaved draft".into();
    app.workspace_action("load_path");
    wait_file(&mut app);
    assert_eq!(app.workspace.name, "Current unsaved draft");
    assert!(app.workspace_dirty());
    assert!(app
        .workspace
        .controller
        .candidate()
        .unwrap()
        .preview
        .is_some());
    app.workspace_action("cancel");
    assert_eq!(app.workspace.name, "Current unsaved draft");
    app.workspace_action("review");
    app.workspace_action("apply");
    app.workspace_poll(&egui::Context::default());
    assert_eq!(app.workspace.name, "Unsaved before failure");
    assert!(!app.workspace_dirty());
    assert!(app.workspace.restored_notice);
    assert!(commands.try_recv().is_err());
    std::fs::remove_file(path).unwrap();
}
#[test]
fn help_and_recovery_navigation_are_passive_and_cancel_never_restarts_stopped_display() {
    let (mut app, commands) = connected();
    app.show_topic(Output::SpatialScan, "/scan", spatial::SCAN_MESSAGE_TYPE);
    let start = commands.try_recv().unwrap();
    ingest(&mut app, started(&start));
    app.scene_settings.draft.fixed_frame = "preserved draft".into();
    app.workspace.path_entry = "/draft/path.json".into();
    for destination in [
        crate::help::Destination::Workspace,
        crate::help::Destination::Scene,
        crate::help::Destination::Diagnostics,
    ] {
        app.show_help(crate::help::Topic::Workspaces);
        app.help_navigate(destination);
        assert!(!app.help.open);
        assert!(commands.try_recv().is_err());
    }
    assert_eq!(app.scene_settings.draft.fixed_frame, "preserved draft");
    assert_eq!(app.workspace.path_entry, "/draft/path.json");
    let snapshot = app.workspace_snapshot().unwrap();
    app.workspace_review_configuration(snapshot).unwrap();
    assert!(commands.try_recv().is_err());
    app.workspace_action("apply");
    let stop = commands.try_recv().unwrap();
    app.workspace_action("cancel");
    ingest(&mut app, stopped(&stop));
    app.workspace_poll(&egui::Context::default());
    assert!(!app.views[0].enabled);
    assert!(app.views[0].subscription.is_none());
    assert!(!app.workspace_frozen());
    assert!(commands.try_recv().is_err());
}
#[test]
fn workspace_toolbar_geometry_does_not_depend_on_long_errors_paths_or_dirty_state() {
    for theme in [egui::Theme::Light, egui::Theme::Dark] {
        for width in [360.0, 640.0] {
            let (mut app, commands) = connected();
            let ctx = egui::Context::default();
            fonts::configure(&ctx);
            shell::configure(&ctx);
            ctx.enable_accesskit();
            ctx.set_theme(theme);
            let mut reference = None;
            app.workspace.focus_path = true;
            for stage in 0..3 {
                if stage == 1 {
                    app.workspace.recovery_error = Some("long failed destination ".repeat(90));
                    app.workspace.path_entry = format!("/{}", "long folder/".repeat(100));
                    app.workspace.name = "long name ".repeat(20);
                }
                if stage == 2 {
                    app.workspace_action("visualize");
                }
                for _ in 0..3 {
                    let output = ctx.run(
                        egui::RawInput {
                            screen_rect: Some(egui::Rect::from_min_size(
                                egui::Pos2::ZERO,
                                egui::vec2(width, 600.0),
                            )),
                            ..Default::default()
                        },
                        |ctx| {
                            egui::CentralPanel::default().show(ctx, |ui| app.workspace_render(ui));
                        },
                    );
                    assert_eq!(
                        ctx.memory(|m| m.focused()),
                        Some(egui::Id::new("workspace-path")),
                        "path focus lost at stage {stage}"
                    );
                    let tree = output.platform_output.accesskit_update.unwrap();
                    let bounds: Vec<_> = [
                        "Open…",
                        "Save",
                        "Save as…",
                        "Cancel operation",
                        "Retry failed stops",
                        "Workspace help",
                    ]
                    .into_iter()
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
                        assert_eq!(&bounds, expected, "stage {stage}, width {width}");
                    } else {
                        reference = Some(bounds);
                    }
                }
            }
            assert!(commands.try_recv().is_err());
        }
    }
}
