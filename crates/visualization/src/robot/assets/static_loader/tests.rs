use super::*;
use std::sync::atomic::{AtomicU64, Ordering};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Dir(PathBuf);
impl Dir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "studio-static-mesh-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path.canonicalize().unwrap())
    }
}
impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/robot_visualizer")
        .canonicalize()
        .unwrap()
}
fn options(base: &Path) -> LoadOptions {
    LoadOptions {
        package_roots: BTreeMap::from([("models".into(), base.to_owned())]),
    }
}
fn copy_stl(dir: &Path, name: &str) {
    std::fs::copy(fixture().join("meshes/wedge-ascii.stl"), dir.join(name)).unwrap();
}
#[test]
fn relative_package_file_and_absolute_resources_share_canonical_cache() {
    let cancel = AtomicBool::new(false);
    let root = fixture();
    let mut loader = MeshAssetLoader::new(Some(&root), &options(&root), &cancel).unwrap();
    let first = loader
        .load("meshes/wedge-ascii.stl", false, &cancel)
        .unwrap();
    let stats = loader.stats();
    let path = root.join("meshes/wedge-ascii.stl");
    for name in [
        "package://models/meshes/wedge-ascii.stl".into(),
        url::Url::from_file_path(&path).unwrap().to_string(),
        path.to_string_lossy().into_owned(),
    ] {
        let repeated = loader.load(&name, false, &cancel).unwrap();
        assert!(Arc::ptr_eq(&first, &repeated));
        assert_eq!(loader.stats(), stats);
    }
    assert!(first.triangle_count > 0);
    assert_eq!(
        first.triangle_count,
        first.parts.iter().map(|p| p.triangles.len()).sum::<usize>()
    );
    let mut no_base = MeshAssetLoader::new(None, &options(&root), &cancel).unwrap();
    assert!(no_base
        .load("package://models/meshes/wedge-ascii.stl", false, &cancel)
        .is_ok());
    assert!(no_base
        .load("meshes/wedge-ascii.stl", false, &cancel)
        .is_err());
}
#[test]
fn embedded_materials_uvs_images_and_robot_output_use_the_same_decoders() {
    let cancel = AtomicBool::new(false);
    let root = fixture();
    let mut loader = MeshAssetLoader::new(Some(&root), &LoadOptions::default(), &cancel).unwrap();
    for (resource, urdf) in [
        ("materials/panels.obj", "obj.urdf"),
        ("materials/panels.dae", "collada.urdf"),
        ("materials/textured.obj", "textured.urdf"),
        ("materials/textured.dae", "collada-textured.urdf"),
    ] {
        let asset = loader.load(resource, true, &cancel).unwrap();
        let robot = load_model(&root.join(urdf), &LoadOptions::default(), &cancel).unwrap();
        let visuals = &robot.links[0].visuals;
        assert_eq!(visuals.len() % asset.parts.len(), 0);
        for (part, visual) in visuals
            .chunks(asset.parts.len())
            .flat_map(|instance| asset.parts.iter().zip(instance))
        {
            match &visual.geometry {
                RobotGeometry::Mesh { triangles, .. } => {
                    assert_eq!(part.triangles.as_ref(), triangles.as_ref())
                }
                RobotGeometry::MeshWithUv {
                    triangles,
                    uvs,
                    texture,
                    ..
                } => {
                    assert_eq!(part.triangles.as_ref(), triangles.as_ref());
                    assert_eq!(part.uvs.as_deref(), Some(uvs.as_ref()));
                    assert_eq!(part.texture.as_deref(), texture.as_deref());
                }
                _ => panic!("Expected mesh"),
            }
            assert_eq!(part.rgba.unwrap_or([0.7, 0.7, 0.7, 1.]), visual.rgba);
        }
    }
    let obj = loader
        .load("materials/textured.obj", true, &cancel)
        .unwrap();
    let dae = loader
        .load("materials/textured.dae", true, &cancel)
        .unwrap();
    assert!(Arc::ptr_eq(
        obj.parts[0].texture.as_ref().unwrap(),
        dae.parts[0].texture.as_ref().unwrap()
    ));
    let plain = loader
        .load("materials/textured.obj", false, &cancel)
        .unwrap();
    assert!(plain
        .parts
        .iter()
        .all(|p| p.texture.is_none() && p.rgba.is_none()));
    assert!(plain.parts.iter().all(|p| p.uvs.is_some()));
}
#[test]
fn no_roots_remote_paths_escapes_and_malformed_uris_never_load_assets() {
    let cancel = AtomicBool::new(false);
    let root = fixture();
    let file = url::Url::from_file_path(root.join("meshes/wedge-ascii.stl"))
        .unwrap()
        .to_string();
    let mut empty = MeshAssetLoader::new(None, &LoadOptions::default(), &cancel).unwrap();
    for path in [
        &file,
        "meshes/wedge-ascii.stl",
        "package://models/meshes/wedge-ascii.stl",
    ] {
        assert!(empty.load(path, false, &cancel).is_err());
        assert_eq!(empty.stats().encoded_bytes, 0);
    }
    let mut loader = MeshAssetLoader::new(Some(&root), &LoadOptions::default(), &cancel).unwrap();
    for path in [
        "../outside.stl",
        "package://missing/a.obj",
        "https://example.com/a.stl",
        "file://remote/a.stl",
        "file:///tmp/outside.stl",
        "file://user:pass@localhost/a.obj",
        "file://localhost:1234/a.obj",
        "meshes\\wedge-ascii.stl",
        "mesh\0.stl",
    ] {
        assert!(loader.load(path, true, &cancel).is_err(), "{path}");
    }
    for suffix in ["?download=1", "#mesh"] {
        assert!(loader
            .load(&format!("{file}{suffix}"), true, &cancel)
            .is_err());
    }
    assert_eq!(loader.stats().encoded_bytes, 0);
}
#[test]
fn file_uri_decodes_spaces_and_unicode_and_reload_is_explicit() {
    let cancel = AtomicBool::new(false);
    let dir = Dir::new();
    let path = dir.0.join("mesh 空 间.stl");
    copy_stl(&dir.0, "mesh 空 间.stl");
    let uri = url::Url::from_file_path(&path).unwrap().to_string();
    assert!(uri.contains('%'));
    let mut loader = MeshAssetLoader::new(Some(&dir.0), &LoadOptions::default(), &cancel).unwrap();
    let asset = loader.load(&uri, false, &cancel).unwrap();
    let stats = loader.stats();
    std::fs::write(&path, b"bad").unwrap();
    assert!(Arc::ptr_eq(
        &asset,
        &loader.load(&uri, false, &cancel).unwrap()
    ));
    assert_eq!(loader.stats(), stats);
    let mut fresh = MeshAssetLoader::new(Some(&dir.0), &LoadOptions::default(), &cancel).unwrap();
    assert!(fresh.load(&uri, false, &cancel).is_err());
    assert_eq!(fresh.stats().resources, 0);
    assert_eq!(
        loader
            .load(&uri, false, &AtomicBool::new(true))
            .unwrap_err()
            .kind,
        RobotErrorKind::Cancelled
    );
}
#[cfg(windows)]
#[test]
fn windows_unc_and_device_resources_are_rejected_before_loading() {
    let cancel = AtomicBool::new(false);
    let root = fixture();
    let mut loader = MeshAssetLoader::new(Some(&root), &LoadOptions::default(), &cancel).unwrap();
    for resource in [
        r"\\remote\share\mesh.stl",
        r"\\?\UNC\remote\share\mesh.stl",
        r"\\.\C:\mesh.stl",
    ] {
        assert!(loader.load(resource, false, &cancel).is_err());
    }
    assert_eq!(loader.stats().encoded_bytes, 0);
}
#[cfg(unix)]
#[test]
fn symlink_sidecar_texture_escape_and_nonregular_assets_are_rejected() {
    use std::os::unix::{ffi::OsStrExt, fs::symlink};
    let cancel = AtomicBool::new(false);
    let dir = Dir::new();
    let outside = Dir::new();
    copy_stl(&outside.0, "outside.stl");
    symlink(outside.0.join("outside.stl"), dir.0.join("escape.stl")).unwrap();
    std::fs::create_dir(dir.0.join("folder.stl")).unwrap();
    let fifo = dir.0.join("pipe.stl");
    let c = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
    let mut loader = MeshAssetLoader::new(Some(&dir.0), &LoadOptions::default(), &cancel).unwrap();
    for path in ["escape.stl", "folder.stl", "pipe.stl"] {
        assert!(loader.load(path, false, &cancel).is_err());
    }
    std::fs::write(
        dir.0.join("mesh.obj"),
        b"mtllib escape.mtl\nv 0 0 0\nv 1 0 0\nv 0 1 0\nusemtl one\nf 1 2 3\n",
    )
    .unwrap();
    std::fs::write(outside.0.join("outside.mtl"), b"newmtl one\nKd 1 1 1\n").unwrap();
    symlink(outside.0.join("outside.mtl"), dir.0.join("escape.mtl")).unwrap();
    assert!(loader.load("mesh.obj", true, &cancel).is_err());
    std::fs::remove_file(dir.0.join("escape.mtl")).unwrap();
    std::fs::write(
        dir.0.join("escape.mtl"),
        b"newmtl one\nKd 1 1 1\nmap_Kd escape.png\n",
    )
    .unwrap();
    std::fs::copy(
        fixture().join("materials/textures/checker.png"),
        outside.0.join("outside.png"),
    )
    .unwrap();
    symlink(outside.0.join("outside.png"), dir.0.join("escape.png")).unwrap();
    std::fs::write(dir.0.join("mesh.obj"),b"mtllib escape.mtl\nv 0 0 0\nv 1 0 0\nv 0 1 0\nvt 0 0\nvt 1 0\nvt 0 1\nusemtl one\nf 1/1 2/2 3/3\n").unwrap();
    assert!(loader.load("mesh.obj", true, &cancel).is_err());
    assert_eq!(loader.stats().resources, 0);
    let plain = loader.load("mesh.obj", false, &cancel).unwrap();
    assert!(plain.parts.iter().all(|p| p.texture.is_none()));
}
#[test]
fn budgets_and_cancellation_preserve_previously_cached_assets() {
    let cancel = AtomicBool::new(false);
    let dir = Dir::new();
    copy_stl(&dir.0, "mesh.stl");
    let mut loader = MeshAssetLoader::new(Some(&dir.0), &LoadOptions::default(), &cancel).unwrap();
    let first = loader.load("mesh.stl", false, &cancel).unwrap();
    let before = loader.stats();
    loader.triangles = MAX_ROBOT_TRIANGLES;
    assert_eq!(
        loader.load("mesh.stl", true, &cancel).unwrap_err().kind,
        RobotErrorKind::Limit
    );
    assert_eq!(loader.stats().resources, before.resources);
    assert!(Arc::ptr_eq(
        &first,
        &loader.load("mesh.stl", false, &cancel).unwrap()
    ));
    assert!(
        loader.stats().encoded_bytes > before.encoded_bytes,
        "Failed decoding still consumes I/O budget"
    );
    assert_eq!(
        MeshAssetLoader::new(
            Some(&dir.0),
            &LoadOptions::default(),
            &AtomicBool::new(true)
        )
        .err()
        .unwrap()
        .kind,
        RobotErrorKind::Cancelled
    );
    let mut loader = MeshAssetLoader::new(Some(&dir.0), &LoadOptions::default(), &cancel).unwrap();
    for i in 0..MAX_MESH_RESOURCES {
        let name = format!("{i}.stl");
        copy_stl(&dir.0, &name);
        loader.load(&name, false, &cancel).unwrap();
    }
    copy_stl(&dir.0, "overflow.stl");
    let before = loader.stats();
    assert_eq!(
        loader
            .load("overflow.stl", false, &cancel)
            .unwrap_err()
            .kind,
        RobotErrorKind::Limit
    );
    assert_eq!(loader.stats(), before);
}

#[test]
fn actual_triangle_and_file_byte_boundaries_are_enforced() {
    let cancel = AtomicBool::new(false);
    let dir = Dir::new();
    let mut bytes = vec![0; 80];
    bytes.extend_from_slice(&(MAX_ROBOT_TRIANGLES as u32).to_le_bytes());
    let mut record = vec![0; 12];
    for point in [[0f32, 0., 0.], [1., 0., 0.], [0., 1., 0.]] {
        for n in point {
            record.extend_from_slice(&n.to_le_bytes());
        }
    }
    record.extend_from_slice(&[0; 2]);
    assert_eq!(record.len(), 50);
    for _ in 0..MAX_ROBOT_TRIANGLES {
        bytes.extend_from_slice(&record);
    }
    std::fs::write(dir.0.join("maximum.stl"), bytes).unwrap();
    copy_stl(&dir.0, "extra.stl");
    let mut loader = MeshAssetLoader::new(Some(&dir.0), &LoadOptions::default(), &cancel).unwrap();
    let maximum = loader.load("maximum.stl", false, &cancel).unwrap();
    assert_eq!(maximum.triangle_count, MAX_ROBOT_TRIANGLES);
    assert_eq!(
        loader.load("extra.stl", false, &cancel).unwrap_err().kind,
        RobotErrorKind::Limit
    );
    assert_eq!(loader.stats().resources, 1);
    assert_eq!(loader.stats().triangles, MAX_ROBOT_TRIANGLES);
    assert!(Arc::ptr_eq(
        &maximum,
        &loader.load("maximum.stl", false, &cancel).unwrap()
    ));
    let file = std::fs::File::create(dir.0.join("oversized.stl")).unwrap();
    file.set_len(MAX_MESH_BYTES as u64 + 1).unwrap();
    let mut loader = MeshAssetLoader::new(Some(&dir.0), &LoadOptions::default(), &cancel).unwrap();
    assert_eq!(
        loader
            .load("oversized.stl", false, &cancel)
            .unwrap_err()
            .kind,
        RobotErrorKind::Limit
    );
    assert_eq!(loader.stats().encoded_bytes, 0);
}
#[test]
fn narrow_file_root_does_not_reuse_a_broader_cached_material_resolution() {
    let cancel = AtomicBool::new(false);
    let dir = Dir::new();
    let narrow = dir.0.join("narrow");
    std::fs::create_dir(&narrow).unwrap();
    std::fs::copy(
        fixture().join("materials/textures/checker.png"),
        dir.0.join("outside-narrow.png"),
    )
    .unwrap();
    std::fs::write(narrow.join("mesh.obj"),b"mtllib material.mtl\nv 0 0 0\nv 1 0 0\nv 0 1 0\nvt 0 0\nvt 1 0\nvt 0 1\nusemtl one\nf 1/1 2/2 3/3\n").unwrap();
    std::fs::write(
        narrow.join("material.mtl"),
        b"newmtl one\nKd 1 1 1\nmap_Kd ../outside-narrow.png\n",
    )
    .unwrap();
    let options = LoadOptions {
        package_roots: BTreeMap::from([("tight".into(), narrow.clone())]),
    };
    let mut loader = MeshAssetLoader::new(Some(&dir.0), &options, &cancel).unwrap();
    let broad = loader.load("narrow/mesh.obj", true, &cancel).unwrap();
    assert!(broad.parts[0].texture.is_some());
    let uri = url::Url::from_file_path(narrow.join("mesh.obj"))
        .unwrap()
        .to_string();
    assert!(loader.load(&uri, true, &cancel).is_err());
    assert_eq!(loader.stats().resources, 1);
    assert!(Arc::ptr_eq(
        &broad,
        &loader.load("narrow/mesh.obj", true, &cancel).unwrap()
    ));
}

#[test]
fn exact_encoded_aggregate_budget_accepts_two_full_files_then_rejects_more_io() {
    let cancel = AtomicBool::new(false);
    let dir = Dir::new();
    let mut bytes = std::fs::read(fixture().join("meshes/wedge-ascii.stl")).unwrap();
    bytes.resize(MAX_MESH_BYTES, b' ');
    std::fs::write(dir.0.join("first.stl"), &bytes).unwrap();
    std::fs::write(dir.0.join("second.stl"), &bytes).unwrap();
    copy_stl(&dir.0, "extra.stl");
    let mut loader = MeshAssetLoader::new(Some(&dir.0), &LoadOptions::default(), &cancel).unwrap();
    let first = loader.load("first.stl", false, &cancel).unwrap();
    loader.load("second.stl", false, &cancel).unwrap();
    assert_eq!(loader.stats().encoded_bytes, MAX_ASSET_BYTES);
    assert_eq!(loader.stats().resources, 2);
    let before = loader.stats();
    assert_eq!(
        loader.load("extra.stl", false, &cancel).unwrap_err().kind,
        RobotErrorKind::Limit
    );
    assert_eq!(loader.stats(), before);
    assert!(Arc::ptr_eq(
        &first,
        &loader.load("first.stl", false, &cancel).unwrap()
    ));
    assert_eq!(loader.stats(), before);
}
