//! Atomically admitted Image/CameraInfo readers sharing the scene's TF pair.
use super::*;
use crate::camera_producer::CameraProducer;
use studio_protocol::{camera_info, camera_snapshot::Owner};

pub(super) struct CameraDisplay {
    image: Box<dyn Reader>,
    info: Box<dyn Reader>,
    producer: CameraProducer,
    began: Instant,
    retired_epoch: u64,
}
#[cfg(test)]
mod tests;
impl CameraDisplay {
    pub(super) fn new(a: &Attach, factory: &Factory) -> Result<Self, String> {
        let Output::CameraSnapshot(source) = &a.configuration.output else {
            return Err("Missing Camera source configuration".into());
        };
        let image = factory(&a.configuration)?;
        if a.cancel.load(Ordering::Acquire) {
            return Err("Camera startup canceled after Image open".into());
        }
        let info = factory(&Configuration {
            topic: source.topic.clone(),
            message_type: camera_info::MESSAGE_TYPE.into(),
            best_effort: source.best_effort,
            transient_local: source.transient_local,
            output: Output::Typed,
            spatial: None,
            robot: None,
            map_updates: None,
        })?;
        if a.cancel.load(Ordering::Acquire) {
            return Err("Camera startup canceled after CameraInfo open".into());
        }
        let producer = CameraProducer::new(Owner {
            generation: a.generation,
            subscription: a.subscription,
            topic: a.configuration.topic.clone(),
            info: (**source).clone(),
            fixed_frame: a
                .configuration
                .spatial
                .as_ref()
                .ok_or("Missing Camera scene configuration")?
                .fixed_frame
                .clone(),
        })?;
        Ok(Self {
            image,
            info,
            producer,
            began: Instant::now(),
            retired_epoch: 1,
        })
    }
    pub(super) fn poll(
        &mut self,
        a: &Attach,
        context: &mut Context,
    ) -> Result<Option<EncodedPayload>, String> {
        let now: u64 = self
            .began
            .elapsed()
            .as_nanos()
            .try_into()
            .map_err(|_| "Camera monotonic clock exhausted")?;
        // Apply calibration changes before accepting a subsequent image. Each
        // lane's finite work allowance preserves peers and cancellation checks.
        for _ in 0..2 {
            let Some(input) = self.info.poll_camera_info()? else {
                break;
            };
            self.producer.info(input)?;
        }
        for _ in 0..2 {
            let Some(input) = self.image.poll_camera_image()? else {
                break;
            };
            self.producer.image(input, now)?;
        }
        self.producer.poll(&mut context.tf, now, false)?;
        let epoch = self.producer.source().epoch();
        if epoch != self.retired_epoch {
            a.queue
                .lock()
                .unwrap()
                .clear_samples(a.generation, a.subscription)
                .map_err(|e| format!("Camera queue retirement: {e:?}"))?;
            self.retired_epoch = epoch;
        }
        if a.cancel.load(Ordering::Acquire)
            || a.queue
                .lock()
                .unwrap()
                .stream_queued_samples(a.subscription)
                != Some(0)
        {
            return Ok(None);
        }
        self.producer
            .poll(&mut context.tf, now, true)?
            .map(|message| {
                message
                    .encode()
                    .map(|bytes| EncodedPayload::CameraSnapshot(bytes.into_boxed_slice()))
            })
            .transpose()
    }
}
