//! Explicit-root, bounded static assets for non-URDF consumers such as Marker.
use super::*;
pub const MAX_MESH_RESOURCES: usize = 2 * studio_protocol::marker::MAX_MARKERS;
/// Bound static material-part storage independently of triangle storage.
pub const MAX_MESH_PARTS: usize = MAX_ROBOT_TRIANGLES;
#[derive(Clone, Debug)]
pub struct MeshAssetPart {
    pub triangles: Arc<[Triangle]>,
    pub uvs: Option<Arc<[TriangleUv]>>,
    pub texture: Option<Arc<RobotTexture>>,
    /// None denotes the unmaterialed default; consumers apply their own color.
    pub rgba: Option<[f32; 4]>,
}
#[derive(Clone, Debug)]
pub struct MeshAsset {
    pub path: PathBuf,
    pub parts: Arc<[MeshAssetPart]>,
    pub triangle_count: usize,
    pub embedded_materials: bool,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MeshAssetStats {
    pub resources: usize,
    pub encoded_bytes: usize,
    pub triangles: usize,
    pub materials: usize,
    pub parts: usize,
    pub textures: usize,
    pub decoded_texture_bytes: usize,
}
/// A worker-owned asset job, scoped to one explicitly selected roots policy.
/// Recreate it for an explicit Reload/policy change; cache hits never reread a
/// resource. URI strings do not confer filesystem authority.
pub struct MeshAssetLoader {
    base: Option<PathBuf>,
    packages: BTreeMap<String, PathBuf>,
    cache: BTreeMap<(PathBuf, PathBuf, bool), Arc<MeshAsset>>,
    total: usize,
    textures: TextureCache,
    triangles: usize,
    materials: usize,
    parts: usize,
}
// Windows canonicalization adds a verbatim drive prefix, while file URIs decode
// to ordinary drive paths. Compare both forms before resolving within approved
// roots. UNC/device paths remain unavailable; no network root is opened here.
fn local_absolute_path(path: &Path) -> Result<PathBuf, RobotError> {
    #[cfg(windows)]
    {
        use std::path::{Component, Prefix};
        let mut components = path.components();
        let drive = match components.next() {
            Some(Component::Prefix(prefix)) => match prefix.kind() {
                Prefix::Disk(drive) | Prefix::VerbatimDisk(drive) => drive,
                _ => return Err(asset("Mesh path must use a local drive")),
            },
            _ => return Err(asset("Mesh path must use a local drive")),
        };
        let mut normalized = PathBuf::from(format!("{}:", char::from(drive)));
        normalized.extend(components.map(|component| component.as_os_str()));
        Ok(normalized)
    }
    #[cfg(not(windows))]
    Ok(path.to_owned())
}

impl MeshAssetLoader {
    pub fn new(
        asset_root: Option<&Path>,
        options: &LoadOptions,
        cancel: &AtomicBool,
    ) -> Result<Self, RobotError> {
        let (base, packages) = shared_mesh::roots(asset_root, options, cancel)?;
        Ok(Self {
            base,
            packages,
            cache: BTreeMap::new(),
            total: 0,
            textures: TextureCache::default(),
            triangles: 0,
            materials: 0,
            parts: 0,
        })
    }
    pub fn stats(&self) -> MeshAssetStats {
        MeshAssetStats {
            resources: self.cache.len(),
            encoded_bytes: self.total,
            triangles: self.triangles,
            materials: self.materials,
            parts: self.parts,
            textures: self.textures.images.len(),
            decoded_texture_bytes: self.textures.bytes,
        }
    }
    fn resource(&self, resource: &str) -> Result<(PathBuf, PathBuf), RobotError> {
        if resource.is_empty()
            || resource.len() > MAX_PATH_BYTES
            || (resource.contains('\\') && !(cfg!(windows) && Path::new(resource).is_absolute()))
            || resource.chars().any(char::is_control)
        {
            return Err(asset("Invalid local mesh resource"));
        }
        if self.base.is_none() && self.packages.is_empty() {
            return Err(asset("Select a local mesh asset root or package mapping"));
        }
        let absolute = if resource
            .get(..5)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("file:"))
        {
            let uri = url::Url::parse(resource).map_err(|_| asset("Invalid local file URI"))?;
            if uri.scheme() != "file"
                || uri
                    .host_str()
                    .is_some_and(|h| !h.is_empty() && h != "localhost")
                || !uri.username().is_empty()
                || uri.password().is_some()
                || uri.port().is_some()
                || uri.query().is_some()
                || uri.fragment().is_some()
            {
                return Err(asset(
                    "Mesh file URI must be local without credentials, port, query or fragment",
                ));
            }
            Some(
                uri.to_file_path()
                    .map_err(|_| asset("Mesh file URI must contain a local absolute path"))?,
            )
        } else if Path::new(resource).is_absolute() {
            Some(PathBuf::from(resource))
        } else {
            None
        };
        let (resolved, root) = if let Some(path) = absolute {
            let path = local_absolute_path(&path)?;
            path_bound(&path)?;
            if (!cfg!(windows) && path.to_string_lossy().contains('\\'))
                || path.to_string_lossy().chars().any(char::is_control)
            {
                return Err(asset("Invalid decoded mesh path"));
            }
            if !self
                .base
                .iter()
                .chain(self.packages.values())
                .any(|root| local_absolute_path(root).is_ok_and(|root| path.starts_with(root)))
            {
                return Err(asset(
                    "Mesh path lies outside selected canonical asset/package roots",
                ));
            }
            let resolved = path
                .canonicalize()
                .map_err(|e| asset(format!("Cannot resolve mesh: {e}")))?;
            path_bound(&resolved)?;
            let root = self
                .base
                .iter()
                .chain(self.packages.values())
                .filter(|root| resolved.starts_with(root))
                .max_by_key(|root| root.components().count())
                .ok_or_else(|| asset("Mesh file lies outside selected asset/package roots"))?
                .clone();
            (resolved, root)
        } else {
            let resolved = resolve(resource, self.base.as_deref(), &self.packages)?;
            let root = if let Some(package) = resource.strip_prefix("package://") {
                let (name, _) = package
                    .split_once('/')
                    .ok_or_else(|| asset("Invalid package mesh"))?;
                self.packages.get(name).cloned()
            } else {
                self.base.clone()
            }
            .ok_or_else(|| asset("Select a local mesh asset root"))?;
            (resolved, root)
        };
        if !resolved
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
                "Supported meshes: STL, static OBJ or Collada",
            ));
        }
        Ok((resolved, root))
    }
    pub fn load(
        &mut self,
        resource: &str,
        embedded_materials: bool,
        cancel: &AtomicBool,
    ) -> Result<Arc<MeshAsset>, RobotError> {
        stl::cancelled(cancel)?;
        let (resolved, root) = self.resource(resource)?;
        let key = (resolved.clone(), root.clone(), embedded_materials);
        if let Some(asset) = self.cache.get(&key) {
            return Ok(asset.clone());
        }
        if self.cache.len() >= MAX_MESH_RESOURCES {
            return Err(limit("Mesh job exceeds 512 resource/material variants"));
        }
        let shared_mesh::Source {
            resolved,
            root,
            decoded,
        } = shared_mesh::decode(resolved, root, &mut self.total, cancel)?;
        let count = decoded.parts.iter().try_fold(0usize, |total, part| {
            total
                .checked_add(part.triangles.len())
                .ok_or_else(|| limit("Mesh triangle count overflow"))
        })?;
        if count == 0 {
            return Err(asset("Mesh contains no triangles"));
        }
        let triangles = self
            .triangles
            .checked_add(count)
            .filter(|n| *n <= MAX_ROBOT_TRIANGLES)
            .ok_or_else(|| limit("Mesh job exceeds 50000 retained triangles"))?;
        let materials = self
            .materials
            .checked_add(decoded.material_count)
            .filter(|n| *n <= MAX_MATERIALS)
            .ok_or_else(|| limit("Mesh job exceeds 128 material definitions"))?;
        let parts = self
            .parts
            .checked_add(decoded.parts.len())
            .filter(|n| *n <= MAX_MESH_PARTS)
            .ok_or_else(|| limit("Mesh job exceeds 50000 material parts"))?;
        let mut output = Vec::with_capacity(decoded.parts.len());
        for part in decoded.parts {
            stl::cancelled(cancel)?;
            let texture = if embedded_materials {
                shared_mesh::embedded_texture(
                    &part,
                    &resolved,
                    &root,
                    &mut self.textures,
                    &mut self.total,
                    cancel,
                )?
            } else {
                None
            };
            output.push(MeshAssetPart {
                triangles: part.triangles.into(),
                uvs: part.uvs.map(Arc::from),
                texture,
                rgba: if embedded_materials { part.rgba } else { None },
            });
        }
        stl::cancelled(cancel)?;
        let asset = Arc::new(MeshAsset {
            path: resolved,
            parts: output.into(),
            triangle_count: count,
            embedded_materials,
        });
        self.cache.insert(key, asset.clone());
        self.triangles = triangles;
        self.materials = materials;
        self.parts = parts;
        Ok(asset)
    }
}
#[cfg(test)]
mod tests;
