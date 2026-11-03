//! Bounded static Wavefront geometry. Sidecar IO belongs to the asset worker.
//! Convex planar faces only; unsupported surfaces/maps never silently disappear.
use super::mesh::{triangulate, Decoded, Material, Part, TextureReference};
use super::{
    stl::cancelled, RobotError, RobotErrorKind, Triangle, TriangleUv, MAX_COORDINATE,
    MAX_MATERIALS, MAX_MESH_BYTES, MAX_NAME_BYTES, MAX_ROBOT_TRIANGLES, MAX_VISUALS,
};
use std::{collections::BTreeMap, sync::atomic::AtomicBool};

pub(super) const MAX_SOURCE_VERTICES: usize = MAX_ROBOT_TRIANGLES * 3;
const MAX_LINE: usize = 64 * 1024;
const MAX_FACE: usize = 256;

fn invalid(message: &str) -> RobotError {
    RobotError::new(RobotErrorKind::Asset, format!("Wavefront: {message}"))
}
fn limit(message: &str) -> RobotError {
    RobotError::new(RobotErrorKind::Limit, format!("Wavefront: {message}"))
}
fn unsupported(message: &str) -> RobotError {
    RobotError::new(RobotErrorKind::Unsupported, format!("Wavefront: {message}"))
}
fn number(s: &str) -> Result<f64, RobotError> {
    s.parse::<f64>()
        .ok()
        .filter(|n| n.is_finite() && n.abs() <= MAX_COORDINATE)
        .ok_or_else(|| invalid("invalid, nonfinite or out-of-range number"))
}
fn numbers<const N: usize>(s: &str) -> Result<[f64; N], RobotError> {
    let mut words = s.split_whitespace();
    let mut out = [0.; N];
    for n in &mut out {
        *n = number(
            words
                .next()
                .ok_or_else(|| invalid("missing numeric component"))?,
        )?;
    }
    if words.next().is_some() {
        return Err(invalid("extra numeric component"));
    }
    Ok(out)
}
fn name(s: &str) -> Result<(), RobotError> {
    if s.is_empty() || s.len() > MAX_NAME_BYTES || s.chars().any(char::is_control) {
        Err(invalid("invalid or overlong name"))
    } else {
        Ok(())
    }
}
fn line(s: &str) -> Result<(&str, &str), RobotError> {
    if s.len() > MAX_LINE {
        return Err(limit("line exceeds 64 KiB"));
    }
    let s = s.split('#').next().unwrap_or("").trim();
    if s.contains('\\') {
        return Err(unsupported(
            "line continuations and backslash paths are unsupported",
        ));
    }
    Ok(s.split_once(char::is_whitespace)
        .map_or((s, ""), |(a, b)| (a, b.trim())))
}
fn source(bytes: &[u8]) -> Result<&str, RobotError> {
    if bytes.len() > MAX_MESH_BYTES {
        return Err(limit("file exceeds 16 MiB"));
    }
    let s = std::str::from_utf8(bytes).map_err(|_| invalid("file must be UTF-8"))?;
    if s.contains('\0') {
        return Err(invalid("NUL in source"));
    }
    Ok(s)
}
fn index(s: &str, len: usize) -> Result<usize, RobotError> {
    let n: i64 = s.parse().map_err(|_| invalid("invalid face index"))?;
    let i = if n < 0 {
        (len as i64).checked_add(n)
    } else {
        n.checked_sub(1)
    }
    .filter(|n| *n >= 0 && (*n as u64) < len as u64)
    .ok_or_else(|| invalid("face index outside preceding source data"))?;
    Ok(i as usize)
}

fn materials(bytes: &[u8], cancel: &AtomicBool) -> Result<BTreeMap<String, Material>, RobotError> {
    let mut out = BTreeMap::new();
    let mut current: Option<String> = None;
    for s in source(bytes)?.lines() {
        cancelled(cancel)?;
        let (key, value) = line(s)?;
        match key {
            "" => {}
            "newmtl" => {
                name(value)?;
                if out.contains_key(value) {
                    return Err(invalid("duplicate material name"));
                }
                if out.len() == MAX_MATERIALS {
                    return Err(limit("material count exceeds 128"));
                }
                out.insert(
                    value.into(),
                    Material {
                        rgba: [0.7, 0.7, 0.7, 1.],
                        texture: None,
                    },
                );
                current = Some(value.into());
            }
            _ if current.is_none() => return Err(invalid("material property before newmtl")),
            "Kd" => {
                let color = numbers::<3>(value)?;
                if color.iter().any(|v| !(0.0..=1.0).contains(v)) {
                    return Err(invalid("diffuse channels outside [0,1]"));
                }
                let rgba = &mut out.get_mut(current.as_ref().unwrap()).unwrap().rgba;
                for i in 0..3 {
                    rgba[i] = color[i] as f32;
                }
            }
            "d" | "Tr" => {
                let [v] = numbers::<1>(value)?;
                let alpha = if key == "Tr" { 1. - v } else { v };
                if !(0.0..=1.0).contains(&alpha) {
                    return Err(invalid("material alpha outside [0,1]"));
                }
                out.get_mut(current.as_ref().unwrap()).unwrap().rgba[3] = alpha as f32;
            }
            "map_Kd" => {
                if value.is_empty()
                    || value.len() > super::MAX_PATH_BYTES
                    || value.starts_with('-')
                    || value.chars().any(char::is_control)
                {
                    return Err(unsupported(
                        "map_Kd requires a plain local image path without options",
                    ));
                }
                let material = out.get_mut(current.as_ref().unwrap()).unwrap();
                if material.texture.is_some() {
                    return Err(invalid("duplicate diffuse texture map"));
                }
                material.texture = Some(TextureReference {
                    filename: value.into(),
                    material_file: None,
                    semantic: None,
                });
            }
            // Validated lighting metadata: current flat-color renderer does not shade it.
            "Ka" | "Ks" | "Ke" => {
                let _ = numbers::<3>(value)?;
            }
            "Ns" | "Ni" => {
                let _ = numbers::<1>(value)?;
            }
            "illum" => {
                let n = value
                    .parse::<u8>()
                    .map_err(|_| invalid("invalid illumination model"))?;
                if n > 2 {
                    return Err(unsupported("illumination/refraction model"));
                }
            }
            _ => {
                return Err(unsupported(&format!(
                    "material directive {key}; use diffuse colors without texture maps"
                )))
            }
        }
    }
    Ok(out)
}

struct FacePart {
    material: Option<String>,
    triangles: Vec<Triangle>,
    uvs: Option<Vec<TriangleUv>>,
}
pub(super) fn decode(
    bytes: &[u8],
    cancel: &AtomicBool,
    mut sidecar: impl FnMut(&str) -> Result<Vec<u8>, RobotError>,
) -> Result<Decoded, RobotError> {
    cancelled(cancel)?;
    let mut positions = Vec::new();
    let mut uvs = Vec::new();
    let mut normals = 0usize;
    let mut palette = BTreeMap::new();
    let mut selected: Option<String> = None;
    let mut parts: Vec<FacePart> = Vec::new();
    let mut count = 0usize;
    for s in source(bytes)?.lines() {
        cancelled(cancel)?;
        let (key, value) = line(s)?;
        match key {
            "" => {}
            "v" => {
                if positions.len() == MAX_SOURCE_VERTICES {
                    return Err(limit("source positions exceed 150000"));
                }
                positions.push(numbers::<3>(value)?);
            }
            "vt" => {
                if uvs.len() == MAX_SOURCE_VERTICES {
                    return Err(limit("source UVs exceed 150000"));
                }
                uvs.push(numbers::<2>(value)?.map(|n| n as f32));
            }
            "vn" => {
                if normals == MAX_SOURCE_VERTICES {
                    return Err(limit("source normals exceed 150000"));
                }
                let _ = numbers::<3>(value)?;
                normals += 1;
            }
            "mtllib" => {
                // One path per statement permits spaces without guessing multiple file names.
                if value.is_empty()
                    || value.len() > super::MAX_PATH_BYTES
                    || value.chars().any(char::is_control)
                {
                    return Err(invalid("invalid material library path"));
                }
                for (n, mut c) in materials(&sidecar(value)?, cancel)? {
                    if let Some(texture) = &mut c.texture {
                        texture.material_file = Some(value.into());
                    }
                    if palette.contains_key(&n) {
                        return Err(invalid("conflicting material libraries"));
                    }
                    if palette.len() == MAX_MATERIALS {
                        return Err(limit("material count exceeds 128"));
                    }
                    palette.insert(n, c);
                }
            }
            "usemtl" => {
                name(value)?;
                selected = Some(value.into());
            }
            "o" | "g" => {
                if !value.is_empty() {
                    name(value)?;
                }
            }
            "s" => {
                if value != "off" && value != "on" && value.parse::<u32>().is_err() {
                    return Err(invalid("invalid smoothing group"));
                }
            }
            "f" => {
                let mut points = Vec::new();
                let mut corners = Vec::new();
                let mut shape = None;
                for word in value.split_whitespace() {
                    if points.len() == MAX_FACE {
                        return Err(limit("face exceeds 256 corners"));
                    }
                    let mut fields = word.split('/');
                    let p = index(fields.next().unwrap_or(""), positions.len())?;
                    let uv = fields.next();
                    let normal = fields.next();
                    if fields.next().is_some() {
                        return Err(invalid("extra face index field"));
                    }
                    if uv == Some("") && normal.is_none() {
                        return Err(invalid("incomplete face texture index"));
                    }
                    if let Some(v) = uv.filter(|v| !v.is_empty()) {
                        corners.push(uvs[index(v, uvs.len())?]);
                    }
                    if let Some(v) = normal {
                        let _ = index(v, normals)?;
                    }
                    let this = (uv.is_some_and(|v| !v.is_empty()), normal.is_some());
                    if shape.is_some_and(|v| v != this) {
                        return Err(invalid("mixed face index layouts"));
                    }
                    shape = Some(this);
                    points.push(positions[p]);
                }
                if points.len() < 3 {
                    return Err(invalid("face needs at least three corners"));
                }
                let added = points.len() - 2;
                count = count
                    .checked_add(added)
                    .filter(|n| *n <= MAX_ROBOT_TRIANGLES)
                    .ok_or_else(|| limit("expanded mesh exceeds 50000 triangles"))?;
                let triangles = triangulate(&points)?;
                let has_uv = !corners.is_empty();
                if parts
                    .last()
                    .is_none_or(|p| p.material != selected || p.uvs.is_some() != has_uv)
                {
                    if parts.len() == MAX_VISUALS {
                        return Err(limit("material parts exceed 256"));
                    }
                    parts.push(FacePart {
                        material: selected.clone(),
                        triangles: Vec::new(),
                        uvs: has_uv.then(Vec::new),
                    });
                }
                let part = parts.last_mut().unwrap();
                part.triangles.extend(triangles);
                if let Some(uvs) = &mut part.uvs {
                    uvs.extend(
                        (1..corners.len() - 1).map(|i| [corners[0], corners[i], corners[i + 1]]),
                    );
                }
            }
            _ => return Err(unsupported(&format!("geometry directive {key}"))),
        }
    }
    if count == 0 {
        return Err(invalid("mesh contains no faces"));
    }
    let material_count = palette.len();
    let parts = parts
        .into_iter()
        .map(|part| {
            let material = part
                .material
                .map(|n| {
                    palette
                        .get(&n)
                        .cloned()
                        .ok_or_else(|| invalid("undefined face material"))
                })
                .transpose()?;
            Ok(Part {
                triangles: part.triangles,
                rgba: material.as_ref().map(|m| m.rgba),
                uvs: part.uvs,
                texture: material.and_then(|m| m.texture),
            })
        })
        .collect::<Result<Vec<_>, RobotError>>()?;
    Ok(Decoded {
        parts,
        material_count,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    const QUAD: &str = "v 0 0 0\nv 1 0 0\nv 1 1 0\nv 0 1 0\n";
    fn parse(s: &str) -> Result<Vec<Part>, RobotError> {
        decode(s.as_bytes(), &AtomicBool::new(false), |_| {
            Err(invalid("unexpected sidecar"))
        })
        .map(|d| d.parts)
    }
    #[test]
    fn wavefront_dissolve_and_transparency_preserve_fractional_alpha() {
        let cancel = AtomicBool::new(false);
        assert_eq!(
            materials(b"newmtl a\nd 0.25", &cancel).unwrap()["a"].rgba[3],
            0.25
        );
        assert_eq!(
            materials(b"newmtl a\nTr 0.75", &cancel).unwrap()["a"].rgba[3],
            0.25
        );
        assert!(materials(b"newmtl a\nTr -0.1", &cancel).is_err());
    }
    #[test]
    fn preserves_corner_uv_seams_negative_indices_and_uv_presence_boundaries() {
        let s=format!("{QUAD}vt -0.5 2\nvt 1 0\nvt 1 1\nvt 0 1\nf 1/-4 2/-3 3/-2 4/-1\nf 1 2 3\nf 1/4 2/3 3/2\n");
        let parts = parse(&s).unwrap();
        assert_eq!(parts.len(), 3);
        assert_eq!(
            parts[0].uvs.as_deref(),
            Some(
                &[
                    [[-0.5, 2.], [1., 0.], [1., 1.]],
                    [[-0.5, 2.], [1., 1.], [0., 1.]]
                ][..]
            )
        );
        assert!(parts[1].uvs.is_none());
        assert_eq!(
            parts[2].uvs.as_deref(),
            Some(&[[[0., 1.], [1., 1.], [1., 0.]]][..])
        );
        assert!(parse(&s.replace("vt -0.5 2", "vt NaN 2")).is_err());
    }
    #[test]
    fn convex_faces_negative_indices_and_material_groups() {
        let s =
            format!("{QUAD}mtllib palette.mtl\nusemtl red\nf -4 -3 -2 -1\nusemtl green\nf 1 2 3\n");
        let p = decode(s.as_bytes(), &AtomicBool::new(false), |name| {
            assert_eq!(name, "palette.mtl");
            Ok(b"newmtl red\nKd 1 0 0\nnewmtl green\nKd 0 1 0\n".to_vec())
        })
        .unwrap()
        .parts;
        assert_eq!(p.len(), 2);
        assert_eq!(p[0].triangles.len(), 2);
        assert_eq!(p[0].rgba, Some([1., 0., 0., 1.]));
        assert_eq!(p[1].rgba, Some([0., 1., 0., 1.]));
        assert_eq!(
            p[0].triangles[0],
            Triangle([[0., 0., 0.], [1., 0., 0.], [1., 1., 0.]])
        );
    }
    #[test]
    fn rejects_bad_indices_partial_surfaces_and_nonconvex_faces() {
        for face in [
            "f 0 2 3",
            "f 1 2 5",
            "f -5 2 3",
            "f 1/1 2/1 3/1",
            "f 1 2",
            "f 1 3 2 4",
            "f 1 2 2",
            "l 1 2",
            "curv 0 1 1 2",
        ] {
            assert!(parse(&format!("{QUAD}{face}")).is_err(), "{face}");
        }
        assert!(parse("v NaN 0 0").is_err());
        assert!(parse("v 0 0 0 2").is_err());
        assert!(parse("v 0 0 0\nv 1 0 0\nv 0 1 0\nv 0 0 1\nf 1 2 3 4").is_err());
        assert!(parse(&format!("{QUAD}usemtl missing\nf 1 2 3")).is_err());
    }
    #[test]
    fn validates_uv_normal_indices_and_cancellation_before_sidecar() {
        let s = format!("{QUAD}vt 0 0\nvt 1 0\nvt 1 1\nvn 0 0 1\nf 1/1/1 2/2/1 3/3/1");
        assert_eq!(parse(&s).unwrap()[0].triangles.len(), 1);
        assert!(parse(&s.replace("3/3/1", "3/4/1")).is_err());
        assert_eq!(
            decode(b"mtllib x", &AtomicBool::new(true), |_| panic!(
                "cancelled IO"
            ))
            .unwrap_err()
            .kind,
            RobotErrorKind::Cancelled
        );
    }
    #[test]
    fn limits_precede_expansion_and_unsupported_material_maps_fail() {
        assert_eq!(
            parse(&format!("{QUAD}f {}", "1 ".repeat(MAX_FACE + 1)))
                .unwrap_err()
                .kind,
            RobotErrorKind::Limit
        );
        assert_eq!(
            parse(&"#".repeat(MAX_LINE + 1)).unwrap_err().kind,
            RobotErrorKind::Limit
        );
        assert_eq!(
            parse(&"v 0 0 0\n".repeat(MAX_SOURCE_VERTICES + 1))
                .unwrap_err()
                .kind,
            RobotErrorKind::Limit
        );
        for mtl in [
            "newmtl a\nmap_Kd -s 1 1 missing.png",
            "newmtl a\nd 1.5",
            "Kd 1 0 0",
            "newmtl a\nKd 2 0 0",
            "newmtl a\nnewmtl a",
        ] {
            assert!(materials(mtl.as_bytes(), &AtomicBool::new(false)).is_err());
        }
    }
    #[test]
    fn exact_triangle_part_material_bounds_and_mid_sidecar_cancellation() {
        let prefix = "v 0 0 0\nv 1 0 0\nv 0 1 0\n";
        let max = format!("{prefix}{}", "f 1 2 3\n".repeat(MAX_ROBOT_TRIANGLES));
        assert_eq!(parse(&max).unwrap()[0].triangles.len(), MAX_ROBOT_TRIANGLES);
        assert_eq!(
            parse(&(max + "f 1 2 3")).unwrap_err().kind,
            RobotErrorKind::Limit
        );
        let mut groups = format!("{prefix}mtllib colors.mtl\n");
        for i in 0..MAX_VISUALS {
            groups.push_str(if i % 2 == 0 {
                "usemtl a\nf 1 2 3\n"
            } else {
                "usemtl b\nf 1 2 3\n"
            });
        }
        let load = |_: &str| Ok(b"newmtl a\nKd 1 0 0\nnewmtl b\nKd 0 1 0".to_vec());
        let cancel = AtomicBool::new(false);
        assert_eq!(
            decode(groups.as_bytes(), &cancel, load)
                .unwrap()
                .parts
                .len(),
            MAX_VISUALS
        );
        groups.push_str("usemtl a\nf 1 2 3\n");
        assert_eq!(
            decode(groups.as_bytes(), &cancel, load).unwrap_err().kind,
            RobotErrorKind::Limit
        );
        let mut library = String::new();
        for i in 0..MAX_MATERIALS {
            library.push_str(&format!("newmtl m{i}\nKd 0 1 0\n"));
        }
        assert_eq!(
            materials(library.as_bytes(), &cancel).unwrap().len(),
            MAX_MATERIALS
        );
        library.push_str("newmtl overflow\n");
        assert_eq!(
            materials(library.as_bytes(), &cancel).unwrap_err().kind,
            RobotErrorKind::Limit
        );
        assert_eq!(
            decode(b"mtllib colors.mtl", &cancel, |_| {
                cancel.store(true, std::sync::atomic::Ordering::Relaxed);
                Ok(b"newmtl a\nKd 1 0 0".to_vec())
            })
            .unwrap_err()
            .kind,
            RobotErrorKind::Cancelled
        );
    }
}
