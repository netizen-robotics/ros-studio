use super::*;

fn v2_bytes(workspace: &Workspace) -> Vec<u8> {
    let mut value = serde_json::to_value(workspace).unwrap();
    value["version"] = 2.into();
    value["layout"] = serde_json::json!({"order": workspace.layout.order, "columns": 2, "panel_weights": vec![100; workspace.views.len()], "collapsed": []});
    for view in value["views"].as_array_mut().unwrap() {
        let kind = view["kind"].as_object_mut().unwrap();
        if let Some(layers) = kind.get_mut("layers").and_then(|v| v.as_array_mut()) {
            for layer in layers {
                layer["visible"] = true.into();
            }
        }
        kind.remove("transforms");
        kind.remove("clock_expectation");
    }
    serde_json::to_vec(&value).unwrap()
}

#[test]
fn v2_migration_preserves_multilayer_slash_frame_and_scan_mismatch_as_blocked_draft() {
    let mut old = Preset::Visualize.workspace();
    old.views[0].topic = "/different_scan".into();
    let ViewKind::Spatial {
        fixed_frame,
        layers,
        ..
    } = &mut old.views[0].kind
    else {
        panic!()
    };
    *fixed_frame = "/map".into();
    let layer = layers[0].clone();
    for index in 1..4 {
        let mut extra = layer.clone();
        extra.id = format!("scan-{index}");
        extra.topic = format!("/scan_{index}");
        layers.push(extra);
    }
    let migrated = Workspace::parse(&v2_bytes(&old)).unwrap();
    assert_eq!(migrated.version, FORMAT_VERSION);
    assert_eq!(migrated.layout, old.layout);
    assert_eq!(migrated.views[0].topic, "/different_scan");
    let ViewKind::Spatial {
        fixed_frame,
        layers,
        transforms,
        clock_expectation,
    } = &migrated.views[0].kind
    else {
        panic!()
    };
    assert_eq!(fixed_frame, "/map");
    assert_eq!(layers.len(), 4);
    assert_eq!(layers[3].topic, "/scan_3");
    assert_eq!(transforms, &None);
    assert_eq!(clock_expectation, &ClockExpectation::Unknown);
    assert!(matches!(
        SupportedRenderers::all().validate_workspace(&migrated),
        Err(RestoreError::UnsupportedConfiguration { .. })
    ));
    let encoded = migrated.encode().unwrap();
    assert_eq!(Workspace::parse(&encoded).unwrap(), migrated);
    assert_eq!(Workspace::parse(&v2_bytes(&old)).unwrap(), migrated);
    assert!(String::from_utf8(encoded)
        .unwrap()
        .contains("\"version\": 7"));
}

#[test]
fn v2_migration_preserves_nonspatial_settings_and_rejects_new_or_duplicate_fields() {
    for original in [Preset::Inspect.workspace(), Preset::Analyze.workspace()] {
        assert_eq!(Workspace::parse(&v2_bytes(&original)).unwrap(), original);
    }
    let original = Preset::Visualize.workspace();
    let bytes = String::from_utf8(v2_bytes(&original)).unwrap();
    for (from, to) in [
        ("\"version\":2", "\"version\":2,\"version\":2"),
        (
            "\"fixed_frame\":\"map\"",
            "\"fixed_frame\":\"map\",\"fixed_frame\":\"odom\"",
        ),
        (
            "\"fixed_frame\":\"map\"",
            "\"fixed_frame\":\"map\",\"transforms\":null",
        ),
        (
            "\"fixed_frame\":\"map\"",
            "\"fixed_frame\":\"map\",\"clock_expectation\":\"system\"",
        ),
    ] {
        assert!(bytes.contains(from));
        assert!(Workspace::parse(bytes.replace(from, to).as_bytes()).is_err());
    }
    let messages = String::from_utf8(v2_bytes(&Preset::Inspect.workspace())).unwrap();
    assert!(Workspace::parse(
        messages
            .replace(
                "\"renderer\":\"messages\"",
                "\"renderer\":\"messages\",\"active\":true"
            )
            .as_bytes()
    )
    .is_err());
}

#[test]
fn v3_tf_configuration_rejects_unknown_duplicate_and_authorizing_members() {
    let base = serde_json::to_value(Preset::Visualize.workspace()).unwrap();
    for path in [
        vec!["views", "0", "kind"],
        vec!["views", "0", "kind", "transforms"],
        vec!["views", "0", "kind", "transforms", "dynamic_tf"],
        vec!["views", "0", "kind", "transforms", "static_tf", "qos"],
    ] {
        let mut value = base.clone();
        let mut target = &mut value;
        for key in path {
            target = if key == "0" {
                &mut target[0]
            } else {
                &mut target[key]
            };
        }
        target
            .as_object_mut()
            .unwrap()
            .insert("verified".into(), true.into());
        assert!(Workspace::parse(&serde_json::to_vec(&value).unwrap()).is_err());
    }
    let text = String::from_utf8(Preset::Visualize.workspace().encode().unwrap()).unwrap();
    for (from, to) in [
        (
            "\"topic\": \"/tf\"",
            "\"topic\": \"/tf\", \"topic\": \"/another\"",
        ),
        (
            "\"clock_expectation\": \"system\"",
            "\"clock_expectation\": \"system\", \"clock_expectation\": \"unknown\"",
        ),
        (
            "\"clock_expectation\": \"system\"",
            "\"clock_expectation\": \"verified_system\"",
        ),
    ] {
        assert!(text.contains(from));
        assert!(Workspace::parse(text.replace(from, to).as_bytes()).is_err());
    }
}

#[test]
fn live_capability_blockers_are_explicit_without_changing_valid_inactive_files() {
    let base = Preset::Visualize.workspace();
    let mutations: [fn(&mut ViewConfig); 10] = [
        |v| {
            let ViewKind::Spatial { layers, .. } = &mut v.kind else {
                panic!()
            };
            let mut extra = layers[0].clone();
            extra.id = "extra".into();
            layers.push(extra);
        },
        |v| {
            let ViewKind::Spatial { fixed_frame, .. } = &mut v.kind else {
                panic!()
            };
            *fixed_frame = "/map".into();
        },
        |v| v.topic = "/another_scan".into(),
        |v| v.qos.durability = Durability::TransientLocal,
        |v| {
            let ViewKind::Spatial { transforms, .. } = &mut v.kind else {
                panic!()
            };
            *transforms = None;
        },
        |v| {
            let ViewKind::Spatial { transforms, .. } = &mut v.kind else {
                panic!()
            };
            transforms.as_mut().unwrap().dynamic_tf.message_type = "std_msgs/msg/String".into();
        },
        |v| {
            let ViewKind::Spatial { transforms, .. } = &mut v.kind else {
                panic!()
            };
            transforms.as_mut().unwrap().dynamic_tf.qos.durability = Durability::TransientLocal;
        },
        |v| {
            let ViewKind::Spatial { transforms, .. } = &mut v.kind else {
                panic!()
            };
            transforms.as_mut().unwrap().static_tf.qos.reliability = Reliability::BestEffort;
        },
        |v| {
            let ViewKind::Spatial { transforms, .. } = &mut v.kind else {
                panic!()
            };
            transforms.as_mut().unwrap().static_tf.qos.durability = Durability::Volatile;
        },
        |v| {
            let ViewKind::Spatial { transforms, .. } = &mut v.kind else {
                panic!()
            };
            let inputs = transforms.as_mut().unwrap();
            inputs.static_tf.topic = inputs.dynamic_tf.topic.clone();
        },
    ];
    for mutate in mutations {
        let mut draft = base.clone();
        mutate(&mut draft.views[0]);
        draft.validate().unwrap();
        let error = SupportedRenderers::all()
            .validate_workspace(&draft)
            .unwrap_err();
        let RestoreError::UnsupportedConfiguration { panel, reason } = error else {
            panic!()
        };
        assert_eq!(panel, "spatial");
        assert!(!reason.is_empty());
        assert_eq!(Workspace::parse(&draft.encode().unwrap()).unwrap(), draft);
    }
}

#[test]
fn unsupported_preview_preserves_document_prior_candidate_and_cannot_emit_stops() {
    let current = Preset::Inspect.workspace();
    let document = Document::new(current.clone()).unwrap();
    let mut restore = RestoreCoordinator::new();
    let id = restore
        .preview(current.clone(), 1, SupportedRenderers::all())
        .unwrap();
    let before = restore.state().clone();
    let mut unsupported = Preset::Visualize.workspace();
    let ViewKind::Spatial { transforms, .. } = &mut unsupported.views[0].kind else {
        panic!()
    };
    *transforms = None;
    assert!(matches!(
        restore.preview(unsupported.clone(), 1, SupportedRenderers::all()),
        Err(RestoreError::UnsupportedConfiguration { .. })
    ));
    assert_eq!(restore.state(), &before);
    assert_eq!(restore.configuration(), Some(&current));
    assert_eq!(document.current(), &current);
    let mut empty = RestoreCoordinator::new();
    assert!(empty
        .preview(unsupported, 1, SupportedRenderers::all())
        .is_err());
    assert_eq!(empty.state(), &RestoreState::Idle);
    assert_eq!(
        empty.apply(id, 1, true, ApplyApproval::DiscardUnsaved, &[]),
        Err(RestoreError::NoPreview)
    );
}

#[test]
fn clock_expectation_never_serializes_verification_and_inactive_files_ignore_live_capacity() {
    let mut workspace = Preset::Visualize.workspace();
    for expectation in [ClockExpectation::Unknown, ClockExpectation::System] {
        let ViewKind::Spatial {
            clock_expectation, ..
        } = &mut workspace.views[0].kind
        else {
            panic!()
        };
        *clock_expectation = expectation;
        SupportedRenderers::all()
            .validate_workspace(&workspace)
            .unwrap();
        let text = String::from_utf8(workspace.encode().unwrap()).unwrap();
        assert!(!text.contains("verified"));
        assert!(!text.contains("authorization"));
        assert_eq!(
            Workspace::parse(text.as_bytes())
                .unwrap()
                .inactive_restore()
                .unwrap()
                .configuration(),
            &workspace
        );
    }
    for index in 0..2 {
        let mut view = workspace.views[0].clone();
        view.id = format!("extra-{index}");
        workspace.views.push(view);
    }
    workspace.layout = Layout::for_views(&workspace.views);
    workspace.validate().unwrap();
    SupportedRenderers::all()
        .validate_workspace(&workspace)
        .unwrap();
    assert_eq!(
        Workspace::parse(&workspace.encode().unwrap()).unwrap(),
        workspace
    );
}

#[test]
fn v3_grid_migrates_strictly_to_inactive_scene_configuration() {
    let original = Preset::Visualize.workspace();
    let mut value = serde_json::to_value(&original).unwrap();
    value["version"] = 3.into();
    value["layout"] = serde_json::json!({"order":["image","spatial"],"columns":2,"panel_weights":[250,100],"collapsed":["spatial"]});
    value["views"][0]["kind"]["layers"][0]["visible"] = false.into();
    let migrated = Workspace::parse(&serde_json::to_vec(&value).unwrap()).unwrap();
    assert_eq!(migrated.layout.order, ["image", "spatial"]);
    assert_eq!(migrated.layout.scene_percent, 65);
    assert_eq!(migrated.views, original.views);
    let encoded = String::from_utf8(migrated.encode().unwrap()).unwrap();
    for obsolete in [
        "visible",
        "collapsed",
        "columns",
        "panel_weights",
        "enabled",
    ] {
        assert!(!encoded.contains(obsolete));
    }
    for bad in [
        serde_json::json!(["spatial", "spatial"]),
        serde_json::json!(["missing"]),
    ] {
        let mut invalid = value.clone();
        invalid["layout"]["order"] = bad;
        assert!(Workspace::parse(&serde_json::to_vec(&invalid).unwrap()).is_err());
    }
    let text = serde_json::to_string(&value).unwrap();
    for (from, to) in [
        ("\"columns\":2", "\"columns\":2,\"columns\":2"),
        ("\"visible\":false", "\"visible\":false,\"visible\":true"),
    ] {
        assert!(text.contains(from));
        assert!(Workspace::parse(text.replace(from, to).as_bytes()).is_err());
    }
    value["layout"]["scene_percent"] = 65.into();
    assert!(Workspace::parse(&serde_json::to_vec(&value).unwrap()).is_err());
}
