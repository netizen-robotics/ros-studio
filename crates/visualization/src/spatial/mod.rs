//! Bounded scene geometry and native 3D presentation; ROS and native TF stay upstream.
mod adapter;
pub mod calibrated_camera;
pub mod calibrated_view;
mod camera3d;
pub mod camera_fit;
pub mod cloud_live;
pub mod combined;
mod gpu;
mod live;
pub mod map_appearance;
pub mod map_geometry;
pub mod map_palette;
pub mod map_texture;
pub mod mapping_live;
pub mod marker_clock;
pub mod marker_geometry;
pub mod marker_mesh;
pub mod marker_primitives;
pub mod marker_text;
pub mod marker_worker;
mod math;
pub mod overlay_geometry;
mod primitive_mesh;
pub mod robot_geometry;
mod scene;
pub mod scene_view;
pub mod sensor_appearance;
pub mod sensor_appearance_view;
pub mod sensor_history;
pub mod uncertainty;
mod view;
pub use adapter::{
    decode_laser_scan, decode_tf_message, SpatialClockPolicy, SpatialSampleError, TfStreamKind,
    TypedScan, TypedTfBatch, TypedTransform, MAX_TF_TRANSFORMS, SCAN_MESSAGE_TYPE, TF_MESSAGE_TYPE,
};
pub use camera3d::CameraPose;
pub use gpu::{MarkerAllocations, RenderVertex, Renderer3d};
pub use live::{
    decode_spatial_sample, SpatialFrame, SpatialLiveError, SpatialStream, SpatialTfStatus,
    SpatialUpdate,
};
pub use math::{Point3, RigidTransform};
pub use scene::{
    Clock, Layer, LayerStatus, LayerToken, Scan, Scene, SceneError, TfOutcome, MAX_LAYERS,
    MAX_POINTS,
};
pub use view::Camera;
pub use view::CameraMode;
#[cfg(test)]
mod tests;

pub mod map_appearance_view;
