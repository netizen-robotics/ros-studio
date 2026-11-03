use super::*;

#[test]
#[allow(clippy::field_reassign_with_default)] // App implements Drop; struct update cannot move its fields.
fn mapping_preserves_custom_file_ids_and_allocates_fresh_runtime_ids_without_verification() {
    let mut app = App::default();
    app.next_view = 41;
    let mut saved = file::Preset::Visualize.workspace();
    saved.views[0].id = "my_custom_room".into();
    saved.views[1].id = "camera-left".into();
    let file::ViewKind::Spatial { layers, .. } = &mut saved.views[0].kind else {
        panic!()
    };
    layers[0].id = "front_scan".into();
    saved.layout = file::Layout::for_views(&saved.views);
    saved.layout.reorder("camera-left", 0).unwrap();
    let file::ViewKind::Image { display_mode } = &mut saved.views[1].kind else {
        panic!()
    };
    *display_mode = file::ImageMode::ActualSize;
    let mapped = app.workspace_prepare(&saved).unwrap();
    assert_eq!(mapped.views[0].id, ViewId(42));
    assert_eq!(mapped.views[1].id, ViewId(43));
    assert_eq!(mapped.views[0].workspace_id, "my_custom_room");
    assert_eq!(mapped.views[1].workspace_id, "camera-left");
    assert_eq!(mapped.views[0].workspace_scan_layer_id, "front_scan");
    assert!(!mapped.views[0].spatial_clock_verified);
    assert_eq!(
        mapped.views[0]
            .configuration
            .spatial
            .as_ref()
            .unwrap()
            .clock_policy,
        studio_protocol::spatial_input::SpatialClockPolicy::Unknown
    );
    assert_eq!(
        mapped.views[1].image.display_mode(),
        images::DisplayMode::ActualSize
    );
    assert_eq!(app.next_view, 41);
    assert!(app.views.is_empty());
    app.workspace_shutdown();
}

#[test]
fn unsupported_or_exhausted_mapping_preserves_current_configuration() {
    let mut app = App::default();
    let before = app.workspace_snapshot().unwrap();
    app.next_view = u64::MAX;
    assert!(app
        .workspace_prepare(&file::Preset::Inspect.workspace())
        .is_err());
    assert_eq!(app.workspace_snapshot().unwrap(), before);
    let mut draft = file::Preset::Visualize.workspace();
    let file::ViewKind::Spatial { transforms, .. } = &mut draft.views[0].kind else {
        panic!()
    };
    *transforms = None;
    assert!(app.workspace_prepare(&draft).is_err());
    assert!(app.views.is_empty());
    assert_eq!(app.next_view, u64::MAX);
    app.workspace_shutdown();
}

#[test]
fn preset_requires_explicit_apply_and_restores_only_inactive_views_without_session() {
    let mut app = App::default();
    app.workspace
        .controller
        .preview_configuration(file::Preset::Visualize.workspace())
        .unwrap();
    app.workspace_poll(&egui::Context::default());
    assert!(app.views.is_empty());
    app.workspace_begin_apply(file::ApplyApproval::DiscardUnsaved)
        .unwrap();
    app.workspace_poll(&egui::Context::default());
    assert_eq!(app.views.len(), 2);
    assert!(app.session.is_none());
    assert!(app.views.iter().all(|view| view.subscription.is_none()));
    assert!(!app.views[0].spatial_clock_verified);
    assert!(app.workspace.controller.path().is_none());
    assert!(app.workspace.controller.dirty());
    let snapshot = app.workspace_snapshot().unwrap();
    assert_eq!(snapshot, file::Preset::Visualize.workspace());
    app.workspace_shutdown();
}

#[test]
fn newly_created_panel_cannot_collide_with_a_custom_loaded_generated_looking_id() {
    let mut app = App::default();
    let configuration = model::configuration(&file::Preset::Inspect.workspace().views[0]).unwrap();
    let mut loaded = ViewState::new(ViewId(1), configuration.clone());
    loaded.workspace_id = "panel_2".into();
    app.views.push(loaded);
    app.views.push(ViewState::new(ViewId(2), configuration));
    app.workspace_adopt_new_view(ViewId(2));
    assert_eq!(app.views[0].workspace_id, "panel_2");
    assert_eq!(app.views[1].workspace_id, "panel_2_1");
    app.workspace_snapshot().unwrap();
    app.workspace_shutdown();
}

#[test]
fn loading_reordered_file_stays_clean_after_apply_and_next_poll() {
    struct Remove(std::path::PathBuf);
    impl Drop for Remove {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }
    let path = Remove(std::env::temp_dir().join(format!(
        "studio-workspace-order-{}.json",
        std::process::id()
    )));
    let mut saved = file::Preset::Analyze.workspace();
    saved.layout.reorder("messages", 0).unwrap();
    file::save_atomic(&path.0, &saved).unwrap();
    let mut app = App::default();
    app.workspace.controller.load_path(path.0.clone()).unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while app.workspace.controller.is_busy() {
        app.workspace_poll(&egui::Context::default());
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(
        app.workspace.controller.candidate().unwrap().workspace,
        saved
    );
    app.workspace_begin_apply(file::ApplyApproval::DiscardUnsaved)
        .unwrap();
    app.workspace_poll(&egui::Context::default());
    app.workspace_poll(&egui::Context::default());
    assert!(
        !app.workspace.controller.dirty(),
        "Restoring saved layout order must not create an unsaved document edit"
    );
    assert_eq!(app.workspace_snapshot().unwrap(), saved);
    let displayed = app
        .workspace_ordered_ids()
        .into_iter()
        .map(|id| {
            app.views
                .iter()
                .find(|view| view.id == id)
                .unwrap()
                .workspace_id
                .as_str()
        })
        .collect::<Vec<_>>();
    assert_eq!(displayed, ["messages", "plot"]);
    assert!(app.views.iter().all(|view| view.subscription.is_none()));
    app.workspace_shutdown();
}

fn path_ui_frame(
    context: &egui::Context,
    saved: &std::path::Path,
    preview: Option<&std::path::Path>,
    target: Option<egui::accesskit::NodeId>,
) -> egui::FullOutput {
    context.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(900.0, 800.0),
            )),
            events: target
                .into_iter()
                .map(|target| {
                    egui::Event::AccessKitActionRequest(egui::accesskit::ActionRequest {
                        action: egui::accesskit::Action::Click,
                        target,
                        data: None,
                    })
                })
                .collect(),
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                workspace_path_ui(ui, saved, "Saved workspace");
                if let Some(preview) = preview {
                    workspace_path_ui(ui, preview, "Preview");
                }
            });
        },
    )
}
fn path_control(output: &egui::FullOutput, label: &str) -> egui::accesskit::NodeId {
    output
        .platform_output
        .accesskit_update
        .as_ref()
        .unwrap()
        .nodes
        .iter()
        .find(|(_, node)| node.label() == Some(label))
        .unwrap_or_else(|| panic!("Missing path control: {label}"))
        .0
}
fn path_has_text(output: &egui::FullOutput, expected: &str) -> bool {
    fn matches(shape: &egui::Shape, expected: &str) -> bool {
        match shape {
            egui::Shape::Text(text) => text.galley.text().contains(expected),
            egui::Shape::Vec(children) => children.iter().any(|child| matches(child, expected)),
            _ => false,
        }
    }
    output
        .shapes
        .iter()
        .any(|shape| matches(&shape.shape, expected))
}
#[test]
fn path_disclosures_are_independent_and_copy_exact_unicode() {
    let context = egui::Context::default();
    context.enable_accesskit();
    context.all_styles_mut(|style| style.animation_time = 0.0);
    let saved = std::path::Path::new("/tmp/工作空间/café 🤖.json");
    let preview = std::path::Path::new("/tmp/预览/other café.json");
    let output = path_ui_frame(&context, saved, Some(preview), None);
    let saved_toggle = path_control(&output, "Saved workspace full path");
    let preview_toggle = path_control(&output, "Preview full path");
    assert_ne!(saved_toggle, preview_toggle);
    let output = path_ui_frame(&context, saved, Some(preview), Some(saved_toggle));
    let saved_copy = path_control(&output, "Copy Saved workspace path");
    assert!(path_has_text(&output, saved.to_str().unwrap()));
    assert!(!path_has_text(&output, preview.to_str().unwrap()));
    let output = path_ui_frame(&context, saved, Some(preview), Some(preview_toggle));
    let preview_copy = path_control(&output, "Copy Preview path");
    assert!(path_has_text(&output, saved.to_str().unwrap()));
    assert!(path_has_text(&output, preview.to_str().unwrap()));
    for (button, exact) in [
        (saved_copy, saved.to_str().unwrap()),
        (preview_copy, preview.to_str().unwrap()),
    ] {
        let output = path_ui_frame(&context, saved, Some(preview), Some(button));
        assert!(output
            .platform_output
            .commands
            .iter()
            .any(|command| matches!(command,
                egui::OutputCommand::CopyText(text) if text == exact
            )));
    }
    let output = path_ui_frame(&context, saved, Some(preview), Some(saved_toggle));
    assert!(!path_has_text(&output, saved.to_str().unwrap()));
    assert!(path_has_text(&output, preview.to_str().unwrap()));
}
#[cfg(unix)]
#[test]
fn non_unicode_path_explains_and_disables_exact_copy_without_mutating_bytes() {
    use std::os::unix::ffi::{OsStrExt, OsStringExt};
    let original = b"/tmp/native-\xff.json".to_vec();
    let path = std::path::PathBuf::from(std::ffi::OsString::from_vec(original.clone()));
    let context = egui::Context::default();
    context.enable_accesskit();
    context.all_styles_mut(|style| style.animation_time = 0.0);
    let output = path_ui_frame(&context, &path, None, None);
    let toggle = path_control(&output, "Saved workspace full path");
    let output = path_ui_frame(&context, &path, None, Some(toggle));
    let copy = path_control(&output, "Copy Saved workspace path");
    assert!(output
        .platform_output
        .accesskit_update
        .as_ref()
        .unwrap()
        .nodes
        .iter()
        .find(|(id, _)| *id == copy)
        .unwrap()
        .1
        .is_disabled());
    assert!(path_has_text(&output, "non-Unicode bytes"));
    assert!(path_has_text(
        &output,
        "File operations preserve the original bytes"
    ));
    let output = path_ui_frame(&context, &path, None, Some(copy));
    assert!(!output
        .platform_output
        .commands
        .iter()
        .any(|command| matches!(command, egui::OutputCommand::CopyText(_))));
    assert_eq!(path.as_os_str().as_bytes(), original);
}
