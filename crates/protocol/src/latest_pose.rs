//! Shared latest-available native TF result. Each result has its own timestamp;
//! this type does not imply acquisition-time alignment or clock authorization.
use serde::{Deserialize, Serialize};
pub const MAX_STAMP_NS: i64 = i32::MAX as i64 * 1_000_000_000 + 999_999_999;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LookupTime {
    At(i64),
    Latest,
}
impl LookupTime {
    pub fn from_stamp(stamp_ns: i64) -> Result<Self, String> {
        match stamp_ns {
            0 => Ok(Self::Latest),
            n if (1..=MAX_STAMP_NS).contains(&n) => Ok(Self::At(n)),
            _ => Err("Invalid TF acquisition timestamp".into()),
        }
    }
    pub fn validate_result(self, result: &LatestPose) -> Result<(), String> {
        result.validate()?;
        if let Self::At(wanted) = self {
            if !(1..=MAX_STAMP_NS).contains(&wanted) {
                return Err("Invalid TF acquisition timestamp".into());
            }
            if let LatestPose::Ready {
                resolved_stamp_ns, ..
            } = result
            {
                if *resolved_stamp_ns != wanted {
                    return Err("Acquisition TF must resolve at the exact requested stamp".into());
                }
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum LatestPose {
    Ready {
        /// Zero is valid for identity/static-only paths.
        resolved_stamp_ns: i64,
        components: [f64; 7],
    },
    Missing,
    Stale,
    Extrapolated,
    LookupTimeout,
}
impl LatestPose {
    pub fn validate(&self) -> Result<(), String> {
        if let Self::Ready {
            resolved_stamp_ns,
            components,
        } = self
        {
            let norm = components[3..].iter().map(|v| v * v).sum::<f64>();
            if !(0..=MAX_STAMP_NS).contains(resolved_stamp_ns)
                || components.iter().any(|v| !v.is_finite())
                || components[..3].iter().any(|v| v.abs() > 1_000_000.)
                || (norm - 1.).abs() > 1e-6
            {
                return Err("Invalid latest native TF pose or resolved timestamp".into());
            }
        }
        Ok(())
    }
}
