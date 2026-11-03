//! Complete bounded CameraInfo extraction using shared C introspection helpers.
use crate::native_schema::*;
use studio_protocol::camera_info::{Calibration, RegionOfInterest, MAX_DISTORTION};

unsafe fn number(root: Ptr, message: Ptr, name: &str) -> Result<u32, String> {
    let (_, value) = field(root, message, name, 12, 0)?;
    Ok(*value.cast::<u32>())
}
unsafe fn matrix<const N: usize>(root: Ptr, message: Ptr, name: &str) -> Result<[f64; N], String> {
    let (member, value) = field(root, message, name, 2, 1)?;
    if sp_bound(member) != N || sp_length(member, value) != N {
        return Err(format!(
            "CameraInfo {name} requires exactly {N} fixed doubles"
        ));
    }
    let mut result = [0.; N];
    for (i, target) in result.iter_mut().enumerate() {
        let element = sp_element(member, value, i);
        if element.is_null() {
            return Err("Missing CameraInfo matrix element".into());
        }
        *target = *element.cast::<f64>();
    }
    Ok(result)
}

/// Copy the standard CameraInfo message, including complete calibration arrays.
///
/// # Safety
/// `owner` must exclusively own initialized standard sensor_msgs/CameraInfo C
/// storage and matching introspection. Accessor pointers must remain valid for
/// this call. No native pointer or source-owned buffer escapes.
pub unsafe fn native_camera_info(owner: Ptr) -> Result<Calibration, String> {
    if owner.is_null() {
        return Err("Null native CameraInfo owner".into());
    }
    let root = sp_schema(owner, 0);
    let message = sp_message(owner, 0);
    schema(root, message, 11)?;
    let (hm, h) = field(root, message, "header", 18, 0)?;
    let (frame_id, stamp_ns) = header(sp_nested(hm), h)?;
    let (_, model) = field(root, message, "distortion_model", 16, 0)?;
    let (dm, data) = field(root, message, "d", 2, 2)?;
    if sp_bound(dm) != 0 {
        return Err("CameraInfo D must be a double sequence".into());
    }
    let count = sp_length(dm, data);
    if count > MAX_DISTORTION {
        return Err("CameraInfo D exceeds 16 coefficients".into());
    }
    let mut d = Vec::with_capacity(count);
    for i in 0..count {
        let element = sp_element(dm, data, i);
        if element.is_null() {
            return Err("Missing CameraInfo distortion element".into());
        }
        d.push(*element.cast::<f64>());
    }
    let (rm, roi) = field(root, message, "roi", 18, 0)?;
    let rs = sp_nested(rm);
    schema(rs, roi, 5)?;
    let (_, rectify) = field(rs, roi, "do_rectify", 6, 0)?;
    let do_rectify = *rectify.cast::<u8>();
    if do_rectify > 1 {
        return Err("Invalid CameraInfo ROI boolean".into());
    }
    let calibration = Calibration {
        frame_id,
        stamp_ns,
        width: number(root, message, "width")?,
        height: number(root, message, "height")?,
        distortion_model: string_bounded(model, 64)?,
        d,
        k: matrix(root, message, "k")?,
        r: matrix(root, message, "r")?,
        p: matrix(root, message, "p")?,
        binning_x: number(root, message, "binning_x")?,
        binning_y: number(root, message, "binning_y")?,
        roi: RegionOfInterest {
            x_offset: number(rs, roi, "x_offset")?,
            y_offset: number(rs, roi, "y_offset")?,
            width: number(rs, roi, "width")?,
            height: number(rs, roi, "height")?,
            do_rectify: do_rectify != 0,
        },
    };
    calibration.validate()?;
    Ok(calibration)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native_schema::test_support::Fixture;
    use serde_json::json;
    fn message() -> serde_json::Value {
        json!({"header":{"frame_id":"camera_optical","stamp":{"sec":2,"nanosec":3}},
            "width":640,"height":480,"distortion_model":"plumb_bob","d":[0.1,0.2,0.3,0.4,0.5],
            "k":[500.,0.,300.,0.,450.,200.,0.,0.,1.],"r":[1.,0.,0.,0.,1.,0.,0.,0.,1.],
            "p":[500.,0.,300.,-50.,0.,450.,200.,9.,0.,0.,1.,0.],"binning_x":2,"binning_y":2,
            "roi":{"x_offset":20,"y_offset":10,"width":400,"height":300,"do_rectify":false}})
    }
    #[test]
    fn complete_standard_c_storage_is_copied_with_roi_binning_and_stereo() {
        let fixture = Fixture::new("sensor_msgs", "CameraInfo", message());
        let copy = unsafe { native_camera_info(fixture.0) }.unwrap();
        assert_eq!(copy.stamp_ns, 2_000_000_003);
        assert_eq!(copy.frame_id, "camera_optical");
        assert_eq!(copy.d, [0.1, 0.2, 0.3, 0.4, 0.5]);
        assert_eq!(copy.effective().unwrap().size, [200, 150]);
        assert_eq!(
            copy.effective().unwrap().p,
            [250., 0., 140., -25., 0., 225., 95., 4.5, 0., 0., 1., 0.]
        );
        assert_eq!(Calibration::decode(&copy.encode().unwrap()).unwrap(), copy);
        fixture.write(json!({"header":{"stamp":{"sec":0,"nanosec":0}},"d":[]}));
        assert_eq!(
            unsafe { native_camera_info(fixture.0) }.unwrap().stamp_ns,
            0
        );
        assert_eq!(copy.d.len(), 5); // No alias to later C writes.
    }
    #[test]
    fn bounds_invalid_values_and_wrong_storage_reject() {
        let fixture = Fixture::new("sensor_msgs", "CameraInfo", message());
        assert!(unsafe { native_camera_info(fixture.0) }.is_ok());
        fixture.write(json!({"d":vec![0.;MAX_DISTORTION+1]}));
        assert!(unsafe { native_camera_info(fixture.0) }
            .unwrap_err()
            .contains("16 coefficients"));
        fixture.write(json!({"d":[],"header":{"stamp":{"sec":-1}}}));
        assert!(unsafe { native_camera_info(fixture.0) }.is_err());
        fixture.write(json!({"header":{"stamp":{"sec":0}}}));
        unsafe {
            let (member, data) = fixture.root("p", 2, 1);
            *sp_element(member, data, 11).cast::<f64>() = f64::NAN;
            assert!(native_camera_info(fixture.0).is_err());
            assert!(native_camera_info(std::ptr::null_mut()).is_err());
        }
        let wrong = Fixture::new("sensor_msgs", "Image", json!({}));
        assert!(unsafe { native_camera_info(wrong.0) }.is_err());
    }
}
