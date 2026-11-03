//! Bounded raw-image component. Feed `decode` on a worker; use `ImageView::show`
//! only on the egui thread. Typed Image adaptation also belongs to the worker.
mod adapter;
mod compact;
mod frame;
mod mailbox;
mod stream;
mod view;
pub use adapter::{
    decode_typed_image, HeaderStamp, ImageSampleError, TypedImage, IMAGE_MESSAGE_TYPE,
};
pub use compact::decode_compact_image;
pub use frame::{decode, DecodeError, DecodedFrame, RawFrame, MAX_DIMENSION, MAX_FRAME_BYTES};
pub use mailbox::{Counters, FrameMetadata, LatestFrames};
pub use stream::{ImageStream, ImageUpdate};
pub use view::{update_texture, DisplayMode, ImageView};

#[cfg(test)]
mod tests;
