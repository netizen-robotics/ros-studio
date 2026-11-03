//! Large raw grid extraction; native ROS storage stays exclusively worker-owned.
use super::*;
use studio_protocol::map_grid::{Grid, Metadata, Patch, MAX_CELLS};
extern "C" {
    fn sp_copy_int8_sequence(
        member: Ptr,
        field: Ptr,
        out: *mut i8,
        count: usize,
        maximum: usize,
    ) -> i32;
}
unsafe fn storage(owner: Ptr, members: usize) -> Result<(Ptr, Ptr), String> {
    if owner.is_null() {
        return Err("Null native grid owner".into());
    }
    let root = sp_schema(owner, 0);
    let message = sp_message(owner, 0);
    schema(root, message, members)?;
    Ok((root, message))
}
unsafe fn source_header(root: Ptr, message: Ptr) -> Result<(String, i64), String> {
    let (member, value) = field(root, message, "header", 18, 0)?;
    header(sp_nested(member), value)
}
unsafe fn cells(root: Ptr, message: Ptr, count: usize) -> Result<Box<[i8]>, String> {
    let (member, value) = field(root, message, "data", 9, 2)?;
    if count == 0 || count > MAX_CELLS || sp_bound(member) != 0 || sp_length(member, value) != count
    {
        return Err("Native grid sequence must match complete bounded dimensions".into());
    }
    let mut cells = vec![0i8; count].into_boxed_slice();
    if sp_copy_int8_sequence(member, value, cells.as_mut_ptr(), count, MAX_CELLS) != 0 {
        return Err("Invalid native INT8 grid sequence storage".into());
    }
    Ok(cells)
}
/// Copy a complete raw2048-square OccupancyGrid without generic previews.
/// # Safety
/// Owner must be live, exclusive worker-local C introspection storage for
/// nav_msgs/msg/OccupancyGrid, with initialized aligned primitives and sequences.
pub unsafe fn native_grid(owner: Ptr) -> Result<Grid, String> {
    let (root, message) = storage(owner, 3)?;
    let (frame_id, stamp_ns) = source_header(root, message)?;
    let info = map_info(root, message)?;
    let metadata = Metadata {
        frame_id,
        stamp_ns,
        width: info.width,
        height: info.height,
        resolution: info.resolution,
        origin: info.origin,
        map_load_time_ns: info.map_load_time_ns,
    };
    metadata.validate().map_err(str::to_owned)?;
    let count = metadata.cell_count().map_err(str::to_owned)?;
    let grid = Grid {
        metadata,
        cells: cells(root, message, count)?,
    };
    grid.validate().map_err(str::to_owned)?;
    Ok(grid)
}
/// Copy one complete bounded OccupancyGridUpdate after scalar-region validation.
/// # Safety
/// Owner must be live, exclusive worker-local C introspection storage for
/// map_msgs/msg/OccupancyGridUpdate; its primitives/sequences must remain valid.
pub unsafe fn native_grid_update(owner: Ptr) -> Result<Patch, String> {
    let (root, message) = storage(owner, 6)?;
    let (frame_id, stamp_ns) = source_header(root, message)?;
    let (_, x) = field(root, message, "x", 13, 0)?;
    let (_, y) = field(root, message, "y", 13, 0)?;
    let (_, width) = field(root, message, "width", 12, 0)?;
    let (_, height) = field(root, message, "height", 12, 0)?;
    let mut patch = Patch {
        frame_id,
        stamp_ns,
        x: *x.cast::<i32>(),
        y: *y.cast::<i32>(),
        width: *width.cast::<u32>(),
        height: *height.cast::<u32>(),
        cells: Box::default(),
    };
    let count = patch.cell_count().map_err(str::to_owned)?;
    patch.cells = cells(root, message, count)?;
    patch.validate().map_err(str::to_owned)?;
    Ok(patch)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mapping_codec::tests::{header, pose, Fixture};
    use serde_json::json;
    extern "C" {
        fn sp_resize(member: Ptr, field: Ptr, count: usize) -> i32;
    }
    fn full() -> Fixture {
        Fixture::new(
            "nav_msgs",
            "OccupancyGrid",
            json!({
            "header":header(), "info":{"map_load_time":{"sec":0,"nanosec":0},
            "resolution":0.5,"width":1,"height":1,"origin":pose()},"data":[-128]}),
        )
    }
    fn update() -> Fixture {
        Fixture::new(
            "map_msgs",
            "OccupancyGridUpdate",
            json!({
            "header":header(),"x":0,"y":0,"width":1,"height":1,"data":[127]}),
        )
    }
    unsafe fn fill_maximum(fixture: &Fixture) {
        let (member, data) = fixture.root("data", 9, 2);
        assert_ne!(sp_resize(member, data, MAX_CELLS), 0);
        // Generated ROS primitive resize leaves new elements uninitialized.
        for index in 0..MAX_CELLS {
            *sp_element(member, data, index).cast::<i8>() = index as u8 as i8;
        }
    }
    #[test]
    fn native_raw_full_and_patch_copy_maximum_without_changing_legacy_rules() {
        let fixture = full();
        let small = unsafe { native_grid(fixture.0) }.unwrap();
        assert_eq!(&*small.cells, &[-128]);
        assert!(unsafe { native_mapping(fixture.0, "/map", mapping::MAP_MESSAGE_TYPE) }.is_err());
        fixture.write(json!({"info":{"width":2048,"height":2048}}));
        unsafe {
            fill_maximum(&fixture);
        }
        let grid = unsafe { native_grid(fixture.0) }.unwrap();
        assert_eq!(grid.metadata.frame_id, "source");
        assert_eq!(grid.metadata.stamp_ns, 2_000_000_003);
        assert_eq!(grid.metadata.origin, [1., 2., 3., 0., 0., 0., 1.]);
        assert_eq!(grid.cell_bytes(), 4 * 1024 * 1024);
        assert!(grid
            .cells
            .iter()
            .enumerate()
            .all(|(i, v)| *v == i as u8 as i8));
        assert!(unsafe { native_mapping(fixture.0, "/map", mapping::MAP_MESSAGE_TYPE) }.is_err());
        let fixture = update();
        assert_eq!(
            &*unsafe { native_grid_update(fixture.0) }.unwrap().cells,
            &[127]
        );
        fixture.write(json!({"width":2048,"height":2048}));
        unsafe {
            fill_maximum(&fixture);
        }
        let patch = unsafe { native_grid_update(fixture.0) }.unwrap();
        assert_eq!(patch.cells.len(), MAX_CELLS);
        assert!(patch
            .cells
            .iter()
            .enumerate()
            .all(|(i, v)| *v == i as u8 as i8));
    }
    #[test]
    fn native_full_rejects_bad_metadata_and_exact_cardinality() {
        for change in [
            json!({"info":{"width":0}}),
            json!({"info":{"width":2049}}),
            json!({"info":{"resolution":0}}),
            json!({"info":{"origin":{"orientation":{"w":0}}}}),
            json!({"header":{"frame_id":""}}),
            json!({"header":{"stamp":{"sec":0,"nanosec":0}}}),
            json!({"header":{"stamp":{"nanosec":1_000_000_000}}}),
            json!({"data":[]}),
            json!({"data":[0,1]}),
        ] {
            let fixture = full();
            fixture.write(change);
            assert!(unsafe { native_grid(fixture.0) }.is_err());
        }
    }
    #[test]
    fn native_updates_reject_invalid_regions_and_storage_before_adoption() {
        for change in [
            json!({"x":-1}),
            json!({"y":-1}),
            json!({"x":2048}),
            json!({"y":2147483647}),
            json!({"width":0}),
            json!({"width":2049}),
            json!({"height":4294967295u32}),
            json!({"data":[]}),
            json!({"data":[0,1]}),
            json!({"header":{"frame_id":""}}),
            json!({"header":{"stamp":{"sec":0,"nanosec":0}}}),
        ] {
            let fixture = update();
            fixture.write(change);
            assert!(unsafe { native_grid_update(fixture.0) }.is_err());
        }
        assert!(unsafe { native_grid(update().0) }.is_err());
        assert!(unsafe { native_grid_update(full().0) }.is_err());
        assert!(unsafe { native_grid(std::ptr::null_mut()) }.is_err());
        assert!(unsafe { native_grid_update(std::ptr::null_mut()) }.is_err());
    }
    #[test]
    fn native_bulk_copy_checks_member_kind_count_and_budget_without_writing() {
        let fixture = full();
        let mut out = [42i8; 2];
        unsafe {
            let (member, data) = fixture.root("data", 9, 2);
            assert_ne!(
                sp_copy_int8_sequence(member, data, out.as_mut_ptr(), 2, MAX_CELLS),
                0
            );
            assert_ne!(
                sp_copy_int8_sequence(member, data, out.as_mut_ptr(), 1, 0),
                0
            );
            let (member, data) = fixture.root("header", 18, 0);
            assert_ne!(
                sp_copy_int8_sequence(member, data, out.as_mut_ptr(), 1, MAX_CELLS),
                0
            );
        }
        assert_eq!(out, [42, 42]);
    }
}
