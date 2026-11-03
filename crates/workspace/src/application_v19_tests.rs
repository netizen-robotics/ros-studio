use super::*;
fn images(count: usize) -> Workspace {
    let mut workspace = Preset::Visualize.workspace();
    let image = workspace
        .views
        .iter()
        .find(|v| matches!(v.kind, ViewKind::Image { .. }))
        .unwrap()
        .clone();
    workspace.views = (0..count)
        .map(|index| ViewConfig {
            id: format!("image{index}"),
            topic: format!("/images/image{index}"),
            ..image.clone()
        })
        .collect();
    workspace.layout = Layout::for_views(&workspace.views);
    workspace.presentation = Some(VisualizerPresentation::default());
    workspace
}
#[test]
fn capacity32_roundtrips_layout_shell_and_inactive_restore_but_rejects33() {
    let mut workspace = images(32);
    workspace
        .presentation
        .as_mut()
        .unwrap()
        .shell
        .expanded_displays = workspace.views.iter().map(|v| v.id.clone()).collect();
    workspace
        .presentation
        .as_mut()
        .unwrap()
        .shell
        .selected_companion = Some("image31".into());
    let bytes = encode(&workspace, Application::Visualizer).unwrap();
    let wire: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(wire["version"], 19);
    if let Ok(path) = std::env::var("STUDIO_CAPACITY32_EXPORT") {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap();
        let path = std::path::PathBuf::from(path);
        assert!(path.starts_with(root.join("artifacts/feasibility/evidence")));
        std::fs::write(path, &bytes).unwrap();
    }
    let imported = parse(&bytes, Application::Visualizer).unwrap();
    assert!(!imported.legacy);
    assert_eq!(imported.workspace, workspace);
    assert!(imported.workspace.inactive_restore().is_ok());
    assert!(encode(&images(33), Application::Visualizer).is_err());
    assert!(parse(&bytes, Application::Tools).is_err());
    let mut old = wire;
    old["version"] = 18.into();
    assert!(parse(&serde_json::to_vec(&old).unwrap(), Application::Visualizer).is_err());
}
#[test]
fn frozen18_and_tools8_keep_eight_view_limits_and_preserve_import_bytes() {
    let bytes = encode(&images(8), Application::Visualizer).unwrap();
    let mut old: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    old["version"] = 18.into();
    let original = serde_json::to_vec(&old).unwrap();
    let imported = parse(&original, Application::Visualizer).unwrap();
    assert!(imported.legacy);
    assert_eq!(imported.workspace.views.len(), 8);
    assert_eq!(serde_json::to_vec(&old).unwrap(), original);
    let mut tools = images(32);
    tools.presentation = None;
    assert!(encode(&tools, Application::Tools).is_err());
    assert!(tools.encode().is_err());
    let mut oversized = old.clone();
    let nine = images(9);
    let nine_bytes = encode(&nine, Application::Visualizer).unwrap();
    let mut wire: serde_json::Value = serde_json::from_slice(&nine_bytes).unwrap();
    for version in [18, 17, 16, 15, 14, 13, 12, 11, 10, 9, 8] {
        wire["version"] = version.into();
        assert!(parse(&serde_json::to_vec(&wire).unwrap(), Application::Visualizer).is_err());
    }
    oversized["shell"]["expanded_displays"] = serde_json::json!(vec!["image0"; 9]);
    assert!(parse(
        &serde_json::to_vec(&oversized).unwrap(),
        Application::Visualizer
    )
    .is_err());
}
