//! Bounded robot assets, local appearance and externally supplied TF presentation.
mod appearance;
mod appearance_view;
pub use appearance::{present_robot, LinkAppearance, RobotAppearance};
pub use appearance_view::RobotAppearanceView;
mod assets;
mod live;
mod setup;
mod setup_view;
pub use setup::{RobotDraft, RobotReview, RobotSetupAction, RobotSetupState};
pub use setup_view::{show_robot_setup, show_robot_setup_in_scene};
mod collada;
mod mesh;
mod model;
mod obj;
mod parser;
mod stl;
mod view;
mod worker;

pub use assets::{
    load_model, load_model_from_text, LoadOptions, MeshAsset, MeshAssetLoader, MeshAssetPart,
    MeshAssetStats, MAX_MESH_PARTS, MAX_MESH_RESOURCES,
};
pub use live::{RobotLive, RobotToken};
pub use model::*;
pub use view::{RobotView, RobotViewState};
pub use worker::{AssetWorker, LoadCompletion, LoadId, LoadSource, LoadedRobot};
