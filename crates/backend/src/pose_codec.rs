//! Complete pose/covariance extraction from worker-owned standard C messages.
//! Header-frame pose only: Odometry child-frame Twist is outside this codec.
use crate::native_schema::*;
use studio_protocol::{
    pose_snapshot::{Decoded, Kind, Source},
    uncertainty::Covariance,
};

/// Extract PoseWithCovarianceStamped or the pose portion of Odometry.
///
/// # Safety
/// `owner` must own live, initialized, exclusive standard C message storage,
/// loaded with the matching introspection type. Accessor pointers remain valid
/// for the call. No native pointer or borrowed source array escapes.
pub unsafe fn native_pose_covariance(
    owner: Ptr,
    topic: &str,
    message_type: &str,
) -> Result<Decoded, String> {
    let kind = Kind::from_message_type(message_type)?;
    if owner.is_null() || topic.is_empty() || topic.len() > 256 || topic.contains('\0') {
        return Err("Invalid native pose/covariance owner or topic".into());
    }
    let root = sp_schema(owner, 0);
    let message = sp_message(owner, 0);
    schema(
        root,
        message,
        match kind {
            Kind::PoseWithCovariance => 2,
            Kind::Odometry => 4,
        },
    )?;
    let (hm, h) = field(root, message, "header", 18, 0)?;
    let (frame_id, acquisition_stamp_ns) = header(sp_nested(hm), h)?;
    let child_frame_id = if kind == Kind::Odometry {
        let (_, child) = field(root, message, "child_frame_id", 16, 0)?;
        Some(string(child)?)
    } else {
        None
    };
    let (pm, p) = field(root, message, "pose", 18, 0)?;
    let ps = sp_nested(pm);
    schema(ps, p, 2)?;
    let (vm, v) = field(ps, p, "pose", 18, 0)?;
    let components = pose(sp_nested(vm), v)?;
    let (cm, c) = field(ps, p, "covariance", 2, 1)?;
    if sp_bound(cm) != 36 || sp_length(cm, c) != 36 {
        return Err("Native pose covariance requires exactly36 fixed doubles".into());
    }
    let mut values = [0.; 36];
    for (i, value) in values.iter_mut().enumerate() {
        let element = sp_element(cm, c, i);
        if element.is_null() {
            return Err("Missing native covariance element accessor".into());
        }
        *value = *element.cast::<f64>();
    }
    let decoded = Decoded {
        kind,
        topic: topic.into(),
        source: Source {
            frame_id,
            child_frame_id,
        },
        acquisition_stamp_ns,
        components,
        covariance: Covariance::new(values)?,
    };
    decoded.validate()?;
    Ok(decoded)
}
#[cfg(test)]
mod tests;
