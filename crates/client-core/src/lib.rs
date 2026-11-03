pub use studio_images::images;
#[cfg(feature = "spatial")]
pub use studio_visualization::spatial;
pub mod backend_pipe;
pub mod camera_stream;
pub mod model;
pub mod network_ui;
pub mod session;
pub fn bundled_distro(manifest: &str) -> Result<String, String> {
    let value: serde_json::Value = serde_json::from_str(manifest).map_err(|e| e.to_string())?;
    match value["distro"].as_str() {
        Some(d @ ("humble" | "jazzy" | "lyrical")) => Ok(d.into()),
        _ => Err("Runtime manifest must declare a supported ROS distro".into()),
    }
}

#[cfg(feature = "test-support")]
pub mod diagnostic_acceptance;
pub mod diagnostic_health;
#[cfg(feature = "test-support")]
pub mod map_fixture;
