//! Complete bounded standard mapping extraction; generic array previews stay unused.
use crate::native_schema::*;
use studio_protocol::mapping::{self, Decoded, Geometry, PathPose};
mod grid;
pub use grid::{native_grid, native_grid_update};
unsafe fn stamped_pose(root: Ptr, message: Ptr, source: &str) -> Result<PathPose, String> {
    schema(root, message, 2)?;
    let (hm, h) = field(root, message, "header", 18, 0)?;
    let (frame_id, stamp_ns) = header(sp_nested(hm), h)?;
    if !frame_id.is_empty() && frame_id != source {
        return Err("Path child frame must be empty or match Path header".into());
    }
    let (pm, p) = field(root, message, "pose", 18, 0)?;
    Ok(PathPose {
        frame_id,
        stamp_ns,
        components: pose(sp_nested(pm), p)?,
    })
}
struct MapInfo {
    width: u32,
    height: u32,
    resolution: f64,
    origin: [f64; 7],
    map_load_time_ns: i64,
}
unsafe fn map_info(root: Ptr, message: Ptr) -> Result<MapInfo, String> {
    let (im, info) = field(root, message, "info", 18, 0)?;
    let schema_info = sp_nested(im);
    schema(schema_info, info, 5)?;
    let (lm, load) = field(schema_info, info, "map_load_time", 18, 0)?;
    let map_load_time_ns = time(sp_nested(lm), load)?;
    let (_, resolution) = field(schema_info, info, "resolution", 1, 0)?;
    let resolution = f64::from(*resolution.cast::<f32>());
    let (_, width) = field(schema_info, info, "width", 12, 0)?;
    let width = *width.cast::<u32>();
    let (_, height) = field(schema_info, info, "height", 12, 0)?;
    let height = *height.cast::<u32>();
    let (om, origin) = field(schema_info, info, "origin", 18, 0)?;
    let origin = pose(sp_nested(om), origin)?;
    Ok(MapInfo {
        width,
        height,
        resolution,
        origin,
        map_load_time_ns,
    })
}
fn map_metadata(
    width: u32,
    height: u32,
    resolution: f64,
    origin: [f64; 7],
    load: i64,
) -> Result<usize, String> {
    let count = (width as usize)
        .checked_mul(height as usize)
        .ok_or("Map dimension overflow")?;
    if width > mapping::MAX_MAP_DIMENSION
        || height > mapping::MAX_MAP_DIMENSION
        || count > mapping::MAX_MAP_CELLS
        || ((width == 0 || height == 0) && (width != 0 || height != 0))
    {
        return Err("Map dimensions exceed bounded complete grid".into());
    }
    // The empty shape validates scalar metadata without allocating source cells.
    Geometry::OccupancyGrid {
        width: 0,
        height: 0,
        resolution,
        origin,
        map_load_time_ns: load,
        cells: Vec::new(),
    }
    .validate("map")?;
    if !mapping::valid_map_bounds(width, height, resolution, &origin) {
        return Err("Map source corners exceed coordinate bounds".into());
    }
    Ok(count)
}
/// Extract one complete standard OccupancyGrid/PoseStamped/Path message.
/// All array lengths and geometry metadata are checked before array allocation.
///
/// # Safety
/// Owner must be live exclusive worker-local native storage loaded with the
/// matching standard C introspection type. Primitive accessors must return
/// initialized aligned values valid until this call returns.
pub unsafe fn native_mapping(
    owner: Ptr,
    topic: &str,
    message_type: &str,
) -> Result<Decoded, String> {
    if owner.is_null() || topic.is_empty() || topic.len() > 256 || topic.contains('\0') {
        return Err("Invalid native mapping owner/topic".into());
    }
    let root = sp_schema(owner, 0);
    let message = sp_message(owner, 0);
    schema(
        root,
        message,
        if message_type == mapping::MAP_MESSAGE_TYPE {
            3
        } else {
            2
        },
    )?;
    let (hm, h) = field(root, message, "header", 18, 0)?;
    let (source_frame, acquisition_stamp_ns) = header(sp_nested(hm), h)?;
    if !studio_protocol::spatial::valid_frame(&source_frame) || acquisition_stamp_ns <= 0 {
        return Err("Mapping requires valid source frame and positive header time".into());
    }
    let geometry = match message_type {
        mapping::MAP_MESSAGE_TYPE => {
            let MapInfo {
                width,
                height,
                resolution,
                origin,
                map_load_time_ns,
            } = map_info(root, message)?;
            let count = map_metadata(width, height, resolution, origin, map_load_time_ns)?;
            let (dm, data) = field(root, message, "data", 9, 2)?;
            if sp_bound(dm) != 0 || sp_length(dm, data) != count {
                return Err("Map data must match complete bounded dimensions".into());
            }
            for index in 0..count {
                let value = sp_element(dm, data, index);
                if value.is_null() || !(-1..=100).contains(&*value.cast::<i8>()) {
                    return Err("Invalid map occupancy value/accessor".into());
                }
            }
            let mut cells = Vec::with_capacity(count);
            for index in 0..count {
                cells.push(*sp_element(dm, data, index).cast::<i8>());
            }
            Geometry::OccupancyGrid {
                width,
                height,
                resolution,
                origin,
                map_load_time_ns,
                cells,
            }
        }
        mapping::POSE_MESSAGE_TYPE => {
            let (pm, p) = field(root, message, "pose", 18, 0)?;
            Geometry::Pose {
                components: pose(sp_nested(pm), p)?,
            }
        }
        mapping::PATH_MESSAGE_TYPE => {
            let (pm, poses) = field(root, message, "poses", 18, 2)?;
            let count = sp_length(pm, poses);
            if sp_bound(pm) != 0 || count > mapping::MAX_PATH_POSES {
                return Err("Path exceeds complete pose budget".into());
            }
            let ps = sp_nested(pm);
            for index in 0..count {
                stamped_pose(ps, sp_element(pm, poses, index), &source_frame)?;
            }
            let mut decoded = Vec::with_capacity(count);
            for index in 0..count {
                decoded.push(stamped_pose(
                    ps,
                    sp_element(pm, poses, index),
                    &source_frame,
                )?);
            }
            Geometry::Path { poses: decoded }
        }
        _ => return Err("Unsupported native mapping type".into()),
    };
    let decoded = Decoded {
        topic: topic.into(),
        message_type: message_type.into(),
        source_frame,
        acquisition_stamp_ns,
        geometry,
    };
    decoded.validate()?;
    Ok(decoded)
}

#[cfg(test)]
mod tests {
    use super::*;
    extern "C" {
        fn sp_resize(member: Ptr, field: Ptr, count: usize) -> i32;
    }
    pub(super) use crate::native_schema::test_support::Fixture;
    impl Fixture {
        fn decode(&self, ty: &str) -> Result<Decoded, String> {
            unsafe { native_mapping(self.0, "/overlay", ty) }
        }
    }
    pub(super) fn header() -> serde_json::Value {
        serde_json::json!({"stamp":{"sec":2,"nanosec":3},"frame_id":"source"})
    }
    pub(super) fn pose() -> serde_json::Value {
        serde_json::json!({"position":{"x":1.,"y":2.,"z":3.},"orientation":{"x":0.,"y":0.,"z":0.,"w":1.}})
    }
    #[test]
    fn native_map_copies_full_65536_cells_and_validates_before_copy() {
        let fixture = Fixture::new(
            "nav_msgs",
            "OccupancyGrid",
            serde_json::json!({"header":header(),"info":{"map_load_time":{"sec":0,"nanosec":0},"resolution":0.5,"width":1,"height":1,"origin":pose()},"data":[-1]}),
        );
        assert!(
            matches!(fixture.decode(mapping::MAP_MESSAGE_TYPE).unwrap().geometry,Geometry::OccupancyGrid{cells,..} if cells==[-1])
        );
        fixture.write(serde_json::json!({"info":{"width":256,"height":256}}));
        unsafe {
            let (m, data) = fixture.root("data", 9, 2);
            assert_ne!(sp_resize(m, data, 65536), 0);
            // Resizing a primitive ROS sequence leaves new bytes uninitialized.
            for index in 0..65536 {
                *sp_element(m, data, index).cast::<i8>() = (index % 101) as i8;
            }
        }
        let full = fixture.decode(mapping::MAP_MESSAGE_TYPE).unwrap();
        assert_eq!(full.acquisition_stamp_ns, 2_000_000_003);
        assert!(
            matches!(full.geometry,Geometry::OccupancyGrid{width:256,height:256,cells,..}
                if cells.len()==65536 && cells.iter().enumerate().all(|(i, value)| *value == (i % 101) as i8))
        );
        unsafe {
            let (m, data) = fixture.root("data", 9, 2);
            assert_ne!(sp_resize(m, data, 65537), 0);
        }
        assert!(fixture.decode(mapping::MAP_MESSAGE_TYPE).is_err());
        fixture.write(serde_json::json!({"info":{"width":0,"height":0},"data":[]}));
        assert!(
            matches!(fixture.decode(mapping::MAP_MESSAGE_TYPE).unwrap().geometry,Geometry::OccupancyGrid{cells,..} if cells.is_empty())
        );
        fixture.write(serde_json::json!({"info":{"resolution":0}}));
        assert!(fixture.decode(mapping::MAP_MESSAGE_TYPE).is_err());
    }
    #[test]
    fn native_path_preserves_1024_order_frames_and_child_times_without_truncation() {
        let poses:Vec<_>=(0..1024).map(|i|serde_json::json!({"header":{"stamp":{"sec":0,"nanosec":i},"frame_id":if i%2==0{""}else{"source"}},"pose":pose()})).collect();
        let fixture = Fixture::new(
            "nav_msgs",
            "Path",
            serde_json::json!({"header":header(),"poses":poses}),
        );
        let full = fixture.decode(mapping::PATH_MESSAGE_TYPE).unwrap();
        if let Geometry::Path { poses } = full.geometry {
            assert_eq!(poses.len(), 1024);
            assert_eq!(poses[1023].stamp_ns, 1023);
            assert_eq!(poses[1].frame_id, "source");
            assert_eq!(poses[0].components, [1., 2., 3., 0., 0., 0., 1.]);
        } else {
            panic!("Wrong mapping geometry");
        }
        unsafe {
            let (m, raw) = fixture.root("poses", 18, 2);
            assert_ne!(sp_resize(m, raw, 1025), 0);
        }
        assert!(fixture.decode(mapping::PATH_MESSAGE_TYPE).is_err());
        fixture.write(serde_json::json!({"poses":[{"header":{"frame_id":"other"},"pose":pose()}]}));
        assert!(fixture.decode(mapping::PATH_MESSAGE_TYPE).is_err());
        fixture.write(serde_json::json!({"poses":[]}));
        assert!(
            matches!(fixture.decode(mapping::PATH_MESSAGE_TYPE).unwrap().geometry,Geometry::Path{poses} if poses.is_empty())
        );
    }
    #[test]
    fn native_pose_checks_header_and_quaternion_instead_of_replacing_invalid_orientation() {
        let fixture = Fixture::new(
            "geometry_msgs",
            "PoseStamped",
            serde_json::json!({"header":header(),"pose":pose()}),
        );
        assert!(
            matches!(fixture.decode(mapping::POSE_MESSAGE_TYPE).unwrap().geometry,Geometry::Pose{components} if components==[1.,2.,3.,0.,0.,0.,1.])
        );
        fixture.write(serde_json::json!({"pose":{"orientation":{"w":0}}}));
        assert!(fixture.decode(mapping::POSE_MESSAGE_TYPE).is_err());
        fixture.write(serde_json::json!({"pose":pose(),"header":{"stamp":{"sec":0,"nanosec":0}}}));
        assert!(fixture.decode(mapping::POSE_MESSAGE_TYPE).is_err());
        assert!(unsafe {
            native_mapping(std::ptr::null_mut(), "/pose", mapping::POSE_MESSAGE_TYPE)
        }
        .is_err());
    }
}
