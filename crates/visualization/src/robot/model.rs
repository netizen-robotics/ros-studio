use crate::spatial::RigidTransform;
use std::sync::Arc;

pub const MAX_URDF_BYTES: usize = studio_protocol::robot_description::MAX_DOCUMENT_BYTES;
pub const MAX_XML_DEPTH: usize = 32;
pub const MAX_XML_ELEMENTS: usize = 10_000;
pub const MAX_LINKS: usize = 128;
pub const MAX_JOINTS: usize = MAX_LINKS - 1;
pub const MAX_VISUALS: usize = 256;
pub const MAX_MATERIALS: usize = 128;
pub const MAX_NAME_BYTES: usize = 256;
pub const MAX_PATH_BYTES: usize = 4096;
pub const MAX_PACKAGE_ROOTS: usize = 16;
pub const MAX_MESH_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_ASSET_BYTES: usize = 32 * 1024 * 1024;
pub const MAX_ROBOT_TRIANGLES: usize = 50_000;
pub const MAX_COORDINATE: f64 = 1_000_000.0;
pub const MAX_DIMENSION: f64 = 10_000.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RobotErrorKind {
    Busy,
    Io,
    Xml,
    InvalidModel,
    Unsupported,
    Asset,
    Limit,
    Cancelled,
    Pose,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RobotError {
    pub kind: RobotErrorKind,
    pub message: String,
}
impl RobotError {
    pub fn new(kind: RobotErrorKind, message: impl Into<String>) -> Self {
        let mut message = message.into();
        let mut end = message.len().min(512);
        while !message.is_char_boundary(end) {
            end -= 1;
        }
        message.truncate(end);
        Self { kind, message }
    }
}
impl std::fmt::Display for RobotError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for RobotError {}

/// Triangle vertices in the mesh/primitive coordinate system, in metres.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Triangle(pub [[f64; 3]; 3]);
/// Indexed corner UVs. Image rows use top-left origin; retain source UVs until sampling.
pub type TriangleUv = [[f32; 2]; 3];
pub use studio_images::encoded::DecodedRaster as RobotTexture;

#[derive(Clone, Debug)]
pub enum RobotGeometry {
    MeshWithUv {
        triangles: Arc<[Triangle]>,
        uvs: Arc<[TriangleUv]>,
        texture: Option<Arc<RobotTexture>>,
        scale: [f64; 3],
    },
    Box {
        size: [f64; 3],
    },
    Cylinder {
        radius: f64,
        length: f64,
    },
    Sphere {
        radius: f64,
    },
    Mesh {
        triangles: Arc<[Triangle]>,
        /// Positive nonuniform scale; apply before the visual origin.
        scale: [f64; 3],
    },
}

#[derive(Clone, Debug)]
pub struct RobotVisual {
    pub role: GeometryRole,
    /// link <- visual. Joint origins must not be applied a second time here.
    pub origin: RigidTransform,
    pub geometry: RobotGeometry,
    /// Straight RGBA; alpha zero hides, fractional alpha uses sorted blending.
    pub rgba: [f32; 4],
}

/// URDF geometry intent is independent of material, link pose and visibility.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GeometryRole {
    #[default]
    Visual,
    Collision,
}

#[derive(Clone, Debug)]
pub struct RobotLink {
    pub name: String,
    pub visuals: Vec<RobotVisual>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JointKind {
    Fixed,
    Revolute,
    Continuous,
    Prismatic,
    Floating,
    Planar,
}

#[derive(Clone, Debug)]
pub struct RobotJoint {
    pub name: String,
    pub parent: usize,
    pub child: usize,
    pub kind: JointKind,
    /// Description metadata only. External TF owns each rendered link pose.
    pub origin: RigidTransform,
}

#[derive(Clone, Debug)]
pub struct RobotModel {
    pub name: String,
    /// Stable description order; pose batches later use these exact names/indices.
    pub links: Vec<RobotLink>,
    pub joints: Vec<RobotJoint>,
    pub root_link: usize,
}
