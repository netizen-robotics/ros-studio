//! Standalone numeric trace component. Input is typed; no diagnostic-text parsing.
mod axis;
mod model;
mod renderer;
mod typed_adapter;
pub use model::*;
pub use renderer::{PlotRenderer, PlotStatus};
pub use typed_adapter::{
    numeric_fields, numeric_value, typed_path, MAX_NUMERIC_FIELDS, MAX_NUMERIC_PATH_BYTES,
};
#[cfg(test)]
mod tests;
