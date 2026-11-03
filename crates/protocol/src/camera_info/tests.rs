use super::*;
fn calibration() -> Calibration {
    Calibration {
        stamp_ns: 1,
        frame_id: "camera_optical".into(),
        width: 1280,
        height: 960,
        distortion_model: "plumb_bob".into(),
        d: vec![0.1, 0., 0., 0., 0.],
        k: [1000., 0., 600., 0., 900., 400., 0., 0., 1.],
        r: identity(),
        p: [1000., 0., 600., -100., 0., 900., 400., 18., 0., 0., 1., 0.],
        binning_x: 0,
        binning_y: 0,
        roi: RegionOfInterest::default(),
    }
}
#[test]
fn roi_then_binning_preserves_stereo_and_source_metadata() {
    let mut c = calibration();
    c.binning_x = 2;
    c.binning_y = 4;
    c.roi = RegionOfInterest {
        x_offset: 100,
        y_offset: 80,
        width: 800,
        height: 600,
        do_rectify: false,
    };
    let bytes = c.encode().unwrap();
    let effective = c.effective().unwrap();
    assert_eq!(effective.size, [400, 150]);
    assert_eq!(
        effective.p,
        [500., 0., 250., -50., 0., 225., 80., 4.5, 0., 0., 1., 0.]
    );
    // Analytical pixel equality: crop in unbinned pixels, then bin each axis.
    let [x, y, z] = [0.3, -0.1, 2.];
    for (row, offset, binning) in [(0, 100., 2.), (1, 80., 4.)] {
        let i = row * 4;
        let coordinate = if row == 0 { x } else { y };
        let before = (c.p[i] * coordinate + c.p[i + 3]) / z + c.p[i + 2];
        let after = (effective.p[i] * coordinate + effective.p[i + 3]) / z + effective.p[i + 2];
        assert!((after - (before - offset) / binning).abs() < 1e-12);
    }
    assert_eq!(c.encode().unwrap(), bytes);
    assert_eq!(Calibration::decode(&bytes).unwrap(), c);
}
#[test]
fn uncalibrated_malformed_and_unsupported_rectified_roi_are_distinct() {
    let mut c = calibration();
    c.k = [0.; 9];
    c.r = [0.; 9];
    c.p = [0.; 12];
    assert!(c.validate().is_ok());
    assert!(c.effective().unwrap_err().contains("uncalibrated"));
    let mut c = calibration();
    c.r[0] = -1.;
    assert!(c.validate().is_err());
    let mut c = calibration();
    c.p[11] = 1.;
    assert!(c.validate().is_err());
    let mut c = calibration();
    c.k[0] = f64::NAN;
    assert!(c.validate().is_err());
    let mut c = calibration();
    c.roi.width = 800;
    c.roi.height = 600;
    c.roi.do_rectify = true;
    assert!(c.validate().is_ok());
    assert!(c.effective().unwrap_err().contains("unsupported"));
    c.d.fill(0.);
    assert_eq!(c.effective().unwrap().size, [800, 600]);
    let mut c = calibration();
    c.roi.x_offset = u32::MAX;
    c.roi.width = 2;
    c.roi.height = 2;
    assert!(c.validate().is_err());
    let mut c = calibration();
    c.binning_x = 32768;
    assert!(c.effective().is_err());
}
#[test]
fn pairing_requires_frame_and_dimensions_but_not_equal_timestamps() {
    let mut c = calibration();
    c.binning_x = 5;
    c.binning_y = 5;
    let mut image = crate::image::ImageMetadata {
        header_stamp: crate::image::HeaderStamp {
            seconds: 10,
            nanoseconds: 0,
        },
        frame_id: c.frame_id.clone(),
        width: 256,
        height: 192,
        encoding: "rgb8".into(),
        is_bigendian: false,
        step: 768,
    };
    assert!(c.matches_image(&image).is_ok());
    image.frame_id = "other_optical".into();
    assert!(c.matches_image(&image).is_err());
    image.frame_id = c.frame_id.clone();
    image.height -= 1;
    assert!(c.matches_image(&image).is_err());
}
#[test]
fn wire_bounds_strict_arrays_and_unknown_fields_reject() {
    let c = calibration();
    let mut v = serde_json::to_value(&c).unwrap();
    v["d"] = serde_json::json!(vec![0.; MAX_DISTORTION + 1]);
    assert!(Calibration::decode(&serde_json::to_vec(&v).unwrap()).is_err());
    let mut v = serde_json::to_value(&c).unwrap();
    v["k"] = serde_json::json!([1, 2, 3]);
    assert!(Calibration::decode(&serde_json::to_vec(&v).unwrap()).is_err());
    let mut v = serde_json::to_value(&c).unwrap();
    v["unexpected"] = true.into();
    assert!(Calibration::decode(&serde_json::to_vec(&v).unwrap()).is_err());
    assert!(Calibration::decode(&vec![b' '; MAX_ENCODED_BYTES + 1]).is_err());
}
