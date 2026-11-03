//! Shared bounded static mesh output and convex planar triangulation.
use super::{RobotError, RobotErrorKind, Triangle, TriangleUv};
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct TextureReference {
    pub filename: String,
    pub material_file: Option<String>,
    pub semantic: Option<String>,
}
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Material {
    pub rgba: [f32; 4],
    pub texture: Option<TextureReference>,
}
#[derive(Debug)]
pub(super) struct Part {
    pub triangles: Vec<Triangle>,
    pub rgba: Option<[f32; 4]>,
    pub uvs: Option<Vec<TriangleUv>>,
    pub texture: Option<TextureReference>,
}
#[derive(Debug)]
pub(super) struct Decoded {
    pub parts: Vec<Part>,
    pub material_count: usize,
}
fn invalid(message: &str) -> RobotError {
    RobotError::new(RobotErrorKind::Asset, message)
}
fn unsupported(message: &str) -> RobotError {
    RobotError::new(RobotErrorKind::Unsupported, message)
}
fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    std::array::from_fn(|i| a[i] - b[i])
}
fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    (0..3).map(|i| a[i] * b[i]).sum()
}
pub(super) fn triangulate(points: &[[f64; 3]]) -> Result<Vec<Triangle>, RobotError> {
    let n = cross(sub(points[1], points[0]), sub(points[2], points[0]));
    let norm = dot(n, n).sqrt();
    if !norm.is_finite() || norm == 0. {
        return Err(invalid("degenerate face"));
    }
    let n = n.map(|v| v / norm);
    let scale = points
        .iter()
        .map(|p| dot(sub(*p, points[0]), sub(*p, points[0])).sqrt())
        .fold(1., f64::max);
    for i in 0..points.len() {
        if dot(n, sub(points[i], points[0])).abs() > scale * 1e-8 {
            return Err(unsupported("nonplanar faces; export triangulated geometry"));
        }
        let turn = cross(
            sub(points[(i + 1) % points.len()], points[i]),
            sub(
                points[(i + 2) % points.len()],
                points[(i + 1) % points.len()],
            ),
        );
        if dot(n, turn) <= 0. {
            return Err(unsupported(
                "concave, self-intersecting or collinear faces; export triangulated geometry",
            ));
        }
    }
    // A convex face must place every other vertex on the same side of every edge.
    // Local turn checks alone would incorrectly admit a star polygon.
    for i in 0..points.len() {
        let edge = sub(points[(i + 1) % points.len()], points[i]);
        for (j, p) in points.iter().enumerate() {
            if j != i
                && j != (i + 1) % points.len()
                && dot(n, cross(edge, sub(*p, points[i]))) <= 0.
            {
                return Err(unsupported(
                    "self-intersecting or nonconvex face; export triangulated geometry",
                ));
            }
        }
    }
    Ok((1..points.len() - 1)
        .map(|i| Triangle([points[0], points[i], points[i + 1]]))
        .collect())
}
