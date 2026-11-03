use super::*;
use crate::spatial::{marker_mesh, marker_mesh::Prepared};
impl Frame {
    /// Worker-only append of one validated mesh key. Pose/scale/bounds are
    /// already prepared; source/asset policy authorization belongs to caller.
    pub fn push_mesh(
        &mut self,
        mesh: &Prepared,
        camera: CameraPose,
        size: [u32; 2],
        deadline: Option<u64>,
        mut cancelled: impl FnMut() -> bool,
    ) -> Result<usize, String> {
        if self.mesh_count.saturating_add(mesh.triangle_count()) > marker_mesh::MAX_TRIANGLES
            || self.len().saturating_add(mesh.triangle_count()) > MAX_INSTANCES
        {
            return Err("Marker mesh instance budget".into());
        }
        let projection = projection::Projection::new(camera, size)?;
        let start = self.len();
        let old_projection = self.projection.clone();
        let old_mesh_count = self.mesh_count;
        let old_textures = self.textures.len();
        let result = (|| {
            for part in mesh.parts.iter() {
                if cancelled() {
                    return Err("Marker mesh projection cancelled".into());
                }
                let slot = if let Some(image) = &part.texture {
                    let i = if let Some(i) =
                        self.textures.iter().position(|old| Arc::ptr_eq(old, image))
                    {
                        i
                    } else {
                        if self.textures.len() >= marker_mesh::MAX_TEXTURES
                            || self
                                .textures
                                .iter()
                                .map(|t| t.rgba.len())
                                .sum::<usize>()
                                .saturating_add(image.rgba.len())
                                > marker_mesh::MAX_TEXTURE_BYTES
                        {
                            return Err("Marker mesh texture budget".into());
                        }
                        self.textures.push(image.clone());
                        self.textures.len() - 1
                    };
                    Some(i as u8)
                } else {
                    None
                };
                for (i, triangle) in part.triangles.iter().enumerate() {
                    if cancelled() {
                        return Err("Marker mesh projection cancelled".into());
                    }
                    let mut columns = [[0.; 4]; 4];
                    for (dest, p) in columns[..3].iter_mut().zip(triangle.0) {
                        *dest = projection.point(Point3(p))?;
                    }
                    let (kind, rgba, eye, ray) = if let Some(slot) = slot {
                        columns[3] = part.tint;
                        let uv = part.uvs.as_ref().ok_or("Textured mesh lost UVs")?[i];
                        (
                            Kind::TexturedTriangle(slot),
                            part.base,
                            [uv[0][0], uv[0][1], uv[1][0], uv[1][1]],
                            [uv[2][0], uv[2][1], 0., 0.],
                        )
                    } else {
                        columns[3] = std::array::from_fn(|j| {
                            (columns[0][j] as f64 / 3.
                                + columns[1][j] as f64 / 3.
                                + columns[2][j] as f64 / 3.) as f32
                        });
                        let c = marker_mesh::compose(part.base, part.tint, [1.; 4]);
                        (Kind::MeshTriangle, c, c, c)
                    };
                    self.commit(
                        kind,
                        ClipInstance {
                            columns,
                            rgba,
                            eye,
                            ray,
                            shape: [kind.tag(), 0, 0, 0],
                        },
                        camera,
                        projection.size,
                        deadline,
                    )?;
                }
            }
            if cancelled() {
                return Err("Marker mesh projection cancelled".into());
            }
            Ok(self.len() - start)
        })();
        if result.is_err() {
            self.instances.truncate(start);
            self.kinds.truncate(start);
            self.key_slots.truncate(start);
            self.mesh_count = old_mesh_count;
            self.projection = old_projection;
            self.textures.truncate(old_textures);
        }
        result
    }
}
