//! Pure worker preparation of explicitly approved mesh assets. No asset IO or
//! source/TF authorization is inferred here; the caller owns those epochs.
use super::{map_geometry::bounded_point, marker_geometry::Body, Point3};
use crate::robot::{MeshAsset, RobotTexture, Triangle, TriangleUv};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
pub const MAX_TRIANGLES: usize = crate::robot::MAX_ROBOT_TRIANGLES;
pub const MAX_TEXTURES: usize = 16;
pub const MAX_TEXTURE_BYTES: usize = 32 * 1024 * 1024;
#[derive(Clone, Debug)]
pub(crate) struct Part {
    pub triangles: Arc<[Triangle]>,
    pub uvs: Option<Arc<[TriangleUv]>>,
    pub texture: Option<Arc<RobotTexture>>,
    pub base: [f32; 4],
    pub tint: [f32; 4],
}
#[derive(Clone, Debug)]
pub struct Prepared {
    pub(crate) parts: Arc<[Part]>,
    triangles: usize,
    bounds: [[f64; 3]; 2],
}
impl Prepared {
    pub fn triangle_count(&self) -> usize {
        self.triangles
    }
    pub fn bounds(&self) -> [[f64; 3]; 2] {
        self.bounds
    }
}
fn check(cancel: &AtomicBool) -> Result<(), String> {
    if cancel.load(Ordering::Acquire) {
        Err("Marker mesh preparation cancelled".into())
    } else {
        Ok(())
    }
}
fn color(c: [f32; 4]) -> bool {
    c.iter().all(|v| v.is_finite() && (0.0..=1.0).contains(v))
}
/// Source-over base followed by the RViz-style flat color overlay. This pure
/// form is also the analytic reference for the textured fragment path.
pub fn compose(base: [f32; 4], tint: [f32; 4], sample: [f32; 4]) -> [f32; 4] {
    let a = base[3] * sample[3];
    let t = tint[3];
    let alpha = a + (1. - a) * t;
    if alpha == 0. {
        return [0.; 4];
    }
    [
        (base[0] * sample[0] * a * (1. - t) + tint[0] * t) / alpha,
        (base[1] * sample[1] * a * (1. - t) + tint[1] * t) / alpha,
        (base[2] * sample[2] * a * (1. - t) + tint[2] * t) / alpha,
        alpha,
    ]
}
/// Preflight repeated instances before allocating transformed geometry. Key
/// errors remain at their stable input index; cancellation/aggregate errors
/// discard all partial output. Assets must belong to the caller's active policy.
pub fn prepare(
    inputs: &[(&Body, &MeshAsset)],
    cancel: &AtomicBool,
) -> Result<Vec<Result<Prepared, String>>, String> {
    check(cancel)?;
    if inputs.len() > 2 * studio_protocol::marker::MAX_MARKERS {
        return Err("Marker mesh entry limit".into());
    }
    let mut total = 0usize;
    for (_, asset) in inputs {
        check(cancel)?;
        total = total
            .checked_add(asset.triangle_count)
            .filter(|n| *n <= MAX_TRIANGLES)
            .ok_or("Marker expanded mesh triangle limit")?;
    }
    let mut out = Vec::with_capacity(inputs.len());
    for (body, asset) in inputs {
        check(cancel)?;
        out.push(one(body, asset, cancel));
        check(cancel)?;
    }
    Ok(out)
}
fn one(body: &Body, asset: &MeshAsset, cancel: &AtomicBool) -> Result<Prepared, String> {
    let Body::Mesh {
        pose,
        scale,
        embedded,
        rgba,
        ..
    } = body
    else {
        return Err("Expected prepared Marker mesh".into());
    };
    if asset.embedded_materials != *embedded
        || !color(*rgba)
        || scale
            .iter()
            .any(|v| !v.is_finite() || *v <= 0. || *v > 1_000_000.)
    {
        return Err("Invalid Marker mesh style/asset mode".into());
    }
    let actual = asset.parts.iter().try_fold(0usize, |n, p| {
        n.checked_add(p.triangles.len())
            .ok_or("Mesh count overflow")
    })?;
    if actual == 0
        || actual != asset.triangle_count
        || actual > MAX_TRIANGLES
        || asset.parts.len() > crate::robot::MAX_MESH_PARTS
    {
        return Err("Invalid loaded mesh triangle/part count".into());
    }
    let mut images: Vec<Arc<RobotTexture>> = Vec::new();
    let mut bytes = 0usize;
    let mut parts = Vec::with_capacity(asset.parts.len());
    let mut bounds = [[f64::INFINITY; 3], [f64::NEG_INFINITY; 3]];
    for part in asset.parts.iter() {
        check(cancel)?;
        if let Some(uvs) = &part.uvs {
            super::robot_geometry::validate_uvs(uvs, part.triangles.len())
                .map_err(|e| e.to_string())?;
        }
        let texture = if *embedded {
            part.texture.clone()
        } else {
            None
        };
        if let Some(image) = &texture {
            super::robot_geometry::validate_texture(image).map_err(|e| e.to_string())?;
            if part.uvs.is_none() {
                return Err("Textured Marker mesh requires UVs".into());
            }
            if !images.iter().any(|old| Arc::ptr_eq(old, image)) {
                images.push(image.clone());
                bytes += image.rgba.len();
            }
            if images.len() > MAX_TEXTURES || bytes > MAX_TEXTURE_BYTES {
                return Err("Marker mesh texture budget".into());
            }
        }
        let imported = part.rgba.unwrap_or([1.; 4]);
        if !color(imported) {
            return Err("Invalid mesh material color".into());
        }
        let (base, tint) = if !embedded {
            (*rgba, [0.; 4])
        } else if *rgba == [0.; 4] {
            (imported, [0.; 4])
        } else if part.rgba.is_some() || texture.is_some() {
            (
                [imported[0], imported[1], imported[2], rgba[3]],
                [rgba[0], rgba[1], rgba[2], rgba[3].min(0.5)],
            )
        } else {
            (*rgba, [0.; 4])
        };
        let mut triangles = Vec::with_capacity(part.triangles.len());
        for triangle in part.triangles.iter() {
            check(cancel)?;
            if !super::robot_geometry::triangle_valid(triangle) {
                return Err("Invalid loaded mesh triangle".into());
            }
            let mut world = [[0.; 3]; 3];
            for (dest, p) in world.iter_mut().zip(triangle.0) {
                *dest = pose
                    .apply(Point3(std::array::from_fn(|i| p[i] * scale[i])))
                    .0;
                if !bounded_point(Point3(*dest)) {
                    return Err("Marker loaded mesh exceeds world bounds".into());
                }
                for (i, value) in dest.iter().enumerate() {
                    bounds[0][i] = bounds[0][i].min(*value);
                    bounds[1][i] = bounds[1][i].max(*value);
                }
            }
            let triangle = Triangle(world);
            if !super::robot_geometry::triangle_valid(&triangle) {
                return Err("Degenerate transformed Marker mesh".into());
            }
            triangles.push(triangle);
        }
        parts.push(Part {
            triangles: triangles.into(),
            uvs: part.uvs.clone(),
            texture,
            base,
            tint,
        });
    }
    check(cancel)?;
    Ok(Prepared {
        parts: parts.into(),
        triangles: actual,
        bounds,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        robot::{LoadOptions, MeshAssetLoader, MeshAssetPart},
        spatial::RigidTransform,
    };
    fn body(embedded: bool, rgba: [f32; 4]) -> Body {
        Body::Mesh {
            pose: RigidTransform::identity(),
            scale: [1.; 3],
            resource: "fixture.obj".into(),
            embedded,
            rgba,
        }
    }
    fn asset(embedded: bool, n: usize) -> MeshAsset {
        MeshAsset {
            path: "fixture.obj".into(),
            parts: vec![MeshAssetPart {
                triangles: vec![Triangle([[0., 0., 0.], [1., 0., 0.], [0., 1., 0.]]); n].into(),
                uvs: None,
                texture: None,
                rgba: embedded.then_some([0.2, 0.4, 0.8, 0.25]),
            }]
            .into(),
            triangle_count: n,
            embedded_materials: embedded,
        }
    }
    #[test]
    fn scale_precedes_fixed_frame_pose_and_bounds_use_loaded_vertices() {
        let asset = asset(false, 1);
        let b = Body::Mesh {
            pose: RigidTransform::new(
                [1., 2., 3.],
                [
                    0.,
                    0.,
                    std::f64::consts::FRAC_1_SQRT_2,
                    std::f64::consts::FRAC_1_SQRT_2,
                ],
            )
            .unwrap(),
            scale: [2., 3., 4.],
            resource: "fixture.obj".into(),
            embedded: false,
            rgba: [1.; 4],
        };
        let mesh = prepare(&[(&b, &asset)], &AtomicBool::new(false))
            .unwrap()
            .remove(0)
            .unwrap();
        let expected = [[-2., 2., 3.], [1., 4., 3.]];
        for (a, b) in mesh
            .bounds()
            .iter()
            .flatten()
            .zip(expected.iter().flatten())
        {
            assert!((a - b).abs() < 1e-10);
        }
        assert_eq!(mesh.triangle_count(), 1);
    }
    #[test]
    fn embedded_zero_sentinel_overlay_alpha_and_nonembedded_color_are_explicit() {
        let a = asset(true, 1);
        let zero = body(true, [0.; 4]);
        let tint = body(true, [1., 0., 0., 0.4]);
        let cancel = AtomicBool::new(false);
        let out = prepare(&[(&zero, &a), (&tint, &a)], &cancel).unwrap();
        let zero = &out[0].as_ref().unwrap().parts[0];
        let tinted = &out[1].as_ref().unwrap().parts[0];
        assert_eq!(
            compose(zero.base, zero.tint, [1.; 4]),
            [0.2, 0.4, 0.8, 0.25]
        );
        let c = compose(tinted.base, tinted.tint, [1.; 4]);
        for (a, b) in c.into_iter().zip([0.7, 0.15, 0.3, 0.64]) {
            assert!((a - b).abs() < 1e-6);
        }
        let a = asset(false, 1);
        let b = body(false, [0.1, 0.2, 0.3, 0.4]);
        let p = prepare(&[(&b, &a)], &cancel).unwrap().remove(0).unwrap();
        assert_eq!(p.parts[0].base, [0.1, 0.2, 0.3, 0.4]);
        assert_eq!(p.parts[0].tint, [0.; 4]);
        assert_eq!(compose([1.; 4], [0.; 4], [1., 1., 1., 0.]), [0.; 4]);
    }
    #[test]
    fn loaded_textured_asset_keeps_uv_image_identity_without_urdf_wrapping() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/robot_visualizer");
        let cancel = AtomicBool::new(false);
        let mut loader =
            MeshAssetLoader::new(Some(&root), &LoadOptions::default(), &cancel).unwrap();
        let asset = loader
            .load("materials/textured.obj", true, &cancel)
            .unwrap();
        let b = body(true, [0.; 4]);
        let prepared = prepare(&[(&b, &asset)], &cancel)
            .unwrap()
            .remove(0)
            .unwrap();
        assert!(Arc::ptr_eq(
            prepared.parts[0].texture.as_ref().unwrap(),
            asset.parts[0].texture.as_ref().unwrap()
        ));
        assert!(Arc::ptr_eq(
            prepared.parts[0].uvs.as_ref().unwrap(),
            asset.parts[0].uvs.as_ref().unwrap()
        ));
        assert_eq!(prepared.bounds(), [[0., 0., 0.], [1., 1., 0.]]);
    }
    #[test]
    fn key_errors_stay_scoped_and_repeated_instance_limit_precedes_expansion() {
        let cancel = AtomicBool::new(false);
        let a = asset(false, 1);
        let valid = body(false, [1.; 4]);
        let bad = body(true, [1.; 4]);
        let results = prepare(&[(&bad, &a), (&valid, &a)], &cancel).unwrap();
        assert!(results[0].is_err());
        assert!(results[1].is_ok());
        assert!(prepare(&[(&valid, &a)], &AtomicBool::new(true)).is_err());
        let a = asset(false, MAX_TRIANGLES);
        assert!(prepare(&[(&valid, &a), (&valid, &a)], &cancel).is_err());
        let result = prepare(&[(&valid, &a)], &cancel).unwrap();
        assert_eq!(result[0].as_ref().unwrap().triangle_count(), MAX_TRIANGLES);
    }
    #[test]
    fn malformed_loaded_counts_uvs_images_and_transformed_bounds_are_rejected() {
        let cancel = AtomicBool::new(false);
        let b = body(true, [0.; 4]);
        let mut a = asset(true, 1);
        a.triangle_count = 2;
        assert!(prepare(&[(&b, &a)], &cancel).unwrap()[0].is_err());
        let mut a = asset(true, 1);
        Arc::make_mut(&mut a.parts)[0].uvs = Some(Vec::new().into());
        assert!(prepare(&[(&b, &a)], &cancel).unwrap()[0].is_err());
        let mut a = asset(true, 1);
        Arc::make_mut(&mut a.parts)[0].texture = Some(Arc::new(RobotTexture {
            width: 0,
            height: 1,
            rgba: vec![],
            has_transparency: false,
        }));
        assert!(prepare(&[(&b, &a)], &cancel).unwrap()[0].is_err());
        let a = asset(true, 1);
        let b = Body::Mesh {
            pose: RigidTransform::new([1_000_000.; 3], [0., 0., 0., 1.]).unwrap(),
            scale: [2.; 3],
            resource: "fixture".into(),
            embedded: true,
            rgba: [0.; 4],
        };
        assert!(prepare(&[(&b, &a)], &cancel).unwrap()[0].is_err());
    }
}
