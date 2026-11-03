//! Constant-size preparation for an immutable, already reassembled map.
use super::{combined::CombinedSceneError, map_appearance::MapAppearance, Point3};
use std::sync::Arc;
use studio_protocol::{map_snapshot::Complete, mapping};
#[derive(Debug)]
pub struct TextureMap {
    complete: Arc<Complete>,
    corners: [Point3; 4],
}
impl TextureMap {
    /// Run on the data worker. Receiver already checked the raw cell digest;
    /// this checks shape/binding/world bounds and prepares only four corners.
    pub fn prepare(complete: Arc<Complete>) -> Result<Self, CombinedSceneError> {
        complete
            .header
            .validate()
            .map_err(CombinedSceneError::new)?;
        complete.grid.validate().map_err(CombinedSceneError::new)?;
        if complete.header.metadata != complete.grid.metadata {
            return Err(CombinedSceneError::new(
                "Map metadata contradicts complete cells",
            ));
        }
        let m = &complete.header.metadata;
        let corners = mapping::map_corners(m.width, m.height, m.resolution, &m.origin)
            .ok_or_else(|| CombinedSceneError::new("Invalid map corners"))?
            .map(|point| Point3(mapping::transform_point(&complete.header.transform, point)));
        if !corners.into_iter().all(super::map_geometry::bounded_point) {
            return Err(CombinedSceneError::new("Invalid fixed-frame map bounds"));
        }
        // Shared bounds are row-major [00,10,01,11]; rasterization needs a
        // perimeter [00,10,11,01] to cover the rectangle without overlap.
        let corners = [corners[0], corners[1], corners[3], corners[2]];
        Ok(Self { complete, corners })
    }
    pub fn complete(&self) -> &Complete {
        &self.complete
    }
    pub fn corners(&self) -> &[Point3; 4] {
        &self.corners
    }
    pub fn dimensions(&self) -> [u32; 2] {
        let m = &self.complete.header.metadata;
        [m.width, m.height]
    }
    /// INT8 and UINT8 share the same one-byte bit pattern. No cell conversion
    /// or per-cell geometry is needed on active rendering frames.
    pub fn bytes(&self) -> &[u8] {
        bytemuck::cast_slice(&self.complete.grid.cells)
    }
}
#[derive(Clone, Copy)]
pub struct TextureMapFrame<'a> {
    pub id: u64,
    pub map: &'a Arc<TextureMap>,
    pub appearance: MapAppearance,
}

impl TextureMapFrame<'_> {
    pub fn validate_scene(
        &self,
        frame: &super::combined::CombinedSceneFrame<'_>,
    ) -> Result<(), CombinedSceneError> {
        self.appearance.validate()?;
        if frame.map.is_some()
            || self.id == 0
            || frame.layers.iter().any(|peer| peer.id == self.id)
            || frame.tf.is_some_and(|peer| peer.id == self.id)
            || frame.pose.is_some_and(|peer| peer.id == self.id)
            || frame.path.is_some_and(|peer| peer.id == self.id)
            || self.map.complete().header.owner.fixed_frame != frame.fixed_frame
        {
            return Err(CombinedSceneError::new("Map scene binding is invalid"));
        }
        Ok(())
    }
}

#[cfg(test)]
pub(crate) fn fixture(width: u32, height: u32, cells: Vec<i8>) -> Arc<Complete> {
    use studio_protocol::{
        map_grid::{Grid, Metadata},
        map_snapshot::{Owner, Received, Receiver, Snapshot, SourceStatus},
        subscriptions::{Generation, SubscriptionId},
    };
    let owner = Owner {
        generation: Generation(1),
        subscription: SubscriptionId(1),
        topic: "/map".into(),
        fixed_frame: "world".into(),
    };
    let grid = Grid {
        metadata: Metadata {
            frame_id: "map".into(),
            stamp_ns: 1,
            width,
            height,
            resolution: 1.,
            origin: [0., 0., 0., 0., 0., 0., 1.],
            map_load_time_ns: 0,
        },
        cells: cells.into_boxed_slice(),
    };
    let snapshot = Snapshot::new(
        owner.clone(),
        1,
        grid,
        [0., 0., 0., 0., 0., 0., 1.],
        Default::default(),
        Default::default(),
        SourceStatus::default(),
    )
    .unwrap();
    let mut receiver = Receiver::new(owner).unwrap();
    for i in 0..snapshot.chunk_count() {
        if let Received::Complete(value) = receiver.push(&snapshot.chunk(i).unwrap()).unwrap() {
            return Arc::from(value);
        }
    }
    panic!("incomplete fixture")
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preparation_preserves_row_order_all_bytes_and_rotated_origin() {
        let mut complete = fixture(16, 16, (0..256).map(|i| i as u8 as i8).collect());
        let value = Arc::get_mut(&mut complete).unwrap();
        let q = std::f64::consts::FRAC_1_SQRT_2;
        value.header.metadata.origin = [1., 2., 0., 0., 0., q, q];
        value.grid.metadata = value.header.metadata.clone();
        value.header.transform = [4., 5., 1., 0., 0., 0., 1.];
        let ptr = value.grid.cells.as_ptr();
        let map = TextureMap::prepare(complete).unwrap();
        assert_eq!(map.bytes().as_ptr(), ptr.cast());
        assert_eq!(map.bytes(), &(0..256).map(|i| i as u8).collect::<Vec<_>>());
        for (actual, expected) in
            map.corners()
                .iter()
                .zip([[5., 7., 1.], [5., 23., 1.], [-11., 23., 1.], [-11., 7., 1.]])
        {
            for (a, b) in actual.0.into_iter().zip(expected) {
                assert!((a - b).abs() < 1e-10);
            }
        }
    }
    #[test]
    fn preparation_rejects_contradictory_metadata_and_invalid_world_bounds() {
        let mut complete = fixture(1, 1, vec![0]);
        Arc::get_mut(&mut complete).unwrap().grid.metadata.width = 2;
        assert!(TextureMap::prepare(complete).is_err());
        let mut complete = fixture(1, 1, vec![0]);
        Arc::get_mut(&mut complete).unwrap().header.transform[0] = f64::INFINITY;
        assert!(TextureMap::prepare(complete).is_err());
    }
}
