//! Reuse empty projection payloads after their final immutable reader releases.
use super::*;
use std::sync::mpsc::{self, Receiver, SyncSender};
const SPARES: usize = 3;
#[derive(Debug, Default)]
pub(super) struct Buffers {
    instances: Vec<ClipInstance>,
    kinds: Vec<Kind>,
    keys: Vec<u16>,
}
pub(crate) struct StoragePool {
    sender: SyncSender<Buffers>,
    receiver: Receiver<Buffers>,
}
impl StoragePool {
    pub fn new() -> Self {
        let (sender, receiver) = mpsc::sync_channel(SPARES);
        Self { sender, receiver }
    }
    pub fn frame(&mut self, clock: MarkerClock, records: usize) -> Result<Frame, String> {
        if records > MAX_INSTANCES {
            return Err("Marker projected storage limit".into());
        }
        let mut buffers = self.receiver.try_recv().unwrap_or_default();
        buffers
            .instances
            .try_reserve_exact(records)
            .map_err(|_| "Cannot reserve Marker instances")?;
        buffers
            .kinds
            .try_reserve_exact(records)
            .map_err(|_| "Cannot reserve Marker kinds")?;
        buffers
            .keys
            .try_reserve_exact(records)
            .map_err(|_| "Cannot reserve Marker keys")?;
        Ok(Frame {
            instances: buffers.instances,
            kinds: buffers.kinds,
            key_slots: buffers.keys,
            recycler: Some(self.sender.clone()),
            ..Frame::owned(clock)
        })
    }
}
impl Drop for Sealed {
    fn drop(&mut self) {
        if let Some(recycler) = self.frame.recycler.take() {
            let mut buffers = Buffers {
                instances: std::mem::take(&mut self.frame.instances),
                kinds: std::mem::take(&mut self.frame.kinds),
                keys: std::mem::take(&mut self.frame.key_slots),
            };
            // These elements are Copy; clearing is constant-time and retains no
            // source/key strings, texture Arcs, clock or permission metadata.
            buffers.instances.clear();
            buffers.kinds.clear();
            buffers.keys.clear();
            // Never wait for worker consumption on the final reader's thread.
            // A closed/full pool simply releases the extra storage as before.
            let _ = recycler.try_send(buffers);
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::spatial::RigidTransform;
    fn sealed(pool: &mut StoragePool) -> ReadyFrame {
        let mut frame = pool.frame(MarkerClock::new(), 4).unwrap();
        let key = KeyIdentity {
            owner: 1,
            source_epoch: 1,
            scene_epoch: 1,
            asset_epoch: None,
            key: studio_protocol::marker::Key {
                namespace: "old".into(),
                id: 4,
            },
            version: 1,
        };
        frame
            .append_key(key, |frame| {
                frame.push(
                    Kind::Cube,
                    &Instance {
                        pose: RigidTransform::identity(),
                        scale: [1.; 3],
                        rgba: [1.; 4],
                    },
                    CameraPose::orbit(0.5, 0.8, 8., [0.; 3]),
                    [64, 64],
                )
            })
            .unwrap();
        frame.seal().unwrap()
    }
    #[test]
    fn storage_waits_for_last_reader_and_resets_metadata_before_reuse() {
        let mut pool = StoragePool::new();
        let first = sealed(&mut pool);
        let pointer = first.instances.as_ptr();
        let retained = first.clone();
        let permit = first.permit(|_| true);
        drop(first);
        assert!(
            pool.receiver.try_recv().is_err(),
            "A live immutable reader must retain its payload"
        );
        assert_eq!(retained.len(), 1);
        drop(retained);
        let next = pool.frame(MarkerClock::new(), 4).unwrap();
        assert_eq!(next.instances.as_ptr(), pointer);
        assert!(
            next.instances.is_empty()
                && next.kinds.is_empty()
                && next.key_slots.is_empty()
                && next.keys.is_empty()
        );
        assert!(next.atlas.is_none() && next.textures.is_empty() && next.projection.is_none());
        assert_eq!(next.glyph_count + next.mesh_count, 0);
        let next = next.seal().unwrap();
        assert!(
            permit.validate_for(&next).is_err(),
            "Recycled storage must not revive old frame authority"
        );
    }
    #[test]
    fn returned_storage_cache_is_bounded_and_reservation_rejects_overflow() {
        let mut pool = StoragePool::new();
        assert!(pool.frame(MarkerClock::new(), MAX_INSTANCES + 1).is_err());
        let frames = (0..SPARES + 2)
            .map(|_| sealed(&mut pool))
            .collect::<Vec<_>>();
        drop(frames);
        for _ in 0..SPARES {
            assert!(pool.receiver.try_recv().is_ok());
        }
        assert!(pool.receiver.try_recv().is_err());
    }
}
