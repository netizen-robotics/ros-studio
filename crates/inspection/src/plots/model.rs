use std::{collections::VecDeque, time::Duration};

pub const MAX_POINTS: usize = 10_000;
pub const WINDOW: Duration = Duration::from_secs(60);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PathSegment {
    Field(String),
    Index(usize),
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FieldPath(pub Vec<PathSegment>);
impl FieldPath {
    pub fn label(&self) -> String {
        let mut label = String::from("$");
        for part in &self.0 {
            match part {
                PathSegment::Field(name) => label.push_str(&format!("[{name:?}]")),
                PathSegment::Index(index) => label.push_str(&format!("[{index}]")),
            }
        }
        label
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Scalar {
    Signed(i64),
    Unsigned(u64),
    Float(f64),
}
impl Scalar {
    pub fn exact_label(self) -> String {
        match self {
            Self::Signed(n) => n.to_string(),
            Self::Unsigned(n) => n.to_string(),
            Self::Float(n) => n.to_string(),
        }
    }
    pub fn coordinate(self) -> Result<f64, Gap> {
        match self {
            Self::Signed(n) => {
                let value = n as f64;
                if value as i128 == i128::from(n) {
                    Ok(value)
                } else {
                    Err(Gap::PrecisionLoss)
                }
            }
            Self::Unsigned(n) => {
                let value = n as f64;
                if value as u128 == u128::from(n) {
                    Ok(value)
                } else {
                    Err(Gap::PrecisionLoss)
                }
            }
            Self::Float(n) if n.is_finite() => Ok(n),
            Self::Float(_) => Err(Gap::NonFinite),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gap {
    Missing,
    Nonnumeric,
    Incomplete,
    NonFinite,
    PrecisionLoss,
    Paused,
    BackwardReceiptTime,
}
impl Gap {
    pub fn label(self) -> &'static str {
        match self {
            Self::Missing => "Selected field is missing",
            Self::Nonnumeric => "Selected field is not numeric",
            Self::Incomplete => "Selected value is truncated or unavailable",
            Self::NonFinite => "Value is NaN or infinity",
            Self::PrecisionLoss => "Integer cannot be plotted exactly as f64",
            Self::Paused => "Samples skipped while locally paused",
            Self::BackwardReceiptTime => "Receipt clock moved backwards; sample rejected",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Sample {
    pub receipt: Duration,
    pub value: Result<Scalar, Gap>,
}
#[derive(Clone, Debug)]
pub struct Point {
    pub receipt: Duration,
    pub value: Option<f64>,
    pub exact: Option<Scalar>,
    pub gap: Option<Gap>,
}
#[derive(Default, Clone, Debug)]
pub struct Counters {
    pub evicted: u64,
    pub skipped_paused: u64,
    pub stale: u64,
    pub backward: u64,
}
#[derive(Debug, Default)]
pub struct Trace {
    points: VecDeque<Point>,
    generation: u64,
    paused: bool,
    break_next: Option<Gap>,
    last_receipt: Option<Duration>,
    pub counters: Counters,
}
impl Trace {
    /// Called on subscription replacement/reconnect. Old generations cannot append.
    pub fn begin(&mut self, generation: u64) {
        *self = Self {
            generation,
            ..Self::default()
        };
    }
    pub fn clear(&mut self) {
        self.points.clear();
        self.counters.evicted = 0;
        // Preserve the receipt watermark and pause state; delayed samples stay rejected.
        self.break_next = None;
    }
    pub fn set_paused(&mut self, paused: bool) {
        if self.paused && !paused && self.counters.skipped_paused > 0 {
            self.break_next = Some(Gap::Paused);
        }
        self.paused = paused;
    }
    pub fn paused(&self) -> bool {
        self.paused
    }
    pub fn points(&self) -> &VecDeque<Point> {
        &self.points
    }
    pub fn push(&mut self, generation: u64, sample: Sample) {
        if generation != self.generation {
            self.counters.stale = self.counters.stale.saturating_add(1);
            return;
        }
        if self.last_receipt.is_some_and(|last| sample.receipt < last) {
            self.counters.backward = self.counters.backward.saturating_add(1);
            self.break_next = Some(Gap::BackwardReceiptTime);
            return;
        }
        self.last_receipt = Some(sample.receipt);
        if self.paused {
            self.counters.skipped_paused = self.counters.skipped_paused.saturating_add(1);
            return;
        }
        if let Some(reason) = self.break_next {
            self.points.push_back(Point {
                receipt: sample.receipt,
                value: None,
                exact: None,
                gap: Some(reason),
            });
            self.break_next = None;
        }
        let exact = sample.value.ok();
        let coordinate = sample.value.and_then(Scalar::coordinate);
        self.points.push_back(Point {
            receipt: sample.receipt,
            value: coordinate.ok(),
            exact,
            gap: coordinate.err(),
        });
        while self.points.len() > MAX_POINTS
            || self
                .points
                .front()
                .is_some_and(|p| sample.receipt.saturating_sub(p.receipt) > WINDOW)
        {
            self.points.pop_front();
            self.counters.evicted = self.counters.evicted.saturating_add(1);
        }
    }
    pub fn extents(&self) -> Option<(f64, f64)> {
        self.points
            .iter()
            .filter_map(|p| p.value)
            .fold(None, |range, v| {
                Some(match range {
                    None => (v, v),
                    Some((min, max)) => (min.min(v), max.max(v)),
                })
            })
    }
    /// At most four vertices per horizontal bucket. Gap-containing buckets are
    /// suppressed, so downsampling never connects across unavailable samples.
    pub fn display_points(&self, buckets: usize) -> Vec<Option<(Duration, f64)>> {
        let buckets = buckets.clamp(1, 2048);
        let Some(first) = self.points.front() else {
            return Vec::new();
        };
        let span = self
            .points
            .back()
            .unwrap()
            .receipt
            .saturating_sub(first.receipt)
            .as_secs_f64();
        let mut groups: Vec<Vec<&Point>> = vec![Vec::new(); buckets];
        for point in &self.points {
            let x = point.receipt.saturating_sub(first.receipt).as_secs_f64();
            let bucket = if span == 0.0 {
                0
            } else {
                ((x / span * buckets as f64) as usize).min(buckets - 1)
            };
            groups[bucket].push(point);
        }
        let mut out = Vec::with_capacity(buckets * 4);
        for group in groups {
            if group.is_empty() {
                continue;
            }
            if group.iter().any(|p| p.value.is_none()) {
                out.push(None);
                continue;
            }
            let min = group
                .iter()
                .enumerate()
                .min_by(|(_, a), (_, b)| a.value.unwrap().total_cmp(&b.value.unwrap()))
                .unwrap()
                .0;
            let max = group
                .iter()
                .enumerate()
                .max_by(|(_, a), (_, b)| a.value.unwrap().total_cmp(&b.value.unwrap()))
                .unwrap()
                .0;
            let mut indexes = vec![0, min, max, group.len() - 1];
            indexes.sort_unstable();
            indexes.dedup();
            for i in indexes {
                out.push(Some((group[i].receipt, group[i].value.unwrap())));
            }
        }
        out
    }
}
