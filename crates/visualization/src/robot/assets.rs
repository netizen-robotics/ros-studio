//! Local, bounded URDF/mesh loading. Geometry preparation must still validate the
//! expanded primitive+mesh triangle budget before model activation.
mod shared_mesh;
mod static_loader;
use super::{
    collada, mesh,
    model::*,
    obj,
    parser::{self, Geometry},
    stl,
};
pub use static_loader::{
    MeshAsset, MeshAssetLoader, MeshAssetPart, MeshAssetStats, MAX_MESH_PARTS, MAX_MESH_RESOURCES,
};
use std::{
    collections::BTreeMap,
    fs::OpenOptions,
    io::Read,
    path::{Component, Path, PathBuf},
    sync::{atomic::AtomicBool, Arc},
};

#[derive(Clone, Debug, Default)]
pub struct LoadOptions {
    pub package_roots: BTreeMap<String, PathBuf>,
}
fn asset(message: impl Into<String>) -> RobotError {
    RobotError::new(RobotErrorKind::Asset, message)
}
fn limit(message: &str) -> RobotError {
    RobotError::new(RobotErrorKind::Limit, message)
}
fn path_bound(path: &Path) -> Result<(), RobotError> {
    if path.as_os_str().len() > MAX_PATH_BYTES {
        Err(limit("Local path exceeds 4096 bytes"))
    } else {
        Ok(())
    }
}
fn read(
    path: &Path,
    max: usize,
    total: &mut usize,
    cancel: &AtomicBool,
) -> Result<Vec<u8>, RobotError> {
    stl::cancelled(cancel)?;
    path_bound(path)?;
    let before = std::fs::metadata(path).map_err(|e| {
        RobotError::new(
            RobotErrorKind::Io,
            format!("Cannot inspect {}: {e}", path.display()),
        )
    })?;
    if !before.is_file() {
        return Err(asset("Robot assets must be regular local files"));
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW);
    }
    let mut file = options.open(path).map_err(|e| {
        RobotError::new(
            RobotErrorKind::Io,
            format!("Cannot open {}: {e}", path.display()),
        )
    })?;
    let metadata = file.metadata().map_err(|e| asset(e.to_string()))?;
    if !metadata.is_file() {
        return Err(asset("Robot assets must be regular local files"));
    }
    if metadata.len() > max as u64 {
        return Err(limit("Robot file byte limit exceeded"));
    }
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 64 * 1024];
    loop {
        stl::cancelled(cancel)?;
        let count = file.read(&mut chunk).map_err(|e| asset(e.to_string()))?;
        if count == 0 {
            break;
        }
        if bytes.len().saturating_add(count) > max || total.saturating_add(count) > MAX_ASSET_BYTES
        {
            return Err(limit("Robot file/aggregate byte budget exceeded"));
        }
        bytes.extend_from_slice(&chunk[..count]);
        *total += count;
    }
    stl::cancelled(cancel)?;
    Ok(bytes)
}
fn canonical_directory(path: &Path) -> Result<PathBuf, RobotError> {
    path_bound(path)?;
    let result = path
        .canonicalize()
        .map_err(|e| asset(format!("Cannot resolve asset root {}: {e}", path.display())))?;
    path_bound(&result)?;
    if !result.is_dir() {
        return Err(asset("Asset root must be a directory"));
    }
    Ok(result)
}
fn package_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_NAME_BYTES
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}
fn resolve(
    filename: &str,
    base: Option<&Path>,
    packages: &BTreeMap<String, PathBuf>,
) -> Result<PathBuf, RobotError> {
    if filename.len() > MAX_PATH_BYTES
        || filename.is_empty()
        || filename.contains('\\')
        || filename.chars().any(char::is_control)
    {
        return Err(asset("Invalid mesh path"));
    }
    let (root, relative) = if let Some(package) = filename.strip_prefix("package://") {
        let (name, relative) = package
            .split_once('/')
            .ok_or_else(|| asset("Package mesh requires package name and path"))?;
        let root = packages
            .get(name)
            .ok_or_else(|| asset(format!("Unmapped package {name}")))?;
        (root.as_path(), relative)
    } else {
        (
            base.ok_or_else(|| {
                asset("Relative meshes in a topic description require an explicit asset root")
            })?,
            filename,
        )
    };
    if relative.contains(':') || relative.is_empty() {
        return Err(asset(
            "Only relative or mapped package mesh paths are supported",
        ));
    }
    let relative = Path::new(relative);
    if relative.is_absolute()
        || relative
            .components()
            .any(|c| !matches!(c, Component::Normal(_) | Component::CurDir))
    {
        return Err(asset(
            "Absolute mesh paths and parent traversal are forbidden",
        ));
    }
    if !relative
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| {
            ["stl", "obj", "dae"]
                .iter()
                .any(|known| e.eq_ignore_ascii_case(known))
        })
    {
        return Err(RobotError::new(
            RobotErrorKind::Unsupported,
            "Supported meshes: STL, static OBJ or Collada with diffuse colors and PNG/JPEG textures",
        ));
    }
    let joined = root.join(relative);
    path_bound(&joined)?;
    let resolved = joined
        .canonicalize()
        .map_err(|e| asset(format!("Cannot resolve mesh {}: {e}", joined.display())))?;
    path_bound(&resolved)?;
    if !resolved.starts_with(root) {
        return Err(asset("Mesh symlink escapes selected asset root"));
    }
    Ok(resolved)
}
/// Resolve an OBJ material library relative to the mesh, confined to the same
/// reviewed root. No sidecar gets an implicit working directory or extra root.
fn material_path(mesh: &Path, root: &Path, filename: &str) -> Result<PathBuf, RobotError> {
    if filename.is_empty()
        || filename.len() > MAX_PATH_BYTES
        || filename.contains([':', '\\'])
        || filename.chars().any(char::is_control)
    {
        return Err(asset("Invalid OBJ material path"));
    }
    let relative = Path::new(filename);
    if relative.is_absolute()
        || relative
            .components()
            .any(|c| !matches!(c, Component::Normal(_) | Component::CurDir))
    {
        return Err(asset(
            "OBJ sidecars require relative paths without parent traversal",
        ));
    }
    if !relative
        .extension()
        .and_then(|s| s.to_str())
        .is_some_and(|s| s.eq_ignore_ascii_case("mtl"))
    {
        return Err(asset("OBJ material library must be a local .mtl file"));
    }
    let joined = mesh
        .parent()
        .ok_or_else(|| asset("Mesh has no directory"))?
        .join(relative);
    path_bound(&joined)?;
    let path = joined
        .canonicalize()
        .map_err(|e| asset(format!("Cannot resolve OBJ material: {e}")))?;
    path_bound(&path)?;
    if !path.starts_with(root) {
        return Err(asset("Material symlink escapes selected asset root"));
    }
    Ok(path)
}
const MAX_TEXTURES: usize = 16;
const MAX_TEXTURE_BYTES: usize = 32 * 1024 * 1024;
#[derive(Default)]
struct TextureCache {
    images: BTreeMap<PathBuf, Arc<RobotTexture>>,
    bytes: usize,
}
fn texture_path(owner: &Path, root: &Path, name: &str) -> Result<PathBuf, RobotError> {
    if name.is_empty()
        || name.len() > MAX_PATH_BYTES
        || name.contains([':', '\\'])
        || name.chars().any(char::is_control)
    {
        return Err(asset("Texture requires a relative local PNG/JPEG path"));
    }
    let relative = Path::new(name);
    if relative.is_absolute()
        || relative.components().any(|c| {
            !matches!(
                c,
                Component::Normal(_) | Component::CurDir | Component::ParentDir
            )
        })
    {
        return Err(asset("Texture path must be relative to its material file"));
    }
    if !relative
        .extension()
        .and_then(|s| s.to_str())
        .is_some_and(|s| {
            ["png", "jpg", "jpeg"]
                .iter()
                .any(|e| s.eq_ignore_ascii_case(e))
        })
    {
        return Err(asset("Texture must be a PNG or JPEG file"));
    }
    let joined = owner
        .parent()
        .ok_or_else(|| asset("Material has no local directory"))?
        .join(relative);
    path_bound(&joined)?;
    let path = joined
        .canonicalize()
        .map_err(|e| asset(format!("Cannot resolve texture: {e}")))?;
    path_bound(&path)?;
    if !path.starts_with(root) {
        return Err(asset("Texture escapes the reviewed asset root"));
    }
    Ok(path)
}
impl TextureCache {
    fn load(
        &mut self,
        path: PathBuf,
        total: &mut usize,
        cancel: &AtomicBool,
    ) -> Result<Arc<RobotTexture>, RobotError> {
        stl::cancelled(cancel)?;
        if let Some(image) = self.images.get(&path) {
            return Ok(Arc::clone(image));
        }
        if self.images.len() == MAX_TEXTURES {
            return Err(limit("Robot exceeds 16 unique textures"));
        }
        let bytes = read(
            &path,
            studio_images::encoded::MAX_ENCODED_BYTES,
            total,
            cancel,
        )?;
        let image =
            studio_images::encoded::decode_raster(&bytes, MAX_TEXTURE_BYTES - self.bytes, cancel)
                .map_err(|e| {
                use studio_images::encoded::RasterError;
                RobotError::new(
                    match e {
                        RasterError::Cancelled => RobotErrorKind::Cancelled,
                        RasterError::Limit => RobotErrorKind::Limit,
                        RasterError::Unsupported => RobotErrorKind::Unsupported,
                        RasterError::Invalid => RobotErrorKind::Asset,
                    },
                    e.to_string(),
                )
            })?;
        stl::cancelled(cancel)?;
        self.bytes += image.rgba.len();
        let image = Arc::new(image);
        self.images.insert(path, Arc::clone(&image));
        Ok(image)
    }
}
fn description_texture_path(
    name: &str,
    base: Option<&Path>,
    packages: &BTreeMap<String, PathBuf>,
) -> Result<PathBuf, RobotError> {
    let (root, relative) = if let Some(package) = name.strip_prefix("package://") {
        let (name, relative) = package
            .split_once('/')
            .ok_or_else(|| asset("Package texture requires a path"))?;
        (
            packages
                .get(name)
                .map(PathBuf::as_path)
                .ok_or_else(|| asset("Unmapped texture package"))?,
            relative,
        )
    } else {
        (
            base.ok_or_else(|| {
                asset("Topic textures require an explicit asset root or package mapping")
            })?,
            name,
        )
    };
    texture_path(&root.join("description.urdf"), root, relative)
}
/// Loads a validated description and local assets. The caller must run bounded
/// geometry preparation on its asset worker before activating this model.
pub fn load_model(
    path: &Path,
    options: &LoadOptions,
    cancel: &AtomicBool,
) -> Result<RobotModel, RobotError> {
    stl::cancelled(cancel)?;
    path_bound(path)?;
    if !path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("urdf"))
    {
        return Err(RobotError::new(
            RobotErrorKind::Unsupported,
            "Select a local exported .urdf file",
        ));
    }
    let canonical = path.canonicalize().map_err(|e| {
        RobotError::new(
            RobotErrorKind::Io,
            format!("Cannot resolve URDF {}: {e}", path.display()),
        )
    })?;
    path_bound(&canonical)?;
    let base = canonical
        .parent()
        .ok_or_else(|| asset("URDF has no asset directory"))?;
    let mut total = 0;
    let bytes = read(&canonical, MAX_URDF_BYTES, &mut total, cancel)?;
    let source = std::str::from_utf8(&bytes)
        .map_err(|_| RobotError::new(RobotErrorKind::Xml, "URDF must be UTF-8"))?;
    load_model_from_text(source, Some(base), options, cancel)
}

/// Shared bounded parser/assets path. Topic sources have no implicit directory.
pub fn load_model_from_text(
    source: &str,
    asset_root: Option<&Path>,
    options: &LoadOptions,
    cancel: &AtomicBool,
) -> Result<RobotModel, RobotError> {
    stl::cancelled(cancel)?;
    if source.len() > MAX_URDF_BYTES {
        return Err(limit("Robot description exceeds 1 MiB"));
    }
    let (base, packages) = shared_mesh::roots(asset_root, options, cancel)?;
    let mut total = source.len();
    let description = parser::parse(source, cancel)?;
    let mut material_count = description.material_count;
    let mut links = Vec::with_capacity(description.links.len());
    let mut triangle_count = 0usize;
    let mut expanded_visuals = 0usize;
    let mut textures = TextureCache::default();
    for link in description.links {
        stl::cancelled(cancel)?;
        let mut visuals = Vec::with_capacity(link.visuals.len());
        for visual in link.visuals {
            stl::cancelled(cancel)?;
            let parts = match visual.geometry {
                Geometry::Ready(geometry) => {
                    let geometry = if let Some(reference) = &visual.texture {
                        let count =
                            crate::spatial::robot_geometry::primitive_texture_count(&geometry)?;
                        triangle_count = triangle_count
                            .checked_add(count)
                            .filter(|n| *n <= MAX_ROBOT_TRIANGLES)
                            .ok_or_else(|| limit("Textured primitives exceed triangle budget"))?;
                        let path = description_texture_path(
                            &reference.filename,
                            base.as_deref(),
                            &packages,
                        )?;
                        let image = textures.load(path, &mut total, cancel)?;
                        crate::spatial::robot_geometry::textured_primitive(geometry, image, cancel)?
                    } else {
                        geometry
                    };
                    vec![(geometry, visual.rgba)]
                }
                Geometry::Mesh { filename, scale } => {
                    let shared_mesh::Source {
                        resolved,
                        root,
                        decoded,
                    } = shared_mesh::source(
                        &filename,
                        base.as_deref(),
                        &packages,
                        &mut total,
                        cancel,
                    )?;
                    material_count = material_count
                        .checked_add(decoded.material_count)
                        .filter(|n| *n <= MAX_MATERIALS)
                        .ok_or_else(|| limit("Expanded robot exceeds 128 material definitions"))?;
                    let parts = decoded.parts;
                    let mut expanded = Vec::with_capacity(parts.len());
                    for part in parts {
                        let texture = if let Some(reference) = &visual.texture {
                            if part.uvs.is_none() {
                                return Err(asset(
                                    "URDF texture override requires mesh UV coordinates",
                                ));
                            }
                            let path = description_texture_path(
                                &reference.filename,
                                base.as_deref(),
                                &packages,
                            )?;
                            Some(textures.load(path, &mut total, cancel)?)
                        } else if visual.material_override {
                            None
                        } else {
                            shared_mesh::embedded_texture(
                                &part,
                                &resolved,
                                &root,
                                &mut textures,
                                &mut total,
                                cancel,
                            )?
                        };
                        triangle_count = triangle_count
                            .checked_add(part.triangles.len())
                            .filter(|n| *n <= MAX_ROBOT_TRIANGLES)
                            .ok_or_else(|| {
                                limit("Expanded mesh instances exceed 50000 triangles")
                            })?;
                        let rgba = if visual.material_override {
                            visual.rgba
                        } else {
                            part.rgba.unwrap_or(visual.rgba)
                        };
                        expanded.push((
                            if let Some(uvs) = part.uvs {
                                RobotGeometry::MeshWithUv {
                                    triangles: part.triangles.into(),
                                    uvs: uvs.into(),
                                    texture,
                                    scale,
                                }
                            } else {
                                RobotGeometry::Mesh {
                                    triangles: part.triangles.into(),
                                    scale,
                                }
                            },
                            rgba,
                        ));
                    }
                    expanded
                }
            };
            expanded_visuals = expanded_visuals
                .checked_add(parts.len())
                .filter(|n| *n <= MAX_VISUALS)
                .ok_or_else(|| limit("Expanded robot material parts exceed 256 visuals"))?;
            for (geometry, rgba) in parts {
                visuals.push(RobotVisual {
                    role: visual.role,
                    origin: visual.origin,
                    geometry,
                    rgba,
                });
            }
        }
        links.push(RobotLink {
            name: link.name,
            visuals,
        });
    }
    stl::cancelled(cancel)?;
    Ok(RobotModel {
        name: description.name,
        links,
        joints: description.joints,
        root_link: description.root_link,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    struct Dir(PathBuf);
    impl Dir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "studio-robot-assets-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    #[test]
    fn collision_roles_origins_file_topic_and_default_hidden_projection() {
        use crate::spatial::{
            robot_geometry::{prepare_robot, robot_vertices},
            CameraPose, RigidTransform,
        };
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/robot_visualizer/collision.urdf");
        let cancel = AtomicBool::new(false);
        let file = load_model(&path, &LoadOptions::default(), &cancel).unwrap();
        let topic = load_model_from_text(
            &std::fs::read_to_string(&path).unwrap(),
            path.parent(),
            &LoadOptions::default(),
            &cancel,
        )
        .unwrap();
        let prepared = prepare_robot(&file, &cancel).unwrap();
        let from_topic = prepare_robot(&topic, &cancel).unwrap();
        assert_eq!(prepared.link_names, ["base", "tip"]);
        assert_eq!(prepared.triangle_count, 26);
        for (a, b) in prepared.visuals.iter().zip(&from_topic.visuals) {
            assert_eq!(a.triangles, b.triangles);
            assert_eq!(a.rgba, b.rgba);
            assert_eq!(a.role, b.role);
            assert_eq!(a.visible, b.visible);
            assert_eq!(a.uvs, b.uvs);
        }
        assert_eq!(prepared.visuals.len(), 3);
        assert!(prepared.visuals[0].visible);
        assert!(prepared.visuals.iter().skip(1).all(|v| !v.visible
            && v.role == GeometryRole::Collision
            && v.texture.is_none()
            && v.rgba == [0.9, 0.55, 0.15, 1.]));
        let base_collision = &prepared.visuals[1];
        assert_eq!(base_collision.triangles[0].0[0], [-0.6, -0.6, 0.]);
        // The description's joint origin is metadata. Only external TF moves tip.
        let tip = &prepared.visuals[2];
        assert_eq!(tip.triangles[0].0[0], [0., 0., 1.]);
        let camera = CameraPose::orbit(0., 0., 10., [0., 0., 0.]);
        let poses = [Some(RigidTransform::identity()); 2];
        assert_eq!(
            robot_vertices(&prepared, &poses, camera, [128, 128])
                .unwrap()
                .len(),
            36
        );
        let mut collision = prepared.clone();
        for v in &mut collision.visuals {
            v.visible = v.role == GeometryRole::Collision;
        }
        assert_eq!(
            robot_vertices(&collision, &poses, camera, [128, 128])
                .unwrap()
                .len(),
            42
        );
        assert_eq!(
            robot_vertices(&collision, &[poses[0], None], camera, [128, 128])
                .unwrap()
                .len(),
            36
        );
    }
    #[test]
    fn hidden_collision_assets_fail_review_and_share_global_part_and_triangle_limits() {
        let cancel = AtomicBool::new(false);
        let load = |body: &str| {
            load_model_from_text(
                &format!("<robot name=\"r\"><link name=\"base\">{body}</link></robot>"),
                None,
                &LoadOptions::default(),
                &cancel,
            )
        };
        for body in [
            "<collision/>",
            "<collision><geometry><box size=\"0 1 1\"/></geometry></collision>",
            "<collision><geometry><box size=\"1 1 1\"/></geometry><material><color rgba=\"1 0 0 1\"/></material></collision>",
            "<collision><geometry><mesh filename=\"missing.stl\"/></geometry></collision>",
        ] { assert!(load(body).is_err(),"{body}"); }
        let sphere = "<collision><geometry><sphere radius=\"1\"/></geometry></collision>";
        let model = load(&sphere.repeat(53)).unwrap();
        assert!(crate::spatial::robot_geometry::prepare_robot(&model, &cancel).is_err());
        let tiny = "<collision><geometry><box size=\"1 1 1\"/></geometry></collision>";
        assert_eq!(
            load(&tiny.repeat(257)).unwrap_err().kind,
            RobotErrorKind::Limit
        );
        let cancelled = AtomicBool::new(true);
        assert_eq!(
            load_model_from_text("<robot/>", None, &LoadOptions::default(), &cancelled)
                .unwrap_err()
                .kind,
            RobotErrorKind::Cancelled
        );
    }
    #[test]
    fn urdf_named_texture_overrides_mesh_materials_using_description_roots() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/robot_visualizer/collada-textured.urdf");
        let base = path.parent().unwrap();
        let cancel = AtomicBool::new(false);
        let source=std::fs::read_to_string(&path).unwrap().replace("</geometry>","</geometry><material name=\"skin\"/>").replace("</robot>","<material name=\"skin\"><texture filename=\"materials/textures/checker.png\"/></material></robot>");
        let model =
            load_model_from_text(&source, Some(base), &LoadOptions::default(), &cancel).unwrap();
        let image = |v: &RobotVisual| match &v.geometry {
            RobotGeometry::MeshWithUv {
                texture: Some(image),
                ..
            } => Arc::clone(image),
            _ => panic!("missing override"),
        };
        assert!(Arc::ptr_eq(
            &image(&model.links[0].visuals[0]),
            &image(&model.links[0].visuals[1])
        ));
        assert!(model.links[0].visuals.iter().all(|v| v.rgba == [1.; 4]));
        let mapped = source
            .replace(
                "materials/textured.dae",
                "package://models/materials/textured.dae",
            )
            .replace(
                "materials/textures/checker.png",
                "package://images/checker.png",
            );
        let options = LoadOptions {
            package_roots: BTreeMap::from([
                ("models".into(), base.into()),
                ("images".into(), base.join("materials/textures")),
            ]),
        };
        assert_eq!(
            image(
                &load_model_from_text(&mapped, None, &options, &cancel)
                    .unwrap()
                    .links[0]
                    .visuals[0]
            ),
            image(&model.links[0].visuals[0])
        );
        assert!(load_model_from_text(
            &source.replace("checker.png", "missing.png"),
            Some(base),
            &LoadOptions::default(),
            &cancel
        )
        .is_err());
        let primitive="<robot name=\"r\"><link name=\"base\"><visual><geometry><box size=\"1 1 1\"/></geometry><material><texture filename=\"materials/textures/checker.png\"/></material></visual></link></robot>";
        let model =
            load_model_from_text(primitive, Some(base), &LoadOptions::default(), &cancel).unwrap();
        assert_eq!(
            crate::spatial::robot_geometry::prepare_robot(&model, &cancel)
                .unwrap()
                .triangle_count,
            12
        );
    }
    #[test]
    fn collada_texture_file_topic_and_package_resources_share_cache_path() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/robot_visualizer/collada-textured.urdf");
        let source = std::fs::read_to_string(&path).unwrap();
        let cancel = AtomicBool::new(false);
        let file = load_model(&path, &LoadOptions::default(), &cancel).unwrap();
        let topic =
            load_model_from_text(&source, path.parent(), &LoadOptions::default(), &cancel).unwrap();
        let image = |m: &RobotModel| match &m.links[0].visuals[0].geometry {
            RobotGeometry::MeshWithUv {
                texture: Some(image),
                ..
            } => Arc::clone(image),
            _ => panic!("missing Collada texture"),
        };
        assert_eq!(image(&file), image(&topic));
        assert_eq!(image(&file).rgba.len(), 16);
        let mapped = source.replace(
            "materials/textured.dae",
            "package://demo/materials/textured.dae",
        );
        let options = LoadOptions {
            package_roots: BTreeMap::from([("demo".into(), path.parent().unwrap().into())]),
        };
        assert_eq!(
            image(&load_model_from_text(&mapped, None, &options, &cancel).unwrap()),
            image(&file)
        );
        assert!(crate::spatial::robot_geometry::prepare_robot(&file, &cancel).is_ok());
    }
    #[test]
    fn obj_texture_resolves_from_mtl_caches_instances_and_override_clears_image() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/robot_visualizer/textured.urdf");
        let cancel = AtomicBool::new(false);
        let model = load_model(&path, &LoadOptions::default(), &cancel).unwrap();
        let image = |v: &RobotVisual| match &v.geometry {
            RobotGeometry::MeshWithUv {
                texture: Some(texture),
                ..
            } => Arc::clone(texture),
            _ => panic!("missing texture"),
        };
        let a = image(&model.links[0].visuals[0]);
        let b = image(&model.links[0].visuals[1]);
        assert!(Arc::ptr_eq(&a, &b));
        assert_eq!((a.width, a.height), (2, 2));
        assert_eq!(&a.rgba[..4], &[255, 0, 0, 255]);
        assert!(crate::spatial::robot_geometry::prepare_robot(&model, &cancel).is_ok());
        let source = std::fs::read_to_string(&path).unwrap().replace(
            "</geometry>",
            "</geometry><material><color rgba=\"0 1 0 1\"/></material>",
        );
        let override_model =
            load_model_from_text(&source, path.parent(), &LoadOptions::default(), &cancel).unwrap();
        assert!(override_model.links[0]
            .visuals
            .iter()
            .all(|v| matches!(v.geometry, RobotGeometry::MeshWithUv { texture: None, .. })));
        assert!(crate::spatial::robot_geometry::prepare_robot(&override_model, &cancel).is_ok());
    }
    #[test]
    fn texture_cache_limits_cancellation_and_root_confinement() {
        let dir = Dir::new();
        let root = dir.0.canonicalize().unwrap();
        std::fs::create_dir(root.join("materials")).unwrap();
        let bytes =
            include_bytes!("../../../../fixtures/robot_visualizer/materials/textures/checker.png");
        let owner = root.join("materials/model.mtl");
        let mut cache = TextureCache::default();
        let mut total = 0;
        let cancel = AtomicBool::new(false);
        for i in 0..MAX_TEXTURES {
            std::fs::write(root.join(format!("{i}.png")), bytes).unwrap();
            let path = texture_path(&owner, &root, &format!("../{i}.png")).unwrap();
            cache.load(path, &mut total, &cancel).unwrap();
        }
        assert_eq!(cache.bytes, MAX_TEXTURES * 16);
        let before = total;
        let path = texture_path(&owner, &root, "../0.png").unwrap();
        let a = cache.load(path.clone(), &mut total, &cancel).unwrap();
        let b = cache.load(path.clone(), &mut total, &cancel).unwrap();
        assert!(Arc::ptr_eq(&a, &b));
        assert_eq!(total, before);
        assert_eq!(
            cache
                .load(path, &mut total, &AtomicBool::new(true))
                .unwrap_err()
                .kind,
            RobotErrorKind::Cancelled
        );
        std::fs::write(root.join("overflow.png"), bytes).unwrap();
        let path = root.join("overflow.png");
        assert_eq!(
            cache
                .load(path.clone(), &mut total, &cancel)
                .unwrap_err()
                .kind,
            RobotErrorKind::Limit
        );
        let mut full = TextureCache {
            images: BTreeMap::new(),
            bytes: MAX_TEXTURE_BYTES,
        };
        assert_eq!(
            full.load(path, &mut total, &cancel).unwrap_err().kind,
            RobotErrorKind::Limit
        );
        assert!(full.images.is_empty());
        for bad in [
            "/tmp/a.png",
            "https://example/a.png",
            "file:///a.png",
            "../../escape.png",
            "../0.gif",
        ] {
            assert!(texture_path(&owner, &root, bad).is_err());
        }
        #[cfg(unix)]
        {
            let outside = Dir::new();
            std::fs::write(outside.0.join("outside.png"), bytes).unwrap();
            std::os::unix::fs::symlink(outside.0.join("outside.png"), root.join("escape.png"))
                .unwrap();
            assert!(texture_path(&owner, &root, "../escape.png").is_err());
        }
    }
    #[test]
    fn collada_file_topic_package_roots_and_urdf_override_share_preparation() {
        use crate::spatial::robot_geometry::prepare_robot;
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/robot_visualizer/collada.urdf");
        let source = std::fs::read_to_string(&path).unwrap();
        let cancel = AtomicBool::new(false);
        let a = load_model(&path, &LoadOptions::default(), &cancel).unwrap();
        let b =
            load_model_from_text(&source, path.parent(), &LoadOptions::default(), &cancel).unwrap();
        let a = prepare_robot(&a, &cancel).unwrap();
        let b = prepare_robot(&b, &cancel).unwrap();
        assert_eq!(a.triangle_count, 4);
        assert_eq!(a.link_names, vec!["base"]);
        assert!(a.visuals.iter().all(|v| v.uvs.is_some()));
        for (a, b) in a.visuals.iter().zip(&b.visuals) {
            assert_eq!(a.triangles, b.triangles);
            assert_eq!(a.uvs, b.uvs);
            assert_eq!(a.rgba, b.rgba);
        }
        assert_eq!(a.visuals[0].triangles[0].0[0], [1., 0., 0.55]);
        assert_eq!(a.visuals[1].rgba, [0.05, 0.75, 0.65, 1.]);
        let mapped = source.replace(
            "materials/panels.dae",
            "package://demo/materials/panels.dae",
        );
        let options = LoadOptions {
            package_roots: BTreeMap::from([("demo".into(), path.parent().unwrap().into())]),
        };
        let mapped = load_model_from_text(&mapped, None, &options, &cancel).unwrap();
        assert_eq!(
            prepare_robot(&mapped, &cancel).unwrap().visuals[0].triangles,
            a.visuals[0].triangles
        );
        let overridden = source.replace(
            "</geometry>",
            "</geometry><material><color rgba=\"0 0 1 1\"/></material>",
        );
        let model =
            load_model_from_text(&overridden, path.parent(), &LoadOptions::default(), &cancel)
                .unwrap();
        assert!(prepare_robot(&model, &cancel)
            .unwrap()
            .visuals
            .iter()
            .all(|v| v.rgba == [0., 0., 1., 1.]));
    }
    #[test]
    fn obj_file_topic_materials_override_and_transform_share_preparation() {
        use crate::spatial::robot_geometry::prepare_robot;
        let path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/robot_visualizer/obj.urdf");
        let cancel = AtomicBool::new(false);
        let source = std::fs::read_to_string(&path).unwrap();
        let options = LoadOptions::default();
        let file = load_model(&path, &options, &cancel).unwrap();
        let topic = load_model_from_text(&source, path.parent(), &options, &cancel).unwrap();
        let a = prepare_robot(&file, &cancel).unwrap();
        let b = prepare_robot(&topic, &cancel).unwrap();
        assert_eq!(a.triangle_count, 4);
        assert_eq!(a.visuals.len(), 2);
        for (a, b) in a.visuals.iter().zip(&b.visuals) {
            assert_eq!(a.triangles, b.triangles);
            assert_eq!(a.rgba, b.rgba);
        }
        assert_eq!(a.visuals[0].rgba, [0.95, 0.2, 0.12, 1.]);
        assert_eq!(a.visuals[1].rgba, [0.05, 0.75, 0.65, 1.]);
        assert_eq!(a.visuals[0].triangles[0].0[0], [-0.8, -0.3, 0.3]);
        let overridden = source.replace(
            "</geometry>",
            "</geometry><material><color rgba=\"0.1 0.2 0.9 1\"/></material>",
        );
        let model = load_model_from_text(&overridden, path.parent(), &options, &cancel).unwrap();
        let prepared = prepare_robot(&model, &cancel).unwrap();
        assert!(prepared
            .visuals
            .iter()
            .all(|p| p.rgba == [0.1, 0.2, 0.9, 1.]));
        assert_eq!(prepared.triangle_count, a.triangle_count);
        let named = (0..MAX_MATERIALS - 1)
            .map(|i| format!("<material name=\"m{i}\"><color rgba=\"1 0 0 1\"/></material>"))
            .collect::<String>();
        let over_materials = source.replace("</robot>", &format!("{named}</robot>"));
        assert_eq!(
            load_model_from_text(&over_materials, path.parent(), &options, &cancel)
                .unwrap_err()
                .kind,
            RobotErrorKind::Limit
        );
        assert!(load_model_from_text(&source, None, &options, &cancel).is_err());
    }
    #[test]
    fn obj_sidecars_resolve_from_mesh_directory_with_same_root_and_missing_is_error() {
        let dir = Dir::new();
        std::fs::create_dir(dir.0.join("mesh")).unwrap();
        std::fs::write(
            dir.0.join("mesh/p.obj"),
            b"mtllib p.mtl\nv 0 0 0\nv 1 0 0\nv 0 1 0\nusemtl a\nf 1 2 3",
        )
        .unwrap();
        let source = r#"<robot name="r"><link name="base"><visual><geometry><mesh filename="mesh/p.obj"/></geometry></visual></link></robot>"#;
        let options = LoadOptions::default();
        let cancel = AtomicBool::new(false);
        assert!(load_model_from_text(source, Some(&dir.0), &options, &cancel).is_err());
        std::fs::write(dir.0.join("mesh/p.mtl"), b"newmtl a\nKd 0 1 0").unwrap();
        assert!(load_model_from_text(source, Some(&dir.0), &options, &cancel).is_ok());
        let root = dir.0.canonicalize().unwrap();
        let mesh = root.join("mesh/p.obj");
        for sidecar in ["../p.mtl", "/p.mtl", "https://example/p.mtl", "p.png"] {
            assert!(material_path(&mesh, &root, sidecar).is_err());
        }
        #[cfg(unix)]
        {
            let outside = Dir::new();
            std::fs::write(outside.0.join("p.mtl"), b"newmtl a\nKd 1 0 0").unwrap();
            std::fs::remove_file(root.join("mesh/p.mtl")).unwrap();
            std::os::unix::fs::symlink(outside.0.join("p.mtl"), root.join("mesh/p.mtl")).unwrap();
            assert!(load_model_from_text(source, Some(&dir.0), &options, &cancel).is_err());
        }
    }
    #[test]
    fn file_and_topic_share_model_loading_and_topic_assets_require_explicit_roots() {
        let dir = Dir::new();
        let source = r#"<robot name="r"><link name="base"><visual><geometry><mesh filename="part.stl"/></geometry></visual></link></robot>"#;
        std::fs::write(dir.0.join("robot.urdf"), source).unwrap();
        std::fs::write(dir.0.join("part.stl"), b"solid p\nfacet normal 0 0 1\nouter loop\nvertex 0 0 0\nvertex 1 0 0\nvertex 0 1 0\nendloop\nendfacet\nendsolid p\n").unwrap();
        let cancel = AtomicBool::new(false);
        let options = LoadOptions::default();
        let file = load_model(&dir.0.join("robot.urdf"), &options, &cancel).unwrap();
        let topic = load_model_from_text(source, Some(&dir.0), &options, &cancel).unwrap();
        assert_eq!(format!("{file:?}"), format!("{topic:?}"));
        assert!(load_model_from_text(source, None, &options, &cancel)
            .unwrap_err()
            .message
            .contains("explicit asset root"));
        let mapped = source.replace("part.stl", "package://demo/part.stl");
        let options = LoadOptions {
            package_roots: BTreeMap::from([("demo".into(), dir.0.clone())]),
        };
        assert!(load_model_from_text(&mapped, None, &options, &cancel).is_ok());
        assert!(
            load_model_from_text(source, Some(&dir.0.join("missing")), &options, &cancel).is_err()
        );
        assert_eq!(
            load_model_from_text(source, None, &options, &AtomicBool::new(true))
                .unwrap_err()
                .kind,
            RobotErrorKind::Cancelled
        );
        assert_eq!(
            load_model_from_text("<invalid", None, &LoadOptions::default(), &cancel)
                .unwrap_err()
                .kind,
            RobotErrorKind::Xml
        );
        assert_eq!(
            load_model_from_text(
                &" ".repeat(MAX_URDF_BYTES + 1),
                None,
                &LoadOptions::default(),
                &cancel
            )
            .unwrap_err()
            .kind,
            RobotErrorKind::Limit
        );
    }
    #[test]
    fn mapped_and_relative_paths_and_parent_traversal() {
        let dir = Dir::new();
        std::fs::write(dir.0.join("part.stl"), b"mesh").unwrap();
        let base = dir.0.canonicalize().unwrap();
        let packages = BTreeMap::from([("demo".into(), base.clone())]);
        assert_eq!(
            resolve("part.stl", Some(&base), &packages).unwrap(),
            resolve("package://demo/part.stl", Some(&base), &packages).unwrap()
        );
        for path in [
            "../part.stl",
            "package://demo/../part.stl",
            "/part.stl",
            "file:///part.stl",
            "https://example/part.stl",
            "package://missing/part.stl",
        ] {
            assert!(resolve(path, Some(&base), &packages).is_err());
        }
    }
    #[cfg(unix)]
    #[test]
    fn canonical_symlink_escape_is_rejected() {
        let root = Dir::new();
        let outside = Dir::new();
        std::fs::write(outside.0.join("outside.stl"), b"mesh").unwrap();
        std::os::unix::fs::symlink(outside.0.join("outside.stl"), root.0.join("escape.stl"))
            .unwrap();
        assert!(resolve(
            "escape.stl",
            Some(&root.0.canonicalize().unwrap()),
            &BTreeMap::new()
        )
        .is_err());
    }
    #[cfg(unix)]
    #[test]
    fn fifo_asset_is_rejected_before_open() {
        use std::os::unix::ffi::OsStrExt;
        let dir = Dir::new();
        let path = dir.0.join("pipe.stl");
        let cpath = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
        // No writer is opened: a blocking FIFO open would hang this regression.
        assert_eq!(unsafe { libc::mkfifo(cpath.as_ptr(), 0o600) }, 0);
        assert_eq!(
            read(&path, MAX_MESH_BYTES, &mut 0, &AtomicBool::new(false))
                .unwrap_err()
                .kind,
            RobotErrorKind::Asset
        );
    }
    #[test]
    fn cancellation_precedes_io_and_bad_tree_fails() {
        assert_eq!(
            load_model(
                Path::new("missing.urdf"),
                &LoadOptions::default(),
                &AtomicBool::new(true)
            )
            .unwrap_err()
            .kind,
            RobotErrorKind::Cancelled
        );
        let dir = Dir::new();
        let path = dir.0.join("robot.urdf");
        std::fs::write(
            &path,
            r#"<robot name="r"><link name="a"/><link name="b"/></robot>"#,
        )
        .unwrap();
        assert_eq!(
            load_model(&path, &LoadOptions::default(), &AtomicBool::new(false))
                .unwrap_err()
                .kind,
            RobotErrorKind::InvalidModel
        );
    }
}
