use studio_protocol::{
    latest_pose::LatestPose,
    marker::{self, store::Store, *},
};

fn key(namespace: &str, id: i32) -> Key {
    Key {
        namespace: namespace.into(),
        id,
    }
}
fn marker(namespace: &str, id: i32) -> Marker {
    Marker {
        key: key(namespace, id),
        frame_id: "source".into(),
        stamp_ns: 10,
        pose: [0., 0., 0., 0., 0., 0., 1.],
        scale: [1.; 3],
        color: [1.; 4],
        lifetime_ns: 0,
        frame_locked: false,
        geometry: Geometry::Cube,
    }
}
fn add(marker: Marker) -> Operation {
    Operation::Add {
        marker: Box::new(marker),
    }
}
fn batch(operations: Vec<Operation>) -> Batch {
    Batch {
        contract_version: CONTRACT_VERSION,
        operations,
    }
}
fn ready(stamp: i64, x: f64) -> LatestPose {
    LatestPose::Ready {
        resolved_stamp_ns: stamp,
        components: [x, 0., 0., 0., 0., 0., 1.],
    }
}
fn store() -> (Store, marker::store::Token) {
    let mut store = Store::default();
    let token = store.start(1, 2, "world").unwrap();
    (store, token)
}

#[test]
fn ordered_duplicate_operations_type_replacement_and_namespace_deleteall_are_owner_scoped() {
    let (mut s, t) = store();
    let mut other = Store::default();
    let ot = other.start(1, 3, "world").unwrap();
    other
        .apply(ot, 1, batch(vec![add(marker("a", 1))]), 0)
        .unwrap();
    let mut sphere = marker("a", 1);
    sphere.geometry = Geometry::Sphere;
    s.apply(
        t,
        1,
        batch(vec![
            add(marker("a", 1)),
            Operation::Delete { key: key("a", 1) },
            add(sphere),
            add(marker("b", 1)),
            Operation::DeleteAll {
                namespace: "a".into(),
            },
            add(marker("a", 2)),
        ]),
        0,
    )
    .unwrap();
    assert_eq!(
        s.entries().keys().cloned().collect::<Vec<_>>(),
        [key("a", 2), key("b", 1)]
    );
    assert_eq!(s.counters().duplicate_keys, 2);
    assert_eq!(other.entries().len(), 1);
    s.apply(
        t,
        2,
        batch(vec![Operation::DeleteAll {
            namespace: String::new(),
        }]),
        0,
    )
    .unwrap();
    assert!(s.entries().is_empty());
    assert_eq!(other.entries().len(), 1);
    assert_eq!(s.budget(), Default::default());
}

#[test]
fn every_positive_lifetime_expires_and_replacement_restarts_deadline_without_replaying() {
    let (mut s, t) = store();
    let mut m = marker("a", 1);
    m.lifetime_ns = 5;
    s.apply(t, 1, batch(vec![add(m.clone()), add(marker("a", 2))]), 100)
        .unwrap();
    s.apply(t, 2, batch(vec![add(m)]), 104).unwrap();
    s.tick(t, 105).unwrap();
    assert_eq!(s.entries().len(), 2);
    s.tick(t, 109).unwrap();
    assert_eq!(s.entries().len(), 1);
    assert_eq!(s.counters().expired, 1);
    let mut tiny = marker("tiny", 1);
    tiny.lifetime_ns = 1;
    s.apply(t, 3, batch(vec![add(tiny)]), 109).unwrap();
    s.tick(t, 110).unwrap();
    assert!(!s.entries().contains_key(&key("tiny", 1)));
    s.tick(t, 10_000_000_000).unwrap();
    assert!(s.entries().contains_key(&key("a", 2)));
}

#[test]
fn stale_owner_sequence_and_activation_cannot_resurrect_data_after_clear_stop_or_restart() {
    let (mut s, t) = store();
    s.apply(t, 1, batch(vec![add(marker("a", 1))]), 0).unwrap();
    let request = s.requests(t, 0).unwrap().pop().unwrap();
    assert!(s.clear(t).unwrap());
    assert!(!s.complete(&request, ready(10, 1.), 1).unwrap());
    assert!(!s.apply(t, 1, batch(vec![add(marker("a", 1))]), 1).unwrap());
    s.stop();
    assert!(s.entries().is_empty());
    assert!(!s.apply(t, 2, batch(vec![add(marker("a", 1))]), 2).unwrap());
    let replacement = s.start(1, 2, "world").unwrap();
    assert_ne!(t, replacement);
    s.apply(replacement, 1, batch(vec![add(marker("a", 1))]), 3)
        .unwrap();
    assert!(!s.complete(&request, ready(10, 1.), 4).unwrap());
    assert!(s.entries()[&key("a", 1)].transform.is_none());
}

#[test]
fn stamped_transform_resolves_once_but_locked_and_zero_stamp_use_labeled_latest() {
    let (mut s, t) = store();
    let mut locked = marker("locked", 1);
    locked.frame_locked = true;
    let mut latest = marker("latest", 1);
    latest.stamp_ns = 0;
    let mut identity = marker("identity", 1);
    identity.frame_id = "world".into();
    s.apply(
        t,
        1,
        batch(vec![
            add(marker("acquisition", 1)),
            add(locked),
            add(latest),
            add(identity),
        ]),
        0,
    )
    .unwrap();
    let requests = s.requests(t, 0).unwrap();
    assert_eq!(requests.len(), 3);
    for req in &requests {
        if req.key.namespace == "acquisition" {
            assert_eq!(req.time, LookupTime::At(10));
            s.complete(req, ready(10, 2.), 1).unwrap();
        } else {
            assert_eq!(req.time, LookupTime::Latest);
            s.complete(req, ready(123, 2.), 1).unwrap();
        }
    }
    assert!(matches!(
        s.entries()[&key("identity", 1)].transform,
        Some(LatestPose::Ready {
            resolved_stamp_ns: 10,
            ..
        })
    ));
    assert!(s.requests(t, RETRY_NS - 1).unwrap().is_empty());
    let locked_req = s.requests(t, RETRY_NS).unwrap().pop().unwrap();
    assert_eq!(locked_req.key.namespace, "locked");
    s.complete(&locked_req, LatestPose::Missing, RETRY_NS + 1)
        .unwrap();
    assert_eq!(
        s.entries()[&key("locked", 1)].transform,
        Some(LatestPose::Missing)
    );
    let req = s.requests(t, RETRY_NS * 2).unwrap().pop().unwrap();
    s.complete(&req, ready(456, 3.), RETRY_NS * 2 + 1).unwrap();
    assert!(matches!(
        s.entries()[&key("locked", 1)].transform,
        Some(LatestPose::Ready {
            resolved_stamp_ns: 456,
            ..
        })
    ));
}

#[test]
fn late_transform_after_delete_replace_expiry_or_request_timeout_is_ignored() {
    for action in [0, 1, 2, 3] {
        let (mut s, t) = store();
        let mut m = marker("a", 1);
        if action == 2 {
            m.lifetime_ns = 10;
        }
        s.apply(t, 1, batch(vec![add(m)]), 0).unwrap();
        let request = s.requests(t, 0).unwrap().pop().unwrap();
        let now = match action {
            0 => {
                s.apply(t, 2, batch(vec![Operation::Delete { key: key("a", 1) }]), 1)
                    .unwrap();
                2
            }
            1 => {
                s.apply(t, 2, batch(vec![add(marker("a", 1))]), 1).unwrap();
                2
            }
            2 => 10,
            _ => LOOKUP_TIMEOUT_NS,
        };
        assert!(!s.complete(&request, ready(10, 2.), now).unwrap());
        if action == 3 {
            assert_eq!(
                s.entries()[&key("a", 1)].transform,
                Some(LatestPose::LookupTimeout)
            );
            let fresh = s.requests(t, now).unwrap().pop().unwrap();
            assert_ne!(fresh.request_id, request.request_id);
            assert!(!s.complete(&request, ready(10, 2.), now + 1).unwrap());
            assert!(s.complete(&fresh, ready(10, 3.), now + 2).unwrap());
        }
    }
}

#[test]
fn invalid_transform_hides_old_locked_pose_and_wrong_acquisition_time_is_rejected() {
    let (mut s, t) = store();
    let mut m = marker("a", 1);
    m.frame_locked = true;
    s.apply(t, 1, batch(vec![add(m)]), 0).unwrap();
    let req = s.requests(t, 0).unwrap().pop().unwrap();
    s.complete(&req, ready(10, 2.), 1).unwrap();
    let req = s.requests(t, RETRY_NS).unwrap().pop().unwrap();
    assert_eq!(
        s.complete(&req, ready(20, f64::NAN), RETRY_NS + 1),
        Err(Error::InvalidTransform)
    );
    assert_eq!(
        s.entries()[&key("a", 1)].transform,
        Some(LatestPose::Missing)
    );
    s.apply(t, 2, batch(vec![add(marker("b", 1))]), RETRY_NS + 2)
        .unwrap();
    let req = s
        .requests(t, RETRY_NS + 2)
        .unwrap()
        .into_iter()
        .find(|r| r.key.namespace == "b")
        .unwrap();
    assert_eq!(
        s.complete(&req, ready(11, 0.), RETRY_NS + 3),
        Err(Error::InvalidTransform)
    );
}

#[test]
fn gaps_and_faults_forget_ghosts_only_in_owner_and_remain_visible_through_clear() {
    let (mut s, t) = store();
    s.apply(t, 1, batch(vec![add(marker("old", 1))]), 0)
        .unwrap();
    s.apply(t, 3, batch(vec![add(marker("new", 1))]), 1)
        .unwrap();
    assert!(!s.entries().contains_key(&key("old", 1)));
    assert!(s.incomplete_history());
    assert_eq!(s.counters().source_gaps, 1);
    s.clear(t).unwrap();
    assert!(s.incomplete_history());
    s.apply(t, 4, batch(vec![add(marker("new", 1))]), 2)
        .unwrap();
    s.source_fault(t).unwrap();
    assert!(s.entries().is_empty());
    assert!(s.incomplete_history());
    s.start(1, 3, "world").unwrap();
    assert!(!s.incomplete_history());
}

#[test]
fn rejected_individual_add_hides_obsolete_key_and_diagnostics_are_bounded() {
    let (mut s, t) = store();
    s.apply(
        t,
        1,
        batch(vec![add(marker("a", 1)), add(marker("peer", 1))]),
        0,
    )
    .unwrap();
    let rejected = (0..100)
        .map(|id| Operation::Rejected {
            key: key("a", id),
            reason: Error::UnsupportedKind,
        })
        .collect();
    s.apply(t, 2, batch(rejected), 1).unwrap();
    assert!(!s.entries().contains_key(&key("a", 1)));
    assert!(s.entries().contains_key(&key("peer", 1)));
    assert_eq!(s.diagnostics().len(), MAX_DIAGNOSTICS);
    assert_eq!(s.counters().rejected, 100);
}

#[test]
fn aggregate_point_limit_replacement_and_expiry_release_budget() {
    let (mut s, t) = store();
    let point_marker = |id| {
        let mut m = marker("a", id);
        m.geometry = Geometry::Points {
            points: vec![[0.; 3]; MAX_POINTS],
            colors: vec![],
        };
        m
    };
    for seq in 1..=4 {
        s.apply(
            t,
            seq,
            batch(
                (0..4)
                    .map(|i| add(point_marker(((seq - 1) * 4 + i) as i32)))
                    .collect(),
            ),
            seq,
        )
        .unwrap();
    }
    assert_eq!(s.budget().points, MAX_RETAINED_POINTS);
    s.apply(t, 5, batch(vec![add(point_marker(0))]), 5).unwrap();
    assert_eq!(s.budget().points, MAX_RETAINED_POINTS);
    s.apply(t, 6, batch(vec![add(point_marker(17))]), 6)
        .unwrap();
    assert!(!s.entries().contains_key(&key("a", 17)));
    assert_eq!(s.diagnostics()[&key("a", 17)], Error::PointLimit);
    let mut expiring = point_marker(0);
    expiring.lifetime_ns = 1;
    s.apply(t, 7, batch(vec![add(expiring)]), 7).unwrap();
    s.tick(t, 8).unwrap();
    assert_eq!(s.budget().points, MAX_RETAINED_POINTS - MAX_POINTS);
    s.clear(t).unwrap();
    assert_eq!(s.budget(), Default::default());
}

#[test]
fn entry_text_resource_limits_do_not_evict_unrelated_entries() {
    for kind in 0..3 {
        let (mut s, t) = store();
        let count = if kind == 0 { MAX_MARKERS } else { 16 };
        let make = |id| {
            let mut m = marker("a", id);
            m.geometry = match kind {
                1 => Geometry::Text {
                    text: "x".repeat(MAX_TEXT_BYTES),
                },
                2 => Geometry::Mesh {
                    resource: format!("/{}", "x".repeat(MAX_RESOURCE_BYTES - 1)),
                    use_embedded_materials: true,
                },
                _ => Geometry::Cube,
            };
            m
        };
        s.apply(
            t,
            1,
            batch((0..count).map(|i| add(make(i as i32))).collect()),
            0,
        )
        .unwrap();
        s.apply(t, 2, batch(vec![add(make(count as i32))]), 1)
            .unwrap();
        assert_eq!(s.entries().len(), count);
        assert!(s.entries().contains_key(&key("a", 0)));
        assert_eq!(
            s.diagnostics()[&key("a", count as i32)],
            match kind {
                0 => Error::EntryLimit,
                1 => Error::TextLimit,
                _ => Error::ResourceLimit,
            }
        );
        s.apply(t, 3, batch(vec![add(make(0))]), 2).unwrap();
        assert_eq!(s.entries().len(), count);
    }
}

#[test]
fn malformed_batch_and_regressing_monotonic_clock_cannot_preserve_old_geometry() {
    let (mut s, t) = store();
    s.apply(t, 1, batch(vec![add(marker("a", 1))]), 10).unwrap();
    let mut broken = marker("bad", 1);
    broken.color[3] = f32::NAN;
    assert_eq!(
        s.apply(t, 2, batch(vec![add(broken)]), 11),
        Err(Error::InvalidMarker)
    );
    assert!(s.entries().is_empty());
    assert!(s.incomplete_history());
    s.apply(t, 3, batch(vec![add(marker("a", 1))]), 12).unwrap();
    assert_eq!(s.tick(t, 11), Err(Error::ClockRegressed));
    assert!(s.token().is_none());
    assert!(s.entries().is_empty());
}

#[test]
fn canonical_roundtrip_all_kinds_and_strict_geometry_wire_limits() {
    let p = vec![[1., 2., 3.], [4., 5., 6.]];
    let geometries = vec![
        Geometry::Arrow { points: vec![] },
        Geometry::Cube,
        Geometry::Sphere,
        Geometry::Cylinder,
        Geometry::LineStrip {
            points: p.clone(),
            colors: vec![],
        },
        Geometry::LineList {
            points: p.clone(),
            colors: vec![[1.; 4]; 2],
        },
        Geometry::CubeList {
            points: p.clone(),
            colors: vec![],
        },
        Geometry::SphereList {
            points: p.clone(),
            colors: vec![],
        },
        Geometry::Points {
            points: p.clone(),
            colors: vec![],
        },
        Geometry::Text {
            text: "Hello\n机器人".into(),
        },
        Geometry::Mesh {
            resource: "package://robot/mesh.obj".into(),
            use_embedded_materials: true,
        },
        Geometry::Triangles {
            points: vec![[0.; 3]; 3],
            colors: vec![],
        },
        Geometry::ArrowStrip {
            points: p,
            colors: vec![],
        },
    ];
    let b = batch(
        geometries
            .into_iter()
            .enumerate()
            .map(|(id, g)| {
                let mut m = marker("all", id as i32);
                m.geometry = g;
                add(m)
            })
            .collect(),
    );
    assert_eq!(Batch::decode(&b.encode().unwrap()).unwrap(), b);
    let mut json = serde_json::to_value(&b).unwrap();
    json["unknown"] = true.into();
    assert!(Batch::decode(&serde_json::to_vec(&json).unwrap()).is_err());
    assert!(Batch::decode(&vec![b' '; MAX_BATCH_BYTES + 1]).is_err());
    assert!(batch(vec![Operation::DeleteAll {
        namespace: "x".repeat(MAX_NAMESPACE_BYTES + 1)
    }])
    .validate()
    .is_err());
    assert!(Geometry::LineList {
        points: vec![[0.; 3]],
        colors: vec![]
    }
    .validate()
    .is_err());
    assert!(Geometry::Triangles {
        points: vec![[0.; 3]; 4],
        colors: vec![]
    }
    .validate()
    .is_err());
    assert!(Geometry::Points {
        points: vec![[f64::INFINITY; 3]],
        colors: vec![]
    }
    .validate()
    .is_err());
    assert!(Geometry::Points {
        points: vec![[0.; 3]; 2],
        colors: vec![[1.; 4]]
    }
    .validate()
    .is_err());
    assert_eq!(
        Geometry::Mesh {
            resource: "https://example.test/mesh.obj".into(),
            use_embedded_materials: false
        }
        .validate(),
        Err(Error::UnsupportedResource)
    );
}

#[test]
fn oversized_action_point_and_encoded_batches_are_rejected_before_partial_mutation() {
    let too_many = batch(
        (0..=MAX_OPERATIONS)
            .map(|_| Operation::DeleteAll {
                namespace: String::new(),
            })
            .collect(),
    );
    assert_eq!(too_many.validate(), Err(Error::BatchLimit));
    let points = batch(
        (0..5)
            .map(|id| {
                let mut m = marker("a", id);
                m.geometry = Geometry::Points {
                    points: vec![[0.; 3]; MAX_POINTS],
                    colors: vec![],
                };
                add(m)
            })
            .collect(),
    );
    assert_eq!(points.validate(), Err(Error::PointLimit));
    let text = batch(
        (0..MAX_OPERATIONS)
            .map(|id| {
                let mut m = marker("a", id as i32);
                m.geometry = Geometry::Text {
                    text: "x".repeat(MAX_TEXT_BYTES),
                };
                add(m)
            })
            .collect(),
    );
    assert!(text.validate().is_ok());
    assert!(text.encode().is_err());
    let (mut s, t) = store();
    s.apply(t, 1, batch(vec![add(marker("old", 1))]), 0)
        .unwrap();
    assert_eq!(s.apply(t, 2, text, 1), Err(Error::BatchLimit));
    assert!(s.entries().is_empty());
    assert!(s.incomplete_history());
}

#[test]
fn deleted_replaced_expired_cleared_and_stopped_entries_release_owned_source_storage() {
    let (mut s, t) = store();
    for action in 0..5 {
        let mut m = marker("a", 1);
        m.lifetime_ns = if action == 2 { 1 } else { 0 };
        s.apply(t, 1 + action * 2, batch(vec![add(m)]), action * 10)
            .unwrap();
        let weak = std::sync::Arc::downgrade(&s.entries()[&key("a", 1)].marker);
        match action {
            0 => {
                s.apply(t, 2, batch(vec![Operation::Delete { key: key("a", 1) }]), 1)
                    .unwrap();
            }
            1 => {
                s.apply(t, 4, batch(vec![add(marker("a", 1))]), 11).unwrap();
            }
            2 => {
                s.tick(t, 21).unwrap();
            }
            3 => {
                s.clear(t).unwrap();
            }
            _ => s.stop(),
        }
        assert!(weak.upgrade().is_none());
    }
}
