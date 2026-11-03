use super::{Point3, RigidTransform};
use std::sync::atomic::{AtomicU64, Ordering};
// One bounded process-wide scalar avoids token reuse even across new Scene values.
static NEXT_LAYER_EPOCH: AtomicU64 = AtomicU64::new(1);
pub const MAX_LAYERS: usize = 4;
pub const MAX_POINTS: usize = 4096;
const MAX_FRAME_NAME: usize = 256;
const MAX_COORDINATE: f64 = 1_000_000.0;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Clock {
    System,
    Simulated,
    Reset,
}
/// The TF engine owns freshness/graph validation. Ready is its verified lookup
/// result at exactly the scan stamp, not a latest-transform shortcut.
pub enum TfOutcome<'a> {
    Ready {
        source: &'a str,
        target: &'a str,
        stamp_ns: i64,
        transform: RigidTransform,
    },
    Missing,
    Stale,
    Extrapolated,
    InvalidGraph,
    CyclicGraph,
    OutOfOrder,
}
pub struct Scan<'a> {
    pub frame: &'a str,
    pub stamp_ns: i64,
    pub clock: Clock,
    pub complete: bool,
    pub angle_min: f64,
    pub angle_increment: f64,
    pub range_min: f64,
    pub range_max: f64,
    pub ranges: &'a [f32],
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SceneError {
    InvalidFrame,
    NoFixedFrame,
    LayerLimit,
    DuplicateLayer,
    UnknownLayer,
    InvalidScan,
    IncompleteScan,
    PointLimit,
    UnsupportedClock,
    ClockReset,
    MissingTf,
    StaleTf,
    ExtrapolatedTf,
    InvalidTfGraph,
    CyclicTfGraph,
    OutOfOrder,
    TfMismatch,
    InvalidPoint,
    StaleGeneration,
    StaleLayer,
    Stopped,
    EpochExhausted,
}
impl std::fmt::Display for SceneError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            match self {
                Self::InvalidFrame =>
                    "Choose a nonempty frame name (maximum 256 bytes, no whitespace).",
                Self::NoFixedFrame => "Choose a fixed frame before projecting scans.",
                Self::LayerLimit => "Scene layer budget reached (four scan layers).",
                Self::DuplicateLayer => "Layer identifier is already configured.",
                Self::UnknownLayer => "Scan layer is not configured.",
                Self::InvalidScan => "Invalid scan angles, range bounds or timestamp.",
                Self::IncompleteScan =>
                    "Incomplete scan rejected; complete range arrays are required.",
                Self::PointLimit => "Scan exceeds the 4096-ray budget.",
                Self::UnsupportedClock => "Simulated time is unsupported in this slice.",
                Self::ClockReset => "Clock reset requires an explicit stop and restart.",
                Self::MissingTf => "Waiting for a transform into the fixed frame.",
                Self::StaleTf => "Transform is stale; scan is hidden.",
                Self::ExtrapolatedTf => "Transform unavailable at the scan timestamp.",
                Self::InvalidTfGraph => "TF engine reported an invalid graph.",
                Self::CyclicTfGraph => "TF engine reported a cyclic graph.",
                Self::OutOfOrder => "Out-of-order scan or transform rejected.",
                Self::TfMismatch =>
                    "Transform source, target or timestamp does not match the scan.",
                Self::StaleGeneration => "Old scene generation rejected.",
                Self::StaleLayer => "Old layer activation rejected.",
                Self::Stopped =>
                    "Scene is stopped; explicitly start before obtaining a layer token.",
                Self::EpochExhausted => "Scene activation counter exhausted; restart the app.",
                Self::InvalidPoint => "Transformed coordinates exceed the finite scene budget.",
            }
        )
    }
}
impl std::error::Error for SceneError {}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LayerStatus {
    Stopped,
    Waiting,
    Receiving,
    Error(SceneError),
}
/// Opaque worker activation identity. Retain this exact token with queued data
/// and errors; never look up a replacement token while delivering old work.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LayerToken {
    generation: u64,
    id: u64,
    epoch: u64,
}
impl LayerToken {
    pub fn layer_id(self) -> u64 {
        self.id
    }
}
pub struct Layer {
    epoch: u64,
    pub id: u64,
    pub visible: bool,
    status: LayerStatus,
    points: Vec<Point3>,
    last_stamp: Option<i64>,
    pub skipped_rays: usize,
}
impl Layer {
    pub fn status(&self) -> &LayerStatus {
        &self.status
    }
    pub fn points(&self) -> &[Point3] {
        &self.points
    }
    fn clear(&mut self, status: LayerStatus) {
        self.points.clear();
        self.skipped_rays = 0;
        self.status = status;
    }
}
#[derive(Default)]
pub struct Scene {
    fixed_frame: Option<String>,
    layers: Vec<Layer>,
    active: bool,
    reset_required: bool,
    generation: u64,
}
fn valid_frame(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= MAX_FRAME_NAME
        && !s.chars().any(char::is_whitespace)
        && !s.chars().any(char::is_control)
}
impl Scene {
    pub fn fixed_frame(&self) -> Option<&str> {
        self.fixed_frame.as_deref()
    }
    pub fn layers(&self) -> &[Layer] {
        &self.layers
    }
    pub fn is_active(&self) -> bool {
        self.active
    }
    /// Configuration changes clear data and stop; caller must explicitly start.
    pub fn set_fixed_frame(&mut self, name: &str) -> Result<(), SceneError> {
        if !valid_frame(name) {
            return Err(SceneError::InvalidFrame);
        }
        self.stop();
        self.fixed_frame = Some(name.to_owned());
        Ok(())
    }
    pub fn add_layer(&mut self, id: u64) -> Result<(), SceneError> {
        if self.layers.iter().any(|l| l.id == id) {
            return Err(SceneError::DuplicateLayer);
        }
        if self.layers.len() >= MAX_LAYERS {
            return Err(SceneError::LayerLimit);
        }
        let epoch = NEXT_LAYER_EPOCH
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |epoch| {
                epoch.checked_add(1)
            })
            .map_err(|_| SceneError::EpochExhausted)?;
        self.layers.push(Layer {
            epoch,
            id,
            visible: true,
            status: LayerStatus::Stopped,
            points: Vec::new(),
            last_stamp: None,
            skipped_rays: 0,
        });
        Ok(())
    }
    /// Query only when starting that layer worker, not for each incoming event.
    pub fn layer_token(&self, id: u64) -> Result<LayerToken, SceneError> {
        if !self.active {
            return Err(SceneError::Stopped);
        }
        let layer = self
            .layers
            .iter()
            .find(|layer| layer.id == id)
            .ok_or(SceneError::UnknownLayer)?;
        Ok(LayerToken {
            generation: self.generation,
            id,
            epoch: layer.epoch,
        })
    }
    pub fn remove_layer(&mut self, id: u64) {
        self.layers.retain(|layer| layer.id != id);
    }
    pub fn set_visible(&mut self, id: u64, visible: bool) {
        if let Some(l) = self.layers.iter_mut().find(|l| l.id == id) {
            l.visible = visible;
        }
    }
    pub fn start(&mut self) -> Result<u64, SceneError> {
        if self.fixed_frame.is_none() {
            return Err(SceneError::NoFixedFrame);
        }
        if self.reset_required {
            return Err(SceneError::ClockReset);
        }
        self.generation = self
            .generation
            .checked_add(1)
            .ok_or(SceneError::EpochExhausted)?;
        self.active = true;
        for l in &mut self.layers {
            l.last_stamp = None;
            l.skipped_rays = 0;
            l.clear(LayerStatus::Waiting);
        }
        Ok(self.generation)
    }
    pub fn stop(&mut self) {
        self.active = false;
        self.reset_required = false;
        for l in &mut self.layers {
            l.last_stamp = None;
            l.skipped_rays = 0;
            l.clear(LayerStatus::Stopped);
        }
    }
    pub fn update(
        &mut self,
        token: LayerToken,
        scan: Scan<'_>,
        tf: TfOutcome<'_>,
    ) -> Result<(), SceneError> {
        if token.generation != self.generation {
            return Err(SceneError::StaleGeneration);
        }
        let Some(index) = self.layers.iter().position(|l| l.id == token.id) else {
            return Err(SceneError::UnknownLayer);
        };
        if self.layers[index].epoch != token.epoch {
            return Err(SceneError::StaleLayer);
        }
        if !self.active {
            return Ok(());
        } // old worker cannot repopulate stopped scene
        let result = self.project(index, &scan, tf);
        match result {
            Ok((points, skipped)) => {
                let l = &mut self.layers[index];
                l.points = points;
                l.skipped_rays = skipped;
                l.last_stamp = Some(scan.stamp_ns);
                l.status = LayerStatus::Receiving;
                Ok(())
            }
            Err(error) => {
                self.layers[index].clear(LayerStatus::Error(error.clone()));
                if error == SceneError::ClockReset {
                    self.reset_required = true;
                    self.active = false;
                    for l in &mut self.layers {
                        l.clear(LayerStatus::Error(SceneError::ClockReset));
                    }
                }
                Err(error)
            }
        }
    }
    fn project(
        &self,
        index: usize,
        scan: &Scan<'_>,
        tf: TfOutcome<'_>,
    ) -> Result<(Vec<Point3>, usize), SceneError> {
        match scan.clock {
            Clock::System => (),
            Clock::Simulated => return Err(SceneError::UnsupportedClock),
            Clock::Reset => return Err(SceneError::ClockReset),
        }
        if !scan.complete {
            return Err(SceneError::IncompleteScan);
        }
        if scan.ranges.len() > MAX_POINTS {
            return Err(SceneError::PointLimit);
        }
        if !valid_frame(scan.frame)
            || scan.stamp_ns <= 0
            || scan.ranges.is_empty()
            || !scan.angle_min.is_finite()
            || !scan.angle_increment.is_finite()
            || scan.angle_increment <= 0.0
            || !(scan.angle_min
                + (scan.ranges.len().saturating_sub(1)) as f64 * scan.angle_increment)
                .is_finite()
            || !scan.range_min.is_finite()
            || !scan.range_max.is_finite()
            || scan.range_min < 0.0
            || scan.range_max <= scan.range_min
            || scan.range_max > MAX_COORDINATE
        {
            return Err(SceneError::InvalidScan);
        }
        if self.layers[index]
            .last_stamp
            .is_some_and(|last| scan.stamp_ns <= last)
        {
            return Err(SceneError::OutOfOrder);
        }
        let transform = match tf {
            TfOutcome::Ready {
                source,
                target,
                stamp_ns,
                transform,
            } => {
                if source != scan.frame
                    || Some(target) != self.fixed_frame()
                    || stamp_ns != scan.stamp_ns
                {
                    return Err(SceneError::TfMismatch);
                }
                transform
            }
            TfOutcome::Missing => return Err(SceneError::MissingTf),
            TfOutcome::Stale => return Err(SceneError::StaleTf),
            TfOutcome::Extrapolated => return Err(SceneError::ExtrapolatedTf),
            TfOutcome::InvalidGraph => return Err(SceneError::InvalidTfGraph),
            TfOutcome::CyclicGraph => return Err(SceneError::CyclicTfGraph),
            TfOutcome::OutOfOrder => return Err(SceneError::OutOfOrder),
        };
        let mut points = Vec::with_capacity(scan.ranges.len());
        let mut skipped = 0;
        for (i, &range) in scan.ranges.iter().enumerate() {
            let range = f64::from(range);
            if !range.is_finite() || range < scan.range_min || range > scan.range_max {
                skipped += 1;
                continue;
            }
            let angle = scan.angle_min + i as f64 * scan.angle_increment;
            let point = transform.apply(Point3([range * angle.cos(), range * angle.sin(), 0.0]));
            if point
                .0
                .iter()
                .any(|v| !v.is_finite() || v.abs() > MAX_COORDINATE)
            {
                return Err(SceneError::InvalidPoint);
            }
            points.push(point);
        }
        Ok((points, skipped))
    }
}
