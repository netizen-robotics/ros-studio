//! Independent synthetic asset/geometry checks; no ROS, native window or GPU.
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
};
use studio_desktop::{
    robot::{load_model, LoadOptions, RobotErrorKind, RobotModel, MAX_MESH_BYTES, MAX_URDF_BYTES},
    spatial::{
        robot_geometry::{prepare_robot, robot_vertices},
        CameraPose, Point3, RigidTransform,
    },
};

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "studio-robot-fixture-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn write(&self, name: &str, bytes: impl AsRef<[u8]>) -> PathBuf {
        let path = self.0.join(name);
        fs::write(&path, bytes).unwrap();
        path
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn options() -> LoadOptions {
    LoadOptions {
        package_roots: BTreeMap::new(),
    }
}
fn fixture() -> (PathBuf, LoadOptions) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/robot_visualizer");
    let options = LoadOptions {
        package_roots: BTreeMap::from([("fixture_robot".into(), root.clone())]),
    };
    (root.join("articulated.urdf"), options)
}
fn load_fixture() -> RobotModel {
    let (path, options) = fixture();
    load_model(&path, &options, &AtomicBool::new(false)).unwrap()
}
fn single_visual(geometry: &str, material: &str) -> String {
    format!("<robot name='test'><link name='base'><visual><geometry>{geometry}</geometry>{material}</visual></link></robot>")
}
fn assert_rejected(xml: &str) {
    let scratch = Scratch::new();
    let path = scratch.write("model.urdf", xml);
    let error = load_model(&path, &options(), &AtomicBool::new(false)).unwrap_err();
    assert!(
        !error.message.is_empty(),
        "rejected input needs an actionable diagnostic"
    );
    assert_ne!(error.kind, RobotErrorKind::Cancelled);
}

#[test]
fn malformed_xml_names_are_rejected_even_inside_ignored_metadata() {
    for metadata in ["<3d/>", "<metadata @bad='1'/>"] {
        assert_rejected(&format!(
            "<robot name='r'><link name='base'/><gazebo>{metadata}</gazebo></robot>"
        ));
    }
}

#[test]
fn original_fixture_loads_primitives_relative_ascii_and_explicit_package_binary_solid_header() {
    let model = load_fixture();
    assert_eq!(
        model
            .links
            .iter()
            .map(|l| l.name.as_str())
            .collect::<Vec<_>>(),
        ["base", "arm", "tip", "ascii_mesh", "binary_mesh"]
    );
    assert_eq!(model.joints.len(), 4);
    assert_eq!(model.links[model.root_link].name, "base");
    assert_eq!(
        model.links.iter().map(|l| l.visuals.len()).sum::<usize>(),
        6
    );
    let prepared = prepare_robot(&model, &AtomicBool::new(false)).unwrap();
    assert_eq!(prepared.link_names.len(), 5);
    let binary = prepared
        .visuals
        .iter()
        .find(|v| model.links[v.link_index].name == "binary_mesh")
        .unwrap();
    assert_eq!(binary.triangles.len(), 1);
    // Binary asset has the same authored triangle as ASCII despite its `solid` header.
    let p = binary.triangles[0].0[2];
    for (a, b) in p.into_iter().zip([0.0, -0.1, 0.5]) {
        assert!((a - b).abs() < 1e-6);
    }
}

#[test]
fn scale_visual_origin_then_external_pose_is_applied_once_and_missing_links_emit_nothing() {
    let model = load_fixture();
    let prepared = prepare_robot(&model, &AtomicBool::new(false)).unwrap();
    let ascii = model
        .links
        .iter()
        .position(|l| l.name == "ascii_mesh")
        .unwrap();
    let visual = prepared
        .visuals
        .iter()
        .find(|v| v.link_index == ascii)
        .unwrap();
    let point = visual.triangles[0].0[1];
    for (a, b) in point.into_iter().zip([0.2, 2.0, 0.0]) {
        assert!((a - b).abs() < 1e-9);
    }
    let camera = CameraPose::orbit(0.5, 0.8, 12.0, [0.0; 3]);
    let mut poses = vec![None; model.links.len()];
    assert!(robot_vertices(&prepared, &poses, camera, [800, 600])
        .unwrap()
        .is_empty());
    let q = std::f64::consts::FRAC_1_SQRT_2;
    poses[ascii] = Some(RigidTransform::new([1.0, 2.0, 0.2], [0.0, 0.0, q, q]).unwrap());
    let vertices = robot_vertices(&prepared, &poses, camera, [800, 600]).unwrap();
    assert_eq!(vertices.len(), 3, "only the resolved ASCII link may render");
    let expected = camera.clip(Point3([-1.0, 2.2, 0.2]), 800.0 / 600.0);
    for (a, b) in vertices[1].clip.into_iter().zip(expected) {
        assert!((a - b).abs() < 1e-5);
    }
    poses[ascii] = None;
    assert!(robot_vertices(&prepared, &poses, camera, [800, 600])
        .unwrap()
        .is_empty());
    poses[ascii] = Some(RigidTransform::identity());
    assert_eq!(
        robot_vertices(&prepared, &poses, camera, [800, 600])
            .unwrap()
            .len(),
        3
    );
    assert!(
        robot_vertices(&prepared, &poses[..4], camera, [800, 600]).is_err(),
        "pose count mismatch must not shift link identities"
    );
}

#[test]
fn material_alpha_zero_hides_only_that_visual_and_fractional_alpha_is_prepared() {
    let model = load_fixture();
    let prepared = prepare_robot(&model, &AtomicBool::new(false)).unwrap();
    let mut poses = vec![None; model.links.len()];
    poses[model.root_link] = Some(RigidTransform::identity());
    let vertices = robot_vertices(
        &prepared,
        &poses,
        CameraPose::orbit(0.5, 0.8, 12.0, [0.0; 3]),
        [800, 600],
    )
    .unwrap();
    assert_eq!(
        vertices.len(),
        36,
        "opaque box is12triangles; hidden sphere must contribute none"
    );
    assert!(vertices.iter().all(|v| v.color[3] == 1.0));
    let scratch = Scratch::new();
    let path = scratch.write(
        "alpha.urdf",
        single_visual(
            "<sphere radius='0.1'/>",
            "<material><color rgba='1 0 0 0.5'/></material>",
        ),
    );
    let alpha = load_model(&path, &LoadOptions::default(), &AtomicBool::new(false)).unwrap();
    assert_eq!(
        prepare_robot(&alpha, &AtomicBool::new(false))
            .unwrap()
            .visuals[0]
            .rgba[3],
        0.5
    );
    assert_rejected(&single_visual(
        "<sphere radius='0.1'/>",
        "<material><texture filename='image.png'/></material>",
    ));
}

#[test]
fn malformed_descriptions_and_non_tree_joint_graphs_fail_explicitly() {
    for xml in [
        "<robot name='broken'><link name='base'></robot>",
        "<robot name='duplicates'><link name='base'/><link name='base'/></robot>",
        "<robot name='disconnected'><link name='base'/><link name='other'/></robot>",
        "<robot name='unknown'><link name='base'/><joint name='j' type='fixed'><parent link='base'/><child link='missing'/></joint></robot>",
        "<robot name='cycle'><link name='a'/><link name='b'/><joint name='ab' type='fixed'><parent link='a'/><child link='b'/></joint><joint name='ba' type='fixed'><parent link='b'/><child link='a'/></joint></robot>",
        "<!DOCTYPE robot [<!ENTITY x SYSTEM 'file:///etc/passwd'>]><robot name='entity'><link name='&x;'/></robot>",
        "<robot name='xacro' xmlns:xacro='http://www.ros.org/wiki/xacro'><xacro:include filename='other.urdf'/><link name='base'/></robot>",
    ] { assert_rejected(xml); }
}

#[test]
fn missing_unmapped_absolute_network_and_parent_mesh_paths_are_rejected() {
    for name in [
        "missing.stl",
        "../outside.stl",
        "package://unmapped/mesh.stl",
        "package://fixture_robot/../outside.stl",
        "/tmp/outside.stl",
        "file:///tmp/outside.stl",
        "https://example.invalid/mesh.stl",
    ] {
        assert_rejected(&single_visual(&format!("<mesh filename='{name}'/>"), ""));
    }
    let (path, _) = fixture();
    assert!(
        load_model(&path, &options(), &AtomicBool::new(false)).is_err(),
        "package resolution must be explicit even when relative asset succeeds"
    );
}

#[cfg(unix)]
#[test]
fn symlink_mesh_escape_outside_urdf_root_is_rejected() {
    let outside = Scratch::new();
    let inside = Scratch::new();
    let mesh = outside.write(
        "outside.stl",
        include_bytes!("../../../fixtures/robot_visualizer/meshes/wedge-ascii.stl"),
    );
    std::os::unix::fs::symlink(mesh, inside.0.join("escape.stl")).unwrap();
    let path = inside.write(
        "model.urdf",
        single_visual("<mesh filename='escape.stl'/>", ""),
    );
    assert!(load_model(&path, &options(), &AtomicBool::new(false)).is_err());
}

#[test]
fn existing_asset_cannot_escape_relative_or_explicit_package_root() {
    let scratch = Scratch::new();
    scratch.write(
        "outside.stl",
        include_bytes!("../../../fixtures/robot_visualizer/meshes/wedge-ascii.stl"),
    );
    let selected = scratch.0.join("selected");
    fs::create_dir(&selected).unwrap();
    let path = selected.join("model.urdf");
    let mapped = LoadOptions {
        package_roots: BTreeMap::from([("fixture_robot".into(), selected.clone())]),
    };
    for filename in ["../outside.stl", "package://fixture_robot/../outside.stl"] {
        fs::write(
            &path,
            single_visual(&format!("<mesh filename='{filename}'/>"), ""),
        )
        .unwrap();
        assert!(
            load_model(&path, &mapped, &AtomicBool::new(false)).is_err(),
            "valid outside assets must remain inaccessible: {filename}"
        );
    }
}

#[test]
fn oversized_urdf_mesh_and_pre_cancelled_loads_fail_without_geometry() {
    let scratch = Scratch::new();
    let path = scratch.write("oversized.urdf", b"");
    fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .unwrap()
        .set_len((MAX_URDF_BYTES + 1) as u64)
        .unwrap();
    assert_eq!(
        load_model(&path, &options(), &AtomicBool::new(false))
            .unwrap_err()
            .kind,
        RobotErrorKind::Limit
    );
    let mesh = scratch.write("large.stl", b"");
    fs::OpenOptions::new()
        .write(true)
        .open(mesh)
        .unwrap()
        .set_len((MAX_MESH_BYTES + 1) as u64)
        .unwrap();
    let path = scratch.write(
        "model.urdf",
        single_visual("<mesh filename='large.stl'/>", ""),
    );
    assert_eq!(
        load_model(&path, &options(), &AtomicBool::new(false))
            .unwrap_err()
            .kind,
        RobotErrorKind::Limit
    );
    let (path, options) = fixture();
    assert_eq!(
        load_model(&path, &options, &AtomicBool::new(true))
            .unwrap_err()
            .kind,
        RobotErrorKind::Cancelled
    );
    assert_eq!(
        prepare_robot(&load_fixture(), &AtomicBool::new(true))
            .unwrap_err()
            .kind,
        RobotErrorKind::Cancelled
    );
}

#[test]
fn expanded_mesh_instances_link_and_visual_counts_enforce_budgets() {
    let scratch = Scratch::new();
    let mut links = String::from("<robot name='too_many_links'><link name='l0'/>");
    for i in 1..=128 {
        links.push_str(&format!("<link name='l{i}'/><joint name='j{i}' type='fixed'><parent link='l{}'/><child link='l{i}'/></joint>", i-1));
    }
    links.push_str("</robot>");
    let path = scratch.write("links.urdf", links);
    assert_eq!(
        load_model(&path, &options(), &AtomicBool::new(false))
            .unwrap_err()
            .kind,
        RobotErrorKind::Limit
    );
    let visual = "<visual><geometry><box size='1 1 1'/></geometry></visual>";
    let path = scratch.write(
        "visuals.urdf",
        format!(
            "<robot name='too_many_visuals'><link name='base'>{}</link></robot>",
            visual.repeat(257)
        ),
    );
    assert_eq!(
        load_model(&path, &options(), &AtomicBool::new(false))
            .unwrap_err()
            .kind,
        RobotErrorKind::Limit
    );
    // 200 faces fit one mesh; 251 separate instances fit the visual bound but
    // expand to 50,200 faces. Asset caching must not bypass the geometry budget.
    let face =
        "facet normal 0 0 1 outer loop vertex 0 0 0 vertex 1 0 0 vertex 0 1 0 endloop endfacet\n";
    scratch.write(
        "repeated.stl",
        format!("solid faces\n{}endsolid faces\n", face.repeat(200)),
    );
    let visual = "<visual><geometry><mesh filename='repeated.stl'/></geometry></visual>";
    let path = scratch.write(
        "instances.urdf",
        format!(
            "<robot name='expanded_budget'><link name='base'>{}</link></robot>",
            visual.repeat(251)
        ),
    );
    let outcome = load_model(&path, &options(), &AtomicBool::new(false))
        .and_then(|model| prepare_robot(&model, &AtomicBool::new(false)));
    assert_eq!(outcome.unwrap_err().kind, RobotErrorKind::Limit);
}

#[test]
fn truncated_degenerate_and_nonfinite_stl_are_rejected() {
    let scratch = Scratch::new();
    let path = scratch.write(
        "model.urdf",
        single_visual("<mesh filename='bad.stl'/>", ""),
    );
    let binary = include_bytes!("../../../fixtures/robot_visualizer/meshes/wedge-binary.stl");
    scratch.write("bad.stl", &binary[..binary.len() - 1]);
    assert!(
        load_model(&path, &options(), &AtomicBool::new(false)).is_err(),
        "binary attribute/count truncation cannot be accepted"
    );
    for vertex in ["0 0 0", "nan 0 0"] {
        scratch.write("bad.stl",format!("solid bad\nfacet normal 0 0 1 outer loop vertex 0 0 0 vertex 1 0 0 vertex {vertex} endloop endfacet\nendsolid bad\n"));
        assert!(
            load_model(&path, &options(), &AtomicBool::new(false)).is_err(),
            "degenerate/nonfinite triangle cannot reach rendering"
        );
    }
}
