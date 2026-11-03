//! Native topic ownership for dynamic-message layouts unsupported by rclrs views.
use serde_json::Value;
use std::ffi::{c_char, c_void, CStr, CString};
extern "C" {
    #[link_name = "sp_schema"]
    fn native_schema_ptr(p: *mut c_void, response: i32) -> *mut c_void;
    #[link_name = "sp_message"]
    fn native_message_ptr(p: *mut c_void, response: i32) -> *mut c_void;
    fn tp_serialized_size(p: *mut c_void, size: *mut usize) -> i32;
    fn tp_create() -> *mut c_void;
    fn tp_destroy(p: *mut c_void);
    fn tp_open(
        p: *mut c_void,
        intro: *const c_char,
        dispatch: *const c_char,
        package: *const c_char,
        name: *const c_char,
        identifier: *const c_char,
        best: i32,
        topic: *const c_char,
        domain: usize,
        transient: i32,
        echo: i32,
    ) -> i32;
    fn tp_echo(p: *mut c_void, timeout_ns: i64) -> i32;
    fn tp_enable_loss(p: *mut c_void) -> i32;
    fn tp_loss_count(p: *mut c_void, total: *mut u64) -> i32;
    fn tp_publication(
        p: *mut c_void,
        publisher: *mut c_char,
        capacity: usize,
        sequence: *mut u64,
    ) -> i32;
    fn sp_error(p: *mut c_void) -> *const c_char;
}
/// Created, polled and destroyed on one backend worker thread.
/// Raw pointer intentionally prevents implicit Send/Sync implementations.
pub struct NativeTopic {
    handle: *mut c_void,
    image: bool,
    camera_info: bool,
    description: bool,
    cloud: bool,
    map_update: bool,
    marker_type: Option<&'static str>,
    pose_kind: Option<studio_protocol::pose_snapshot::Kind>,
    loss_observer: Option<bool>,
    lost_total: u64,
    mapping_type: Option<&'static str>,
    topic: String,
}
/// Local-context publisher identity and sequence for the last successfully taken
/// sample. Sequence support is middleware-specific; the first sample cannot
/// establish whether anything was missed before joining.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Publication {
    pub publisher_id: String,
    pub sequence: Option<u64>,
}
impl Drop for NativeTopic {
    fn drop(&mut self) {
        unsafe { tp_destroy(self.handle) };
    }
}
impl NativeTopic {
    fn check(&self, code: i32) -> Result<i32, String> {
        if code < 0 {
            Err(unsafe { CStr::from_ptr(sp_error(self.handle)) }
                .to_string_lossy()
                .into_owned())
        } else {
            Ok(code)
        }
    }
    pub fn open(
        topic: &str,
        typ: &str,
        best: bool,
        transient: bool,
        echo: bool,
    ) -> Result<Self, String> {
        Self::open_with_bundle(topic, typ, best, transient, echo, "")
    }
    pub fn open_with_bundle(
        topic: &str,
        typ: &str,
        best: bool,
        transient: bool,
        echo: bool,
        bundle: &str,
    ) -> Result<Self, String> {
        let parts: Vec<_> = typ.split('/').collect();
        if parts.len() != 3
            || parts[1] != "msg"
            || parts
                .iter()
                .any(|s| s.is_empty() || !s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
        {
            return Err("Expected PACKAGE/msg/TYPE".into());
        }
        let domain = std::env::var("ROS_DOMAIN_ID")
            .unwrap_or_else(|_| "0".into())
            .parse::<usize>()
            .map_err(|e| e.to_string())?;
        let rmw = std::env::var("RMW_IMPLEMENTATION").map_err(|e| e.to_string())?;
        let identifier = match rmw.as_str() {
            "rmw_fastrtps_cpp" => "rosidl_typesupport_fastrtps_c",
            "rmw_cyclonedds_cpp" => "rosidl_typesupport_introspection_c",
            _ => return Err("Native topics currently support Fast DDS and Cyclone DDS".into()),
        };
        let prefixes = if bundle.is_empty() {
            std::env::split_paths(
                &std::env::var_os("AMENT_PREFIX_PATH").ok_or("Missing runtime prefix")?,
            )
            .collect::<Vec<_>>()
        } else {
            vec![crate::probe_bundle::bundle(
                std::path::Path::new(bundle),
                parts[0],
            )?]
        };
        let prefix = prefixes
            .into_iter()
            .find(|p| {
                crate::probe_bundle::library(p, parts[0], "rosidl_typesupport_introspection_c")
                    .is_file()
            })
            .ok_or("Message introspection library missing from runtime prefixes")?;
        let strings = [
            crate::probe_bundle::library(&prefix, parts[0], "rosidl_typesupport_introspection_c")
                .to_string_lossy()
                .into_owned(),
            crate::probe_bundle::library(&prefix, parts[0], identifier)
                .to_string_lossy()
                .into_owned(),
            parts[0].into(),
            parts[2].into(),
            identifier.into(),
            topic.into(),
        ]
        .into_iter()
        .map(|s| CString::new(s).map_err(|e| e.to_string()))
        .collect::<Result<Vec<_>, _>>()?;
        let owner = Self {
            handle: unsafe { tp_create() },
            image: typ == studio_protocol::image::MESSAGE_TYPE,
            camera_info: typ == studio_protocol::camera_info::MESSAGE_TYPE,
            description: typ == studio_protocol::robot_description::MESSAGE_TYPE,
            cloud: typ == studio_protocol::cloud::MESSAGE_TYPE,
            map_update: typ == studio_protocol::map_grid::UPDATE_MESSAGE_TYPE,
            marker_type: match typ {
                studio_protocol::marker::MESSAGE_TYPE => {
                    Some(studio_protocol::marker::MESSAGE_TYPE)
                }
                studio_protocol::marker::ARRAY_MESSAGE_TYPE => {
                    Some(studio_protocol::marker::ARRAY_MESSAGE_TYPE)
                }
                _ => None,
            },
            loss_observer: None,
            pose_kind: studio_protocol::pose_snapshot::Kind::from_message_type(typ).ok(),
            lost_total: 0,
            mapping_type: match typ {
                studio_protocol::mapping::MAP_MESSAGE_TYPE => {
                    Some(studio_protocol::mapping::MAP_MESSAGE_TYPE)
                }
                studio_protocol::mapping::POSE_MESSAGE_TYPE => {
                    Some(studio_protocol::mapping::POSE_MESSAGE_TYPE)
                }
                studio_protocol::mapping::PATH_MESSAGE_TYPE => {
                    Some(studio_protocol::mapping::PATH_MESSAGE_TYPE)
                }
                _ => None,
            },
            topic: topic.into(),
        };
        if owner.handle.is_null() {
            return Err("Native allocation failed".into());
        }
        owner.check(unsafe {
            tp_open(
                owner.handle,
                strings[0].as_ptr(),
                strings[1].as_ptr(),
                strings[2].as_ptr(),
                strings[3].as_ptr(),
                strings[4].as_ptr(),
                best.into(),
                strings[5].as_ptr(),
                domain,
                transient.into(),
                echo.into(),
            )
        })?;
        Ok(owner)
    }
    pub fn poll_value(&mut self) -> Result<Option<Value>, String> {
        if self.check(unsafe { tp_echo(self.handle, 0) })? == 0 {
            return Ok(None);
        }
        unsafe {
            crate::service_codec::native_read(
                sp_schema_for_tools(self.handle),
                sp_message_for_tools(self.handle),
            )
        }
        .map(Some)
    }
    pub fn poll_statistics(&mut self) -> Result<Option<usize>, String> {
        if self.check(unsafe { tp_echo(self.handle, 0) })? == 0 {
            return Ok(None);
        }
        let mut bytes = 0;
        self.check(unsafe { tp_serialized_size(self.handle, &mut bytes) })?;
        Ok(Some(bytes))
    }
    pub fn poll(&mut self) -> Result<Option<Value>, String> {
        if self.check(unsafe { tp_echo(self.handle, 100_000_000) })? == 0 {
            return Ok(None);
        }
        unsafe { crate::service_codec::native_preview(self.handle) }.map(Some)
    }
    /// Take one sample and convert it directly from native metadata.
    pub fn poll_typed(&mut self) -> Result<Option<studio_protocol::structured::Value>, String> {
        self.poll_typed_timeout(100_000_000)
    }
    /// Nonblocking group workers pass zero; legacy polling retains its100ms wait.
    pub fn poll_typed_timeout(
        &mut self,
        timeout_ns: i64,
    ) -> Result<Option<studio_protocol::structured::Value>, String> {
        if !(0..=100_000_000).contains(&timeout_ns) {
            return Err("Invalid topic polling timeout".into());
        }
        if self.check(unsafe { tp_echo(self.handle, timeout_ns) })? == 0 {
            return Ok(None);
        }
        unsafe { crate::service_codec::native_typed_value(self.handle) }.map(Some)
    }
    /// Take and copy a compact complete Image on this owning worker. Available
    /// only when opened with sensor_msgs/msg/Image; no session wire change.
    pub fn poll_description(&mut self) -> Result<Option<String>, String> {
        if !self.description {
            return Err("Description requires std_msgs/msg/String".into());
        }
        let count = self.check(unsafe { tp_echo(self.handle, 0) })?;
        if count == 0 {
            return Ok(None);
        }
        unsafe { crate::description_codec::native_description(self.handle) }.map(Some)
    }
    pub fn poll_image(&mut self) -> Result<Option<studio_protocol::image::ImageSample>, String> {
        if !self.image {
            return Err("Compact Image polling requires sensor_msgs/msg/Image".into());
        }
        if self.check(unsafe { tp_echo(self.handle, 100_000_000) })? == 0 {
            return Ok(None);
        }
        unsafe { crate::image_codec::native_image(self.handle, &self.topic) }.map(Some)
    }
    /// Nonblocking shared-scene input; malformed content is separate from a
    /// failing native reader so paired Camera owners can show recovery states.
    pub fn poll_image_input(
        &mut self,
    ) -> Result<Option<Result<studio_protocol::image::ImageSample, String>>, String> {
        if !self.image {
            return Err("Camera Image polling requires sensor_msgs/msg/Image".into());
        }
        if self.check(unsafe { tp_echo(self.handle, 0) })? == 0 {
            return Ok(None);
        }
        Ok(Some(unsafe {
            crate::image_codec::native_image(self.handle, &self.topic)
        }))
    }
    pub fn poll_camera_info(
        &mut self,
    ) -> Result<Option<Result<studio_protocol::camera_info::Calibration, String>>, String> {
        if !self.camera_info {
            return Err("CameraInfo polling requires sensor_msgs/msg/CameraInfo".into());
        }
        if self.check(unsafe { tp_echo(self.handle, 0) })? == 0 {
            return Ok(None);
        }
        Ok(Some(unsafe {
            crate::camera_info_codec::native_camera_info(self.handle)
        }))
    }
    /// Nonblocking bounded cloud extraction on the shared scene owner thread.
    pub fn poll_cloud(&mut self) -> Result<Option<studio_protocol::cloud::DecodedCloud>, String> {
        if !self.cloud {
            return Err("PointCloud2 polling requires sensor_msgs/msg/PointCloud2".into());
        }
        if self.check(unsafe { tp_echo(self.handle, 0) })? == 0 {
            return Ok(None);
        }
        unsafe { crate::cloud_codec::native_cloud(self.handle, &self.topic) }.map(Some)
    }
    /// Complete bounded mapping extraction on the shared actor thread.
    pub fn poll_mapping(&mut self) -> Result<Option<studio_protocol::mapping::Decoded>, String> {
        let typ = self
            .mapping_type
            .ok_or("Mapping polling requires a standard mapping type")?;
        if self.check(unsafe { tp_echo(self.handle, 0) })? == 0 {
            return Ok(None);
        }
        unsafe { crate::mapping_codec::native_mapping(self.handle, &self.topic, typ) }.map(Some)
    }
    /// Complete raw grid extraction on the owning worker, without wire admission.
    pub fn poll_grid(&mut self) -> Result<Option<studio_protocol::map_grid::Grid>, String> {
        self.poll_grid_input()?.transpose()
    }
    pub fn poll_markers(
        &mut self,
    ) -> Result<Option<Result<studio_protocol::marker::Batch, String>>, String> {
        let message_type = self
            .marker_type
            .ok_or("Marker polling requires Marker or MarkerArray")?;
        if self.check(unsafe { tp_echo(self.handle, 0) })? == 0 {
            return Ok(None);
        }
        Ok(Some(unsafe {
            crate::marker_codec::native_markers(self.handle, message_type)
        }))
    }
    /// Nonblocking complete pose extraction. Invalid source data is distinct
    /// from a native take failure so the scene owner can retire only its source.
    pub fn poll_pose_covariance(
        &mut self,
    ) -> Result<Option<Result<studio_protocol::pose_snapshot::Decoded, String>>, String> {
        let kind = self
            .pose_kind
            .ok_or("Pose covariance polling requires an exact standard pose type")?;
        if self.check(unsafe { tp_echo(self.handle, 0) })? == 0 {
            return Ok(None);
        }
        Ok(Some(unsafe {
            crate::pose_codec::native_pose_covariance(self.handle, &self.topic, kind.message_type())
        }))
    }
    /// Separate a malformed source sample from a failing native reader.
    pub fn poll_grid_input(
        &mut self,
    ) -> Result<Option<Result<studio_protocol::map_grid::Grid, String>>, String> {
        if self.mapping_type != Some(studio_protocol::mapping::MAP_MESSAGE_TYPE) {
            return Err("Grid polling requires nav_msgs/msg/OccupancyGrid".into());
        }
        if self.check(unsafe { tp_echo(self.handle, 0) })? == 0 {
            return Ok(None);
        }
        Ok(Some(unsafe {
            crate::mapping_codec::native_grid(self.handle)
        }))
    }
    /// Take one update in arrival order; callers must apply it before coalescing.
    pub fn poll_grid_update(&mut self) -> Result<Option<studio_protocol::map_grid::Patch>, String> {
        self.poll_grid_update_input()?.transpose()
    }
    pub fn poll_grid_update_input(
        &mut self,
    ) -> Result<Option<Result<studio_protocol::map_grid::Patch, String>>, String> {
        if !self.map_update {
            return Err("Grid updates require map_msgs/msg/OccupancyGridUpdate".into());
        }
        if self.check(unsafe { tp_echo(self.handle, 0) })? == 0 {
            return Ok(None);
        }
        Ok(Some(unsafe {
            crate::mapping_codec::native_grid_update(self.handle)
        }))
    }
    /// Opt in to middleware message-lost events. Support does not imply that
    /// all history eviction or late-join gaps can be observed.
    pub fn enable_message_loss_events(&mut self) -> Result<bool, String> {
        if let Some(supported) = self.loss_observer {
            return Ok(supported);
        }
        let supported = self.check(unsafe { tp_enable_loss(self.handle) })? != 0;
        self.loss_observer = Some(supported);
        Ok(supported)
    }
    /// Cumulative reported loss, retaining the previous count when no event is
    /// available. None means explicitly unsupported; never infer gap-free data.
    pub fn message_loss_count(&mut self) -> Result<Option<u64>, String> {
        match self.loss_observer {
            None => Err("Enable message-lost observation before polling it".into()),
            Some(false) => Ok(None),
            Some(true) => {
                let mut total = 0;
                if self.check(unsafe { tp_loss_count(self.handle, &mut total) })? != 0 {
                    self.lost_total = self.lost_total.max(total);
                }
                Ok(Some(self.lost_total))
            }
        }
    }
    pub fn publication(&self) -> Result<Publication, String> {
        let mut publisher = [0 as c_char; 129];
        let mut sequence = 0;
        let supported = self.check(unsafe {
            tp_publication(
                self.handle,
                publisher.as_mut_ptr(),
                publisher.len(),
                &mut sequence,
            )
        })? != 0;
        Ok(Publication {
            publisher_id: unsafe { CStr::from_ptr(publisher.as_ptr()) }
                .to_string_lossy()
                .into_owned(),
            sequence: supported.then_some(sequence),
        })
    }
}

unsafe fn sp_schema_for_tools(p: *mut c_void) -> *mut c_void {
    native_schema_ptr(p, 0)
}
unsafe fn sp_message_for_tools(p: *mut c_void) -> *mut c_void {
    native_message_ptr(p, 0)
}
