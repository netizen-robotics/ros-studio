//! Compatibility facade while the independent applications are qualified.
pub use studio_images::images;
pub use studio_inspection::{plots, typed_view};
pub use studio_ui_foundation::{workspace_dialog, workspace_document};
pub use studio_visualization::{robot, spatial};
pub use studio_workspace::workspace;
pub mod subscriptions;
