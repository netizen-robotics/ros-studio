use studio_protocol::{
    map_grid::{
        Apply, Assembly, Grid, Invalidation, Metadata, Patch, Status, MAX_CELLS, MAX_DIMENSION,
    },
    mapping,
};

fn grid(width: u32, height: u32, stamp_ns: i64) -> Grid {
    Grid {
        metadata: Metadata {
            frame_id: "map".into(),
            stamp_ns,
            width,
            height,
            resolution: 0.05,
            origin: [0., 0., 0., 0., 0., 0., 1.],
            map_load_time_ns: 1,
        },
        cells: vec![0; width as usize * height as usize].into_boxed_slice(),
    }
}
fn patch(x: i32, y: i32, width: u32, height: u32, stamp_ns: i64) -> Patch {
    Patch {
        frame_id: "map".into(),
        stamp_ns,
        x,
        y,
        width,
        height,
        cells: vec![100; width as usize * height as usize].into_boxed_slice(),
    }
}

#[test]
fn maximum_map_and_full_area_patch_preserve_raw_bytes_with_four_mib_storage() {
    let mut assembly = Assembly::default();
    let owner = assembly.start().unwrap();
    let mut full = grid(MAX_DIMENSION, MAX_DIMENSION, 10);
    for (index, cell) in full.cells.iter_mut().enumerate() {
        *cell = index as u8 as i8;
    }
    assert_eq!(full.cell_bytes(), 4 * 1024 * 1024);
    assert_eq!(
        assembly.full(owner, 1, full),
        Ok(Apply::Updated { revision: 1 })
    );
    assert_eq!(assembly.cell_bytes(), MAX_CELLS);
    assert_eq!(
        &assembly.snapshot().unwrap().cells[127..130],
        &[127, -128, -127]
    );
    let mut update = patch(0, 0, MAX_DIMENSION, MAX_DIMENSION, 11);
    update.cells.fill(-128);
    assert_eq!(
        assembly.patch(owner, 2, update),
        Ok(Apply::Updated { revision: 2 })
    );
    assert!(assembly
        .snapshot()
        .unwrap()
        .cells
        .iter()
        .all(|v| *v == -128));
    assert_eq!(assembly.cell_bytes(), MAX_CELLS);
    assembly.stop();
    assert_eq!(assembly.cell_bytes(), 0);
    assert_eq!(assembly.status(), Status::Stopped);
}

#[test]
fn dimension_and_count_limits_reject_before_adoption() {
    for (width, height) in [
        (0, 0),
        (0, 1),
        (1, 0),
        (2049, 1),
        (1, 2049),
        (u32::MAX, u32::MAX),
    ] {
        let mut bad = grid(1, 1, 10);
        bad.metadata.width = width;
        bad.metadata.height = height;
        assert!(bad.validate().is_err());
    }
    let mut truncated = grid(3, 4, 10);
    truncated.cells = vec![0; 11].into_boxed_slice();
    assert!(truncated.validate().is_err());
    truncated.cells = vec![0; 13].into_boxed_slice();
    assert!(truncated.validate().is_err());
}

#[test]
fn patch_metadata_can_be_checked_before_allocating_cells() {
    let mut update = patch(0, 0, 1, 1, 10);
    update.width = MAX_DIMENSION;
    update.height = MAX_DIMENSION;
    update.cells = Box::default();
    assert_eq!(update.cell_count(), Ok(MAX_CELLS));
    assert!(update.validate().is_err());
    update.x = 1;
    assert!(update.cell_count().is_err());
}

#[test]
fn finite_normalized_source_bounds_and_stamps_are_shared_with_legacy_mapping() {
    let mut valid = grid(2048, 1024, 10);
    let half = std::f64::consts::FRAC_1_SQRT_2;
    valid.metadata.origin = [10., 20., 30., 0., 0., half, half];
    valid.validate().unwrap();
    for edit in 0..9 {
        let mut bad = grid(1, 1, 10);
        match edit {
            0 => bad.metadata.resolution = f64::NAN,
            1 => bad.metadata.origin[0] = f64::INFINITY,
            2 => bad.metadata.origin[6] = 2.,
            3 => {
                bad.metadata.origin[0] = mapping::MAX_COORDINATE;
            }
            4 => bad.metadata.stamp_ns = 0,
            5 => bad.metadata.stamp_ns = mapping::MAX_STAMP_NS + 1,
            6 => bad.metadata.map_load_time_ns = -1,
            7 => bad.metadata.frame_id = "/bad frame".into(),
            _ => bad.metadata.resolution = 0.,
        }
        assert!(bad.validate().is_err(), "variant {edit}");
    }
    let mut too_wide = grid(2048, 2048, 10);
    too_wide.metadata.resolution = 1000.;
    assert!(too_wide.validate().is_err());
    assert_eq!(mapping::MAX_MAP_DIMENSION, 256);
    assert!(mapping::Geometry::OccupancyGrid {
        width: 257,
        height: 1,
        resolution: 0.05,
        origin: [0., 0., 0., 0., 0., 0., 1.],
        map_load_time_ns: 1,
        cells: vec![0; 257],
    }
    .validate("map")
    .is_err());
    assert!(mapping::Geometry::OccupancyGrid {
        width: 1,
        height: 1,
        resolution: 0.05,
        origin: [0., 0., 0., 0., 0., 0., 1.],
        map_load_time_ns: 1,
        cells: vec![-128],
    }
    .validate("map")
    .is_err());
}

#[test]
fn patches_wait_for_base_and_copy_rows_without_touching_neighbor_cells() {
    let mut assembly = Assembly::default();
    let owner = assembly.start().unwrap();
    assert_eq!(
        assembly.patch(owner, 1, patch(0, 0, 1, 1, 9)),
        Ok(Apply::WaitingFull)
    );
    assert_eq!(assembly.counters().updates_without_base, 1);
    assembly.full(owner, 2, grid(4, 3, 10)).unwrap();
    let mut update = patch(2, 1, 2, 2, 11);
    update.cells = vec![1, 2, -1, -128].into_boxed_slice();
    assembly.patch(owner, 3, update).unwrap();
    let snapshot = assembly.snapshot().unwrap();
    assert_eq!(&*snapshot.cells, &[0, 0, 0, 0, 0, 0, 1, 2, 0, 0, -1, -128]);
    assert_eq!(snapshot.metadata.stamp_ns, 11);
    assert_eq!(assembly.revision(), 2);
    assert_eq!(assembly.counters().patches, 1);
}

#[test]
fn malformed_current_patch_invalidates_without_partial_rows_and_requires_fresh_full() {
    for edit in 0..8 {
        let mut assembly = Assembly::default();
        let owner = assembly.start().unwrap();
        assembly.full(owner, 1, grid(4, 3, 10)).unwrap();
        let mut update = patch(2, 1, 2, 2, 11);
        match edit {
            0 => update.cells = vec![100; 3].into_boxed_slice(),
            1 => update.x = -1,
            2 => update.y = -1,
            3 => update.x = i32::MAX,
            4 => update.y = 2,
            5 => update.frame_id = "other_frame".into(),
            6 => update.width = 0,
            _ => update.height = u32::MAX,
        }
        assert!(assembly.patch(owner, 2, update).is_err(), "variant {edit}");
        assert_eq!(assembly.cell_bytes(), 0);
        assert_eq!(
            assembly.status(),
            Status::WaitingFull(Some(Invalidation::InvalidInput))
        );
        assert_eq!(
            assembly.patch(owner, 3, patch(0, 0, 1, 1, 12)),
            Ok(Apply::WaitingFull)
        );
        assert_eq!(
            assembly.full(owner, 4, grid(2, 2, 13)),
            Ok(Apply::Updated { revision: 2 })
        );
    }
}

#[test]
fn stale_owner_and_sequence_including_malformed_input_cannot_clear_current_map() {
    let mut assembly = Assembly::default();
    let retired = assembly.start().unwrap();
    assembly.full(retired, 1, grid(1, 1, 1)).unwrap();
    let current = assembly.start().unwrap();
    assembly.full(current, 1, grid(2, 2, 10)).unwrap();
    assert_eq!(
        assembly.patch(retired, u64::MAX, patch(-1, 0, 1, 1, 11)),
        Ok(Apply::Stale)
    );
    assert_eq!(
        assembly.patch(current, 1, patch(-1, 0, 1, 1, 11)),
        Ok(Apply::Stale)
    );
    assert!(!assembly.invalidate(retired, Invalidation::SourceFailure));
    let mut peer = Assembly::default();
    let peer_token = peer.start().unwrap();
    assert_ne!(peer_token, current);
    assert_eq!(
        assembly.full(peer_token, 99, grid(1, 1, 10)),
        Ok(Apply::Stale)
    );
    assert_eq!(assembly.cell_bytes(), 4);
    assert_eq!(assembly.status(), Status::Ready);
    assert_eq!(assembly.revision(), 1);
    assert_eq!(assembly.counters().stale_inputs, 3);
    assert_eq!(
        assembly.patch(current, 2, patch(1, 1, 1, 1, 11)),
        Ok(Apply::Updated { revision: 2 })
    );
}

#[test]
fn known_loss_or_fault_requires_full_snapshot_and_does_not_replay_patches() {
    for reason in [
        Invalidation::QueueOverflow,
        Invalidation::SourceFailure,
        Invalidation::Clear,
    ] {
        let mut assembly = Assembly::default();
        let owner = assembly.start().unwrap();
        assembly.full(owner, 1, grid(2, 2, 10)).unwrap();
        assert!(assembly.invalidate(owner, reason));
        assert_eq!(assembly.cell_bytes(), 0);
        assert_eq!(assembly.status(), Status::WaitingFull(Some(reason)));
        assert_eq!(
            assembly.patch(owner, 2, patch(0, 0, 1, 1, 11)),
            Ok(Apply::WaitingFull)
        );
        assembly.full(owner, 3, grid(2, 2, 12)).unwrap();
        assert_eq!(&*assembly.snapshot().unwrap().cells, &[0, 0, 0, 0]);
        assert_eq!(assembly.counters().invalidations, 1);
    }
}

#[test]
fn equal_stamp_distinct_patches_are_valid_and_older_source_messages_are_ignored() {
    let mut assembly = Assembly::default();
    let owner = assembly.start().unwrap();
    assembly.full(owner, 1, grid(2, 1, 10)).unwrap();
    assembly.patch(owner, 2, patch(0, 0, 1, 1, 10)).unwrap();
    assembly.patch(owner, 3, patch(1, 0, 1, 1, 10)).unwrap();
    assert_eq!(&*assembly.snapshot().unwrap().cells, &[100, 100]);
    assert_eq!(
        assembly.patch(owner, 4, patch(0, 0, 1, 1, 9)),
        Ok(Apply::Stale)
    );
    assert_eq!(assembly.full(owner, 5, grid(2, 1, 9)), Ok(Apply::Stale));
    assert_eq!(assembly.revision(), 3);
    assert_eq!(&*assembly.snapshot().unwrap().cells, &[100, 100]);
}

#[test]
fn full_replacement_changes_metadata_and_stop_releases_owner_storage() {
    let mut assembly = Assembly::default();
    let owner = assembly.start().unwrap();
    assembly.full(owner, 1, grid(4, 3, 10)).unwrap();
    assert_eq!(
        assembly.full(owner, 2, grid(4, 3, 10)),
        Ok(Apply::Unchanged)
    );
    let mut replacement = grid(1, 2, 11);
    replacement.metadata.origin[0] = 20.;
    replacement.metadata.frame_id = "replacement".into();
    assembly.full(owner, 3, replacement).unwrap();
    assert_eq!(assembly.cell_bytes(), 2);
    assert_eq!(assembly.snapshot().unwrap().metadata.origin[0], 20.);
    assembly.stop();
    assert!(assembly.snapshot().is_none());
    assert_eq!(assembly.status(), Status::Stopped);
    assert_eq!(
        assembly.patch(owner, 4, patch(0, 0, 1, 1, 12)),
        Ok(Apply::Stale)
    );
    let next = assembly.start().unwrap();
    assert_ne!(next, owner);
    assert_eq!(assembly.status(), Status::WaitingFull(None));
    assert_eq!(assembly.revision(), 0);
    assert_eq!(assembly.cell_bytes(), 0);
}

#[test]
fn malformed_full_snapshot_clears_only_current_owner_and_recovers_on_valid_full() {
    let mut assembly = Assembly::default();
    let retired = assembly.start().unwrap();
    let owner = assembly.start().unwrap();
    assembly.full(owner, 1, grid(2, 2, 10)).unwrap();
    let mut invalid = grid(2, 2, 11);
    invalid.cells = vec![0; 3].into_boxed_slice();
    assert_eq!(assembly.full(retired, 100, invalid), Ok(Apply::Stale));
    assert_eq!(assembly.cell_bytes(), 4);
    let mut invalid = grid(2, 2, 11);
    invalid.metadata.origin[6] = 0.;
    assert!(assembly.full(owner, 2, invalid).is_err());
    assert!(assembly.snapshot().is_none());
    assert_eq!(
        assembly.status(),
        Status::WaitingFull(Some(Invalidation::InvalidInput))
    );
    assembly.full(owner, 3, grid(1, 1, 12)).unwrap();
    assert_eq!(assembly.cell_bytes(), 1);
    assert_eq!(assembly.status(), Status::Ready);
}

#[test]
fn changed_equal_stamp_full_map_cannot_silently_roll_back_applied_updates() {
    let mut assembly = Assembly::default();
    let owner = assembly.start().unwrap();
    assembly.full(owner, 1, grid(2, 1, 10)).unwrap();
    assembly.patch(owner, 2, patch(0, 0, 1, 1, 10)).unwrap();
    assert!(assembly.full(owner, 3, grid(2, 1, 10)).is_err());
    assert_eq!(assembly.cell_bytes(), 0);
    assert_eq!(
        assembly.patch(owner, 4, patch(1, 0, 1, 1, 11)),
        Ok(Apply::WaitingFull)
    );
    assembly.full(owner, 5, grid(2, 1, 12)).unwrap();
    assert_eq!(&*assembly.snapshot().unwrap().cells, &[0, 0]);
}
