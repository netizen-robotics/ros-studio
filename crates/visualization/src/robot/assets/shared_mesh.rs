//! Static mesh decoding shared by Robot descriptions and Marker asset jobs.
use super::*;
pub(super) struct Source {
    pub resolved: PathBuf,
    pub root: PathBuf,
    pub decoded: mesh::Decoded,
}
pub(super) fn roots(
    base: Option<&Path>,
    options: &LoadOptions,
    cancel: &AtomicBool,
) -> Result<(Option<PathBuf>, BTreeMap<String, PathBuf>), RobotError> {
    stl::cancelled(cancel)?;
    let base = base.map(canonical_directory).transpose()?;
    if options.package_roots.len() > MAX_PACKAGE_ROOTS {
        return Err(limit("At most 16 package roots are supported"));
    }
    let mut packages = BTreeMap::new();
    for (name, path) in &options.package_roots {
        stl::cancelled(cancel)?;
        if !package_name(name) {
            return Err(asset("Invalid package mapping name"));
        }
        packages.insert(name.clone(), canonical_directory(path)?);
    }
    Ok((base, packages))
}
pub(super) fn source(
    filename: &str,
    base: Option<&Path>,
    packages: &BTreeMap<String, PathBuf>,
    total: &mut usize,
    cancel: &AtomicBool,
) -> Result<Source, RobotError> {
    let resolved = resolve(filename, base, packages)?;
    let root = if let Some(package) = filename.strip_prefix("package://") {
        let (name, _) = package
            .split_once('/')
            .ok_or_else(|| asset("Invalid package mesh"))?;
        packages.get(name).map(PathBuf::as_path)
    } else {
        base
    }
    .ok_or_else(|| asset("Missing mesh root"))?;
    decode(resolved, root.to_owned(), total, cancel)
}
pub(super) fn decode(
    resolved: PathBuf,
    root: PathBuf,
    total: &mut usize,
    cancel: &AtomicBool,
) -> Result<Source, RobotError> {
    let bytes = read(&resolved, MAX_MESH_BYTES, total, cancel).map_err(|e| {
        if e.kind == RobotErrorKind::Io {
            asset(e.message)
        } else {
            e
        }
    })?;
    let extension = resolved.extension().and_then(|s| s.to_str()).unwrap_or("");
    let decoded = if extension.eq_ignore_ascii_case("dae") {
        collada::decode(&bytes, cancel)?
    } else if extension.eq_ignore_ascii_case("obj") {
        obj::decode(&bytes, cancel, |name| {
            stl::cancelled(cancel)?;
            let path = material_path(&resolved, &root, name)?;
            read(&path, MAX_MESH_BYTES, total, cancel)
        })?
    } else {
        mesh::Decoded {
            parts: vec![mesh::Part {
                triangles: stl::decode(&bytes, cancel)?,
                rgba: None,
                uvs: None,
                texture: None,
            }],
            material_count: 0,
        }
    };
    stl::cancelled(cancel)?;
    Ok(Source {
        resolved,
        root,
        decoded,
    })
}
pub(super) fn embedded_texture(
    part: &mesh::Part,
    resolved: &Path,
    root: &Path,
    textures: &mut TextureCache,
    total: &mut usize,
    cancel: &AtomicBool,
) -> Result<Option<Arc<RobotTexture>>, RobotError> {
    let Some(reference) = &part.texture else {
        return Ok(None);
    };
    if part.uvs.is_none() {
        return Err(asset(
            "Textured mesh requires UV coordinates for every corner",
        ));
    }
    let owner = reference
        .material_file
        .as_ref()
        .map(|name| material_path(resolved, root, name))
        .transpose()?
        .unwrap_or_else(|| resolved.to_owned());
    let path = texture_path(&owner, root, &reference.filename)?;
    Ok(Some(textures.load(path, total, cancel)?))
}
