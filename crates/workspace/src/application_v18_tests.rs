use super::*;
#[test]
fn shell_roundtrip_rejects_stale_ids_and_frozen_or_wrong_app_documents() {
    let mut w = parse(
        &Preset::Visualize.workspace().encode().unwrap(),
        Application::Visualizer,
    )
    .unwrap()
    .workspace;
    let id = w
        .views
        .iter()
        .find(|v| matches!(v.kind, ViewKind::Image { .. }))
        .unwrap()
        .id
        .clone();
    let p = w.presentation.as_mut().unwrap();
    p.shell = crate::visualizer_shell::Layout {
        topics_open: true,
        global_options_open: true,
        companions_open: true,
        companion_percent: 30,
        selected_companion: Some(id.clone()),
        expanded_displays: vec![id.clone()],
    };
    let bytes = encode(&w, Application::Visualizer).unwrap();
    let parsed = parse(&bytes, Application::Visualizer).unwrap();
    assert!(!parsed.legacy);
    assert_eq!(parsed.workspace, w);
    assert!(parse(&bytes, Application::Tools).is_err());
    let wire: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(wire["version"], VISUALIZER_VERSION);
    let mut old = wire.clone();
    old["version"] = 17.into();
    assert!(parse(&serde_json::to_vec(&old).unwrap(), Application::Visualizer).is_err());
    old.as_object_mut().unwrap().remove("shell");
    let old_bytes = serde_json::to_vec(&old).unwrap();
    let imported = parse(&old_bytes, Application::Visualizer).unwrap();
    assert!(imported.legacy);
    assert_eq!(
        imported.workspace.presentation.unwrap().shell,
        Default::default()
    );
    assert_eq!(serde_json::to_vec(&old).unwrap(), old_bytes);
    for shell in [
        serde_json::json!({"selected_companion":"gone"}),
        serde_json::json!({"expanded_displays":[id.clone(),id]}),
        serde_json::json!({"companion_percent":41}),
        serde_json::json!({"companion_percent":14}),
        serde_json::json!({"enabled":true}),
    ] {
        let mut bad = wire.clone();
        for (key, value) in shell.as_object().unwrap() {
            bad["shell"][key] = value.clone();
        }
        assert!(parse(&serde_json::to_vec(&bad).unwrap(), Application::Visualizer).is_err());
    }
}
