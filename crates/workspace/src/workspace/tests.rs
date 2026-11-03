use super::*;
use std::path::PathBuf;
fn fixture() -> Workspace {
    Workspace {
        tools: None,
        presentation: None,
        version: FORMAT_VERSION,
        name: "Inspection".into(),
        panels: Panels {
            data_open: true,
            inspector_open: false,
            diagnostics_open: false,
        },
        views: vec![ViewConfig {
            id: "messages-1".into(),
            kind: ViewKind::Messages,
            topic: "/demo/status".into(),
            message_type: "std_msgs/msg/String".into(),
            qos: Qos {
                reliability: Reliability::BestEffort,
                durability: Durability::TransientLocal,
            },
        }],
        layout: Layout {
            order: vec!["messages-1".into()],
            scene_percent: 65,
        },
        connection_suggestion: Some(ConnectionSuggestion {
            domain: 187,
            middleware: None,
        }),
    }
}
struct TestDirectory(PathBuf);
impl TestDirectory {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!(
            "studio-workspace-test-{}-{}",
            std::process::id(),
            super::storage_test_id()
        ));
        std::fs::create_dir(&p).unwrap();
        Self(p)
    }
}
impl Drop for TestDirectory {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}
#[test]
fn round_trip_is_inactive_configuration_only() {
    let original = fixture();
    let encoded = original.encode().unwrap();
    let restore = Workspace::parse(&encoded)
        .unwrap()
        .inactive_restore()
        .unwrap();
    assert_eq!(restore.configuration(), &original);
    let text = String::from_utf8(encoded).unwrap();
    for prohibited in [
        "active",
        "session",
        "request",
        "samples",
        "backend",
        "executable",
        "environment",
        "middleware",
    ] {
        assert!(!text.contains(prohibited));
    }
    assert!(restore.configuration().panels.data_open);
}
#[test]
fn rejects_execution_activation_and_unknown_fields_everywhere() {
    let base = serde_json::to_value(fixture()).unwrap();
    for field in [
        "active",
        "session_id",
        "request_id",
        "backend_path",
        "command",
        "samples",
        "environment",
        "library_path",
    ] {
        for level in 0..3 {
            let mut value = base.clone();
            let target = match level {
                0 => value.as_object_mut().unwrap(),
                1 => value["views"][0].as_object_mut().unwrap(),
                _ => value["connection_suggestion"].as_object_mut().unwrap(),
            };
            target.insert(field.into(), true.into());
            assert!(
                Workspace::parse(&serde_json::to_vec(&value).unwrap()).is_err(),
                "{field}, level {level}"
            );
        }
    }
}
#[test]
fn rejects_unsupported_kind_version_duplicate_keys_and_corruption() {
    let base = String::from_utf8(fixture().encode().unwrap()).unwrap();
    for input in [
        base.replace("\"messages\"", "\"image\""),
        base.replace("\"version\": 7", "\"version\": 99"),
        base.replace("\"version\": 7", "\"version\": 7, \"version\": 7"),
        "{garbage}".into(),
        format!("{base} trailing"),
    ] {
        assert!(Workspace::parse(input.as_bytes()).is_err());
    }
}
#[test]
fn bounds_bytes_depth_and_counts() {
    assert!(Workspace::parse(&vec![b' '; MAX_FILE_BYTES + 1]).is_err());
    let deep = format!(
        "{}0{}",
        "[".repeat(MAX_JSON_DEPTH + 1),
        "]".repeat(MAX_JSON_DEPTH + 1)
    );
    assert!(Workspace::parse(deep.as_bytes())
        .unwrap_err()
        .to_string()
        .contains("nesting"));
    let mut config = fixture();
    config.views.push(config.views[0].clone());
    assert!(config.validate().is_err());
    config.views.clear();
    config.layout = Layout::for_views(&config.views);
    assert!(config.validate().is_ok());
    config.name = "Quoted [{ brackets \\".into();
    assert_eq!(Workspace::parse(&config.encode().unwrap()).unwrap(), config);
}
#[test]
fn validates_identifiers_names_types_and_domain() {
    for topic in ["", "/", "relative", "/a//b", "/9bad", "/a/", "/a b", "/a\n"] {
        let mut config = fixture();
        config.views[0].topic = topic.into();
        assert!(config.validate().is_err(), "{topic:?}");
    }
    for ty in [
        "",
        "std_msgs/String",
        "std_msgs/srv/String",
        "../msg/Foo",
        "std_msgs/msg/9Bad",
        "std_msgs/msg/Foo/Bar",
    ] {
        let mut config = fixture();
        config.views[0].message_type = ty.into();
        assert!(config.validate().is_err(), "{ty}");
    }
    for id in ["", "../escape", "space id", "😺"] {
        let mut config = fixture();
        config.views[0].id = id.into();
        assert!(config.validate().is_err());
    }
    let mut config = fixture();
    config.connection_suggestion.as_mut().unwrap().domain = 233;
    assert!(config.validate().is_err());
    config.connection_suggestion.as_mut().unwrap().domain = 232;
    assert!(config.validate().is_ok());
    config.name = "x".repeat(129);
    assert!(config.validate().is_err());
}
#[test]
fn save_load_and_replacement_preserve_configuration() {
    let dir = TestDirectory::new();
    let file = dir.0.join("workspace.json");
    save_atomic(&file, &fixture()).unwrap();
    assert_eq!(load(&file).unwrap(), fixture());
    let mut next = fixture();
    next.name = "Changed".into();
    save_atomic(&file, &next).unwrap();
    assert_eq!(load(&file).unwrap(), next);
    assert_eq!(std::fs::read_dir(&dir.0).unwrap().count(), 1);
}
#[test]
fn interrupted_write_preserves_previous_bytes_and_cleans_temp() {
    let dir = TestDirectory::new();
    let file = dir.0.join("workspace.json");
    save_atomic(&file, &fixture()).unwrap();
    let original = std::fs::read(&file).unwrap();
    let mut next = fixture();
    next.name = "Changed".into();
    assert!(storage::simulate_interrupted_save(&file, &next).is_err());
    assert_eq!(std::fs::read(&file).unwrap(), original);
    assert_eq!(std::fs::read_dir(&dir.0).unwrap().count(), 1);
}
#[test]
fn save_validation_and_rename_failure_are_non_destructive() {
    let dir = TestDirectory::new();
    let file = dir.0.join("workspace.json");
    save_atomic(&file, &fixture()).unwrap();
    let original = std::fs::read(&file).unwrap();
    let mut invalid = fixture();
    invalid.version = 99;
    assert!(save_atomic(&file, &invalid).is_err());
    assert_eq!(std::fs::read(&file).unwrap(), original);
    let directory = dir.0.join("existing-directory");
    std::fs::create_dir(&directory).unwrap();
    let marker = directory.join("keep");
    std::fs::write(&marker, b"keep").unwrap();
    assert!(save_atomic(&directory, &fixture()).is_err());
    assert_eq!(std::fs::read(marker).unwrap(), b"keep");
    assert_eq!(std::fs::read_dir(&dir.0).unwrap().count(), 2);
    assert!(save_atomic(&dir.0.join("missing/file.json"), &fixture()).is_err());
}
#[test]
fn bounded_load_and_failed_open_leave_document_unchanged() {
    let dir = TestDirectory::new();
    let file = dir.0.join("oversized.json");
    std::fs::write(&file, vec![b' '; MAX_FILE_BYTES + 1]).unwrap();
    let mut doc = Document::new(fixture()).unwrap();
    doc.apply_inactive(fixture().inactive_restore().unwrap());
    assert!(!doc.is_dirty());
    assert!(load(&file).is_err());
    assert_eq!(doc.current(), &fixture());
    assert!(!doc.is_dirty());
    assert!(load(&dir.0.join("missing")).is_err());
}
#[test]
fn dirty_state_changes_only_after_successful_edits_and_saves() {
    let dir = TestDirectory::new();
    let file = dir.0.join("workspace.json");
    let mut doc = Document::new(fixture()).unwrap();
    assert!(doc.is_dirty());
    doc.save(&file).unwrap();
    assert!(!doc.is_dirty());
    let mut next = fixture();
    next.name = "Changed".into();
    doc.edit(next.clone()).unwrap();
    assert!(doc.is_dirty());
    assert!(doc.save(&dir.0.join("missing/file")).is_err());
    assert!(doc.is_dirty());
    assert_eq!(doc.current(), &next);
    let mut invalid = next.clone();
    invalid.version = 8;
    assert!(doc.edit(invalid).is_err());
    assert_eq!(doc.current(), &next);
    doc.edit(fixture()).unwrap();
    assert!(!doc.is_dirty());
}

#[cfg(unix)]
#[test]
fn rejects_nonregular_workspace_sources_without_blocking() {
    const CHILD: &str = "STUDIO_TEST_WORKSPACE_FIFO";
    if let Some(path) = std::env::var_os(CHILD) {
        assert!(load(std::path::Path::new(&path)).is_err());
        return;
    }
    let dir = TestDirectory::new();
    let fifo = dir.0.join("source.fifo");
    assert!(std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .unwrap()
        .success());
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "workspace::tests::rejects_nonregular_workspace_sources_without_blocking",
        ])
        .env(CHILD, &fifo)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success());
            break;
        }
        if std::time::Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("workspace FIFO load blocked waiting for a writer");
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

#[test]
fn v1_migrates_deterministically_without_accepting_v2_or_unknown_fields() {
    let legacy = br#"{"version":1,"name":"Legacy","panels":{"data_open":true,"inspector_open":false,"diagnostics_open":false},"views":[{"id":"old","kind":"messages","topic":"/demo/status","message_type":"std_msgs/msg/String","qos":{"reliability":"reliable","durability":"volatile"}}],"connection_suggestion":{"domain":187}}"#;
    let migrated = Workspace::parse(legacy).unwrap();
    assert_eq!(migrated.version, FORMAT_VERSION);
    assert_eq!(migrated.layout.order, ["old"]);
    assert_eq!(migrated.layout.scene_percent, 65);
    assert_eq!(
        migrated.connection_suggestion.as_ref().unwrap().middleware,
        None
    );
    assert_eq!(
        Workspace::parse(&migrated.encode().unwrap()).unwrap(),
        migrated
    );
    assert_eq!(
        Workspace::parse(legacy).unwrap().encode().unwrap(),
        migrated.encode().unwrap()
    );
    let legacy = String::from_utf8(legacy.to_vec()).unwrap();
    for input in [
        legacy.replace("\"domain\":187", "\"domain\":187,\"domain\":187"),
        legacy.replace(
            "\"kind\":\"messages\"",
            "\"kind\":\"messages\",\"active\":true",
        ),
        legacy.replace("\"name\":\"Legacy\"", "\"name\":\"Legacy\",\"layout\":{}"),
        legacy.replace(
            "\"domain\":187",
            "\"domain\":187,\"middleware\":\"fast_dds\"",
        ),
    ] {
        assert!(Workspace::parse(input.as_bytes()).is_err(), "{input}");
    }
}
#[test]
fn all_renderer_settings_round_trip_exactly_and_restore_inactive() {
    let mut workspace = Preset::Visualize.workspace();
    let mut plot = fixture().views.remove(0);
    plot.id = "plot".into();
    let path = crate::plots::FieldPath(vec![
        crate::plots::PathSegment::Field("field.with.dots".into()),
        crate::plots::PathSegment::Index(3),
    ]);
    plot.kind = ViewKind::numeric_plot(&path);
    workspace.views.push(plot);
    let mut messages = fixture().views.remove(0);
    messages.id = "messages".into();
    workspace.views.push(messages);
    workspace.layout = Layout::for_views(&workspace.views);
    workspace.connection_suggestion = Some(ConnectionSuggestion {
        domain: 187,
        middleware: Some(Middleware::CycloneDds),
    });
    let roundtrip = Workspace::parse(&workspace.encode().unwrap())
        .unwrap()
        .inactive_restore()
        .unwrap();
    assert_eq!(roundtrip.configuration(), &workspace);
    assert_eq!(
        roundtrip.configuration().views[2].kind.plot_path(),
        Some(path)
    );
    assert!(workspace.views.len() == 4);
    for preset in [Preset::Inspect, Preset::Visualize, Preset::Analyze] {
        let config = preset.workspace();
        assert!(config.validate().is_ok());
        assert_eq!(Workspace::parse(&config.encode().unwrap()).unwrap(), config);
    }
}
#[test]
fn layout_edits_preserve_panel_identity_and_reject_invalid_references() {
    let mut workspace = Preset::Visualize.workspace();
    workspace.layout.scene_percent = 75;
    workspace.layout.reorder("spatial", 1).unwrap();
    assert_eq!(workspace.layout.order, ["image", "spatial"]);
    workspace.validate().unwrap();
    let before = workspace.layout.clone();
    assert!(workspace.layout.reorder("spatial", 2).is_err());
    assert_eq!(workspace.layout, before);
    workspace.layout.order[0] = "spatial".into();
    assert!(workspace.validate().is_err());
    for percent in [0, 34, 81, 255] {
        workspace.layout = before.clone();
        workspace.layout.scene_percent = percent;
        assert!(workspace.validate().is_err());
    }
}

#[test]
fn hostile_nested_renderer_layer_and_layout_members_are_rejected() {
    let base = serde_json::to_value(Preset::Visualize.workspace()).unwrap();
    for level in 0..5 {
        let mut value = base.clone();
        let target = match level {
            0 => &mut value["layout"],
            1 => &mut value["views"][0]["kind"],
            2 => &mut value["views"][0]["kind"]["layers"][0],
            3 => &mut value["views"][0]["kind"]["layers"][0]["qos"],
            _ => &mut value["views"][1]["kind"],
        };
        target
            .as_object_mut()
            .unwrap()
            .insert("command".into(), "run".into());
        assert!(
            Workspace::parse(&serde_json::to_vec(&value).unwrap()).is_err(),
            "level {level}"
        );
    }
    let bytes = String::from_utf8(Preset::Visualize.workspace().encode().unwrap()).unwrap();
    for (from, to) in [
        (
            "\"fixed_frame\": \"map\"",
            "\"fixed_frame\": \"map\", \"fixed_frame\": \"odom\"",
        ),
        (
            "\"display_mode\": \"fit\"",
            "\"display_mode\": \"fit\", \"display_mode\": \"actual_size\"",
        ),
        (
            "\"scene_percent\": 65",
            "\"scene_percent\": 65, \"scene_percent\": 65",
        ),
        (
            "\"renderer\": \"image\"",
            "\"renderer\": \"image\", \"renderer\": \"image\"",
        ),
    ] {
        assert!(bytes.contains(from));
        assert!(
            Workspace::parse(bytes.replace(from, to).as_bytes()).is_err(),
            "{from}"
        );
    }
    let messages = String::from_utf8(fixture().encode().unwrap()).unwrap();
    assert!(Workspace::parse(
        messages
            .replace(
                "\"renderer\": \"messages\"",
                "\"renderer\": \"messages\", \"active\": true"
            )
            .as_bytes()
    )
    .is_err());
}
#[test]
fn renderer_resource_and_range_limits_are_enforced() {
    let mut config = Preset::Visualize.workspace();
    let ViewKind::Spatial { layers, .. } = &mut config.views[0].kind else {
        unreachable!()
    };
    layers.push(layers[0].clone());
    assert!(config.validate().is_err());
    for kind in [
        ViewKind::NumericPlot { field_path: vec![] },
        ViewKind::NumericPlot {
            field_path: vec![PathSegment::Index(4096)],
        },
        ViewKind::NumericPlot {
            field_path: vec![PathSegment::Field("a".into()); 17],
        },
        ViewKind::Spatial {
            fixed_frame: "bad frame".into(),
            layers: vec![],
            transforms: None,
            clock_expectation: ClockExpectation::Unknown,
        },
        ViewKind::Image {
            display_mode: ImageMode::ActualSize,
        },
    ] {
        let mut config = fixture();
        config.views[0].kind = kind;
        assert!(config.validate().is_err());
    }
    let mut config = Preset::Visualize.workspace();
    let ViewKind::Spatial { layers, .. } = &mut config.views[0].kind else {
        unreachable!()
    };
    let layer = layers[0].clone();
    *layers = (0..5)
        .map(|i| {
            let mut l = layer.clone();
            l.id = format!("layer-{i}");
            l
        })
        .collect();
    assert!(config.validate().is_err());
}
#[test]
fn asynchronous_save_completion_does_not_clean_newer_edits() {
    let original = fixture();
    let mut doc = Document::new(original.clone()).unwrap();
    let mut edited = original.clone();
    edited.name = "Edited while worker saves".into();
    doc.edit(edited.clone()).unwrap();
    doc.mark_saved(original).unwrap();
    assert!(doc.is_dirty());
    assert_eq!(doc.current(), &edited);
    doc.mark_saved(edited).unwrap();
    assert!(!doc.is_dirty());
}
