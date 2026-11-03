//! Complete bounded Marker extraction. No ROS callbacks, TF, asset loading or
//! generic preview values enter this codec. A caller owns live C storage.
use crate::native_schema::*;
use studio_protocol::marker::{self, Batch, Error, Geometry, Key, Marker, Operation};

unsafe fn sequence(root: Ptr, message: Ptr, name: &str) -> Result<(Ptr, Ptr, usize), String> {
    let (member, value) = field(root, message, name, 18, 2)?;
    if sp_bound(member) != 0 {
        return Err("Marker requires an unbounded ROS message sequence".into());
    }
    Ok((member, value, sp_length(member, value)))
}
unsafe fn vector(root: Ptr, message: Ptr) -> Result<[f64; 3], String> {
    schema(root, message, 3)?;
    let mut result = [0.; 3];
    for (i, name) in ["x", "y", "z"].into_iter().enumerate() {
        let (_, value) = field(root, message, name, 2, 0)?;
        result[i] = *value.cast::<f64>();
    }
    Ok(result)
}
unsafe fn color(root: Ptr, message: Ptr) -> Result<[f32; 4], String> {
    schema(root, message, 4)?;
    let mut result = [0.; 4];
    for (i, name) in ["r", "g", "b", "a"].into_iter().enumerate() {
        let (_, value) = field(root, message, name, 1, 0)?;
        result[i] = *value.cast::<f32>();
    }
    Ok(result)
}
unsafe fn extension_is_set(root: Ptr, message: Ptr) -> Result<bool, String> {
    if sp_count(root) == 15 {
        return Ok(false);
    }
    let (_, resource) = field(root, message, "texture_resource", 16, 0)?;
    let (_, _, uv_count) = sequence(root, message, "uv_coordinates")?;
    let (tm, texture) = field(root, message, "texture", 18, 0)?;
    let (dm, data) = field(sp_nested(tm), texture, "data", 8, 2)?;
    let texture_count = sp_length(dm, data);
    let (mm, mesh) = field(root, message, "mesh_file", 18, 0)?;
    let (dm, data) = field(sp_nested(mm), mesh, "data", 8, 2)?;
    let mesh_count = sp_length(dm, data);
    let (_, filename) = field(sp_nested(mm), mesh, "filename", 16, 0)?;
    Ok(sp_string_size(resource) != 0
        || uv_count != 0
        || texture_count != 0
        || mesh_count != 0
        || sp_string_size(filename) != 0)
}
unsafe fn geometry(
    root: Ptr,
    message: Ptr,
    kind: i32,
    total: &mut usize,
) -> Result<Geometry, Error> {
    let malformed = |_: String| Error::InvalidGeometry;
    if !(0..=11).contains(&kind) {
        return Err(Error::UnsupportedKind);
    }
    if extension_is_set(root, message).map_err(malformed)? {
        return Err(Error::UnsupportedResource);
    }
    let (points, colors) = if kind == 0 || matches!(kind, 4..=8 | 11) {
        let (pm, pv, count) = sequence(root, message, "points").map_err(malformed)?;
        let (cm, cv, color_count) = sequence(root, message, "colors").map_err(malformed)?;
        if count > marker::MAX_POINTS || color_count > marker::MAX_POINTS {
            return Err(Error::PointLimit);
        }
        let face_colors = kind == 11 && count.is_multiple_of(3) && color_count == count / 3;
        if kind != 0 && color_count != 0 && color_count != count && !face_colors {
            return Err(Error::InvalidGeometry);
        }
        // Native caller preflights the cumulative input bound before copying.
        *total += count;
        if *total > marker::MAX_BATCH_POINTS {
            return Err(Error::BatchLimit);
        }
        let mut points = Vec::with_capacity(count);
        let expanded_colors = if face_colors { count } else { color_count };
        let mut colors = Vec::with_capacity(if kind == 0 { 0 } else { expanded_colors });
        for i in 0..count {
            points.push(vector(sp_nested(pm), sp_element(pm, pv, i)).map_err(malformed)?);
        }
        if kind != 0 {
            for i in 0..expanded_colors {
                let source = if face_colors { i / 3 } else { i };
                colors.push(color(sp_nested(cm), sp_element(cm, cv, source)).map_err(malformed)?);
            }
        }
        (points, colors)
    } else {
        (Vec::new(), Vec::new())
    };
    Ok(match kind {
        0 => Geometry::Arrow { points },
        1 => Geometry::Cube,
        2 => Geometry::Sphere,
        3 => Geometry::Cylinder,
        4 => Geometry::LineStrip { points, colors },
        5 => Geometry::LineList { points, colors },
        6 => Geometry::CubeList { points, colors },
        7 => Geometry::SphereList { points, colors },
        8 => Geometry::Points { points, colors },
        9 => {
            let (_, value) = field(root, message, "text", 16, 0).map_err(malformed)?;
            Geometry::Text {
                text: string_bounded(value, marker::MAX_TEXT_BYTES)
                    .map_err(|_| Error::TextLimit)?,
            }
        }
        10 => {
            let (_, value) = field(root, message, "mesh_resource", 16, 0).map_err(malformed)?;
            let (_, embedded) =
                field(root, message, "mesh_use_embedded_materials", 6, 0).map_err(malformed)?;
            Geometry::Mesh {
                resource: string_bounded(value, marker::MAX_RESOURCE_BYTES)
                    .map_err(|_| Error::ResourceLimit)?,
                use_embedded_materials: *embedded.cast::<u8>() != 0,
            }
        }
        11 => Geometry::Triangles { points, colors },
        _ => unreachable!(),
    })
}
unsafe fn operation(root: Ptr, message: Ptr, total: &mut usize) -> Result<Operation, String> {
    if root.is_null() || message.is_null() || !matches!(sp_count(root), 15 | 19) {
        return Err("Expected exact supported native Marker schema/storage".into());
    }
    let (_, namespace) = field(root, message, "ns", 16, 0)?;
    let (_, id) = field(root, message, "id", 13, 0)?;
    let key = Key {
        namespace: string_bounded(namespace, marker::MAX_NAMESPACE_BYTES)?,
        id: *id.cast::<i32>(),
    };
    key.validate().map_err(|e| e.to_string())?;
    let (_, action) = field(root, message, "action", 13, 0)?;
    match *action.cast::<i32>() {
        2 => return Ok(Operation::Delete { key }),
        3 => {
            return Ok(Operation::DeleteAll {
                namespace: key.namespace,
            })
        }
        0 => {}
        _ => {
            return Ok(Operation::Rejected {
                key,
                reason: Error::InvalidMarker,
            })
        }
    }
    let mut read = || -> Result<Marker, Error> {
        let malformed = |_: String| Error::InvalidMarker;
        let (_, kind) = field(root, message, "type", 13, 0).map_err(malformed)?;
        let geometry = geometry(root, message, *kind.cast::<i32>(), total)?;
        let (hm, h) = field(root, message, "header", 18, 0).map_err(malformed)?;
        let (frame_id, stamp_ns) = header(sp_nested(hm), h).map_err(malformed)?;
        let (pm, p) = field(root, message, "pose", 18, 0).map_err(malformed)?;
        let pose = pose(sp_nested(pm), p).map_err(malformed)?;
        let (sm, s) = field(root, message, "scale", 18, 0).map_err(malformed)?;
        let scale = vector(sp_nested(sm), s).map_err(malformed)?;
        let (cm, c) = field(root, message, "color", 18, 0).map_err(malformed)?;
        let color = color(sp_nested(cm), c).map_err(malformed)?;
        let (lm, l) = field(root, message, "lifetime", 18, 0).map_err(malformed)?;
        let lifetime_ns = time(sp_nested(lm), l).map_err(malformed)? as u64;
        let (_, locked) = field(root, message, "frame_locked", 6, 0).map_err(malformed)?;
        let marker = Marker {
            key: key.clone(),
            frame_id,
            stamp_ns,
            pose,
            scale,
            color,
            lifetime_ns,
            frame_locked: *locked.cast::<u8>() != 0,
            geometry,
        };
        marker.validate()?;
        Ok(marker)
    };
    match read() {
        Ok(marker) => Ok(Operation::Add {
            marker: Box::new(marker),
        }),
        Err(Error::BatchLimit) => Err("Native MarkerArray exceeds cumulative point limit".into()),
        Err(reason) => Ok(Operation::Rejected { key, reason }),
    }
}

/// Extract an exact standard Marker or MarkerArray without generic previews.
/// Malformed ADDs become keyed rejections; malformed identities/whole-array
/// limits fail the owning source. This function performs no filesystem I/O.
/// # Safety
/// `owner` must be live exclusive worker-local C introspection storage for the
/// specified standard type. Every schema/message pointer must remain valid.
pub unsafe fn native_markers(owner: Ptr, message_type: &str) -> Result<Batch, String> {
    if owner.is_null() {
        return Err("Null native Marker owner".into());
    }
    let root = sp_schema(owner, 0);
    let message = sp_message(owner, 0);
    let mut points = 0;
    let operations = match message_type {
        marker::MESSAGE_TYPE => vec![operation(root, message, &mut points)?],
        marker::ARRAY_MESSAGE_TYPE => {
            schema(root, message, 1)?;
            let (member, value, count) = sequence(root, message, "markers")?;
            if count > marker::MAX_OPERATIONS {
                return Err("Native MarkerArray exceeds256 actions".into());
            }
            let mut operations = Vec::with_capacity(count);
            for i in 0..count {
                operations.push(operation(
                    sp_nested(member),
                    sp_element(member, value, i),
                    &mut points,
                )?);
            }
            operations
        }
        _ => return Err("Unsupported Marker native type".into()),
    };
    let batch = Batch {
        contract_version: marker::CONTRACT_VERSION,
        operations,
    };
    batch.encode()?;
    Ok(batch)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native_schema::test_support::Fixture;
    use serde_json::{json, Value};
    extern "C" {
        fn sp_resize(member: Ptr, value: Ptr, count: usize) -> i32;
    }
    fn source(kind: i32) -> Value {
        json!({"header":{"frame_id":"source","stamp":{"sec":2,"nanosec":3}},"ns":"demo","id":4,
            "type":kind,"action":0,"pose":{"position":{"x":1.,"y":2.,"z":3.},"orientation":{"w":1.}},
            "scale":{"x":1.,"y":2.,"z":3.},"color":{"r":0.25,"g":0.5,"b":0.75,"a":0.8},
            "lifetime":{"sec":1,"nanosec":9},"frame_locked":true})
    }
    fn fixture(value: Value) -> Fixture {
        Fixture::new("visualization_msgs", "Marker", value)
    }
    fn decode(f: &Fixture) -> Batch {
        unsafe { native_markers(f.0, marker::MESSAGE_TYPE) }.unwrap()
    }
    fn added(batch: &Batch) -> &Marker {
        let Operation::Add { marker } = &batch.operations[0] else {
            panic!("Expected ADD: {batch:?}")
        };
        marker
    }
    unsafe fn fill_points(root: Ptr, message: Ptr, count: usize) {
        let (pm, p) = field(root, message, "points", 18, 2).unwrap();
        assert_ne!(sp_resize(pm, p, count), 0);
        for index in 0..count {
            let point = sp_element(pm, p, index);
            for (axis, name) in ["x", "y", "z"].into_iter().enumerate() {
                let (_, v) = field(sp_nested(pm), point, name, 2, 0).unwrap();
                *v.cast::<f64>() = index as f64 + axis as f64 / 10.;
            }
        }
    }
    #[test]
    fn native_pinned_kinds_copy_complete_values_and_newer_kind_is_explicitly_unavailable() {
        for kind in 0..=11 {
            let mut value = source(kind);
            if kind == 9 {
                value["text"] = json!("机器人\n".repeat(100));
            }
            if kind == 10 {
                value["mesh_resource"] = json!("package://demo/mesh.obj");
                value["mesh_use_embedded_materials"] = true.into();
            }
            let f = fixture(value);
            if kind == 0 || matches!(kind, 4..=8 | 11) {
                unsafe {
                    fill_points(
                        sp_schema(f.0, 0),
                        sp_message(f.0, 0),
                        if kind == 11 { 3 } else { 2 },
                    );
                }
            }
            let batch = decode(&f);
            let m = added(&batch);
            assert_eq!(
                m.key,
                Key {
                    namespace: "demo".into(),
                    id: 4
                }
            );
            assert_eq!(m.stamp_ns, 2_000_000_003);
            assert_eq!(m.pose, [1., 2., 3., 0., 0., 0., 1.]);
            assert_eq!(m.scale, [1., 2., 3.]);
            assert_eq!(m.color, [0.25, 0.5, 0.75, 0.8]);
            assert_eq!(m.lifetime_ns, 1_000_000_009);
            assert!(m.frame_locked);
            if kind == 9 {
                assert_eq!(m.geometry.text_bytes(), "机器人\n".repeat(100).len());
            }
            assert_eq!(Batch::decode(&batch.encode().unwrap()).unwrap(), batch);
        }
        assert!(matches!(
            &decode(&fixture(source(12))).operations[0],
            Operation::Rejected {
                reason: Error::UnsupportedKind,
                ..
            }
        ));
    }
    #[test]
    fn full_4096_point_and_color_arrays_bypass_generic_preview_without_truncation() {
        let mut value = source(8);
        value["frame_locked"] = false.into();
        value["colors"] = json!(vec![
            json!({"r":0.25,"g":0.5,"b":0.75,"a":1.});
            marker::MAX_POINTS
        ]);
        let f = fixture(value);
        unsafe {
            fill_points(sp_schema(f.0, 0), sp_message(f.0, 0), marker::MAX_POINTS);
        }
        let batch = decode(&f);
        let m = added(&batch);
        assert_eq!(m.geometry.points().len(), 4096);
        assert_eq!(m.geometry.colors().len(), 4096);
        assert_eq!(m.geometry.points()[4095], [4095., 4095.1, 4095.2]);
        assert_eq!(m.geometry.colors()[4095], [0.25, 0.5, 0.75, 1.]);
    }
    #[test]
    fn triangle_face_colors_expand_to_bounded_canonical_vertex_colors() {
        let mut value = source(11);
        value["colors"] = json!([
            {"r":1.,"g":0.,"b":0.,"a":0.25},
            {"r":0.,"g":1.,"b":0.,"a":0.75}
        ]);
        let f = fixture(value);
        unsafe {
            fill_points(sp_schema(f.0, 0), sp_message(f.0, 0), 6);
        }
        let batch = decode(&f);
        let colors = added(&batch).geometry.colors();
        assert_eq!(colors.len(), 6);
        assert_eq!(&colors[..3], &[[1., 0., 0., 0.25]; 3]);
        assert_eq!(&colors[3..], &[[0., 1., 0., 0.75]; 3]);
    }
    #[test]
    fn default_delete_fields_need_no_valid_pose_time_frame_or_geometry() {
        for action in [2, 3] {
            let f = fixture(json!({"action":action,"ns":"delete me","id":-3,"type":999}));
            let batch = decode(&f);
            assert_eq!(
                batch.operations[0],
                if action == 2 {
                    Operation::Delete {
                        key: Key {
                            namespace: "delete me".into(),
                            id: -3,
                        },
                    }
                } else {
                    Operation::DeleteAll {
                        namespace: "delete me".into(),
                    }
                }
            );
        }
        let f = fixture(json!({"action":3}));
        assert_eq!(
            decode(&f).operations[0],
            Operation::DeleteAll {
                namespace: String::new()
            }
        );
    }
    #[test]
    fn keyed_invalid_add_removes_old_key_without_disturbing_array_peer() {
        let mut bad = source(1);
        bad["color"]["a"] = json!(2.);
        let mut peer = source(1);
        peer["id"] = json!(5);
        let f = Fixture::new(
            "visualization_msgs",
            "MarkerArray",
            json!({"markers":[source(1),peer,bad]}),
        );
        let batch = unsafe { native_markers(f.0, marker::ARRAY_MESSAGE_TYPE) }.unwrap();
        assert!(matches!(
            batch.operations[2],
            Operation::Rejected {
                reason: Error::InvalidMarker,
                ..
            }
        ));
        let mut s = marker::store::Store::default();
        let t = s.start(1, 1, "world").unwrap();
        s.apply(t, 1, batch, 0).unwrap();
        assert_eq!(s.entries().len(), 1);
        assert_eq!(s.entries().keys().next().unwrap().id, 5);
        assert_eq!(s.counters().rejected, 1);
    }
    #[test]
    fn invalid_native_values_counts_and_resources_are_rejected_before_retention() {
        let f = fixture(source(1));
        unsafe {
            let (cm, c) = field(sp_schema(f.0, 0), sp_message(f.0, 0), "color", 18, 0).unwrap();
            let (_, a) = field(sp_nested(cm), c, "a", 1, 0).unwrap();
            *a.cast::<f32>() = f32::NAN;
        }
        assert!(matches!(
            decode(&f).operations[0],
            Operation::Rejected {
                reason: Error::InvalidMarker,
                ..
            }
        ));
        let f = fixture(source(8));
        unsafe {
            fill_points(
                sp_schema(f.0, 0),
                sp_message(f.0, 0),
                marker::MAX_POINTS + 1,
            );
        }
        assert!(matches!(
            decode(&f).operations[0],
            Operation::Rejected {
                reason: Error::PointLimit,
                ..
            }
        ));
        for (field, value, reason) in [
            (
                "text",
                json!("x".repeat(marker::MAX_TEXT_BYTES + 1)),
                Error::TextLimit,
            ),
            (
                "mesh_resource",
                json!("https://example.test/mesh.obj"),
                Error::UnsupportedResource,
            ),
        ] {
            let mut v = source(if field == "text" { 9 } else { 10 });
            v[field] = value;
            let f = fixture(v);
            assert!(
                matches!(&decode(&f).operations[0],Operation::Rejected {reason:actual,..} if *actual==reason)
            );
        }
        let mut v = source(10);
        v["mesh_resource"] = json!("file:///tmp/example.obj");
        v["mesh_file"] = json!({"data":[1,2,3]});
        assert!(matches!(
            decode(&fixture(v)).operations[0],
            Operation::Rejected {
                reason: Error::UnsupportedResource,
                ..
            }
        ));
    }
    #[test]
    fn native_array_action_and_cumulative_point_limits_reject_the_whole_input() {
        let f = Fixture::new(
            "visualization_msgs",
            "MarkerArray",
            json!({"markers":vec![source(1);marker::MAX_OPERATIONS+1]}),
        );
        assert!(unsafe { native_markers(f.0, marker::ARRAY_MESSAGE_TYPE) }.is_err());
        let f = Fixture::new(
            "visualization_msgs",
            "MarkerArray",
            json!({"markers":vec![source(8);5]}),
        );
        unsafe {
            let (am, a) = field(sp_schema(f.0, 0), sp_message(f.0, 0), "markers", 18, 2).unwrap();
            for i in 0..5 {
                fill_points(sp_nested(am), sp_element(am, a, i), marker::MAX_POINTS);
            }
        }
        assert!(unsafe { native_markers(f.0, marker::ARRAY_MESSAGE_TYPE) }
            .unwrap_err()
            .contains("cumulative"));
        let wrong = Fixture::new("std_msgs", "String", json!({"data":"wrong type"}));
        assert!(unsafe { native_markers(wrong.0, marker::MESSAGE_TYPE) }.is_err());
    }
}
