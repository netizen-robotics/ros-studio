//! Small immutable authorization metadata, independent of coalesced geometry.
use super::*;
use std::collections::BTreeMap;
use studio_protocol::marker::Key;

#[derive(Clone, Debug)]
struct VisibleKey {
    since_revision: u64,
    version: u64,
    deadline: Option<u64>,
}

/// Process-local observation of an accepted Marker state. Capture it with a
/// worker input and compare it with a fresh stream observation before drawing.
/// Ordinary content/TF updates do not revoke an older, still eligible key;
/// disappearance, TF failure or Clear does. Never persist this as authority.
#[derive(Clone, Debug)]
pub struct MarkerVisibility {
    owner: Owner,
    epoch: u64,
    revision: u64,
    keys: BTreeMap<Key, VisibleKey>,
}
impl MarkerVisibility {
    pub(super) fn next(
        previous: Option<&Self>,
        owner: &Owner,
        epoch: u64,
        revision: u64,
        state: &wire::Complete,
        now: u64,
    ) -> Self {
        let previous = previous.filter(|p| p.owner == *owner && p.epoch == epoch);
        let keys = state
            .visible_entries(now)
            .map(|entry| {
                let since_revision = previous
                    .and_then(|p| p.keys.get(&entry.marker.key))
                    .filter(|key| key.deadline.is_none_or(|t| now < t))
                    .map_or(revision, |key| key.since_revision);
                (
                    entry.marker.key.clone(),
                    VisibleKey {
                        since_revision,
                        version: entry.version,
                        deadline: entry.expires_at_monotonic_ns,
                    },
                )
            })
            .collect();
        Self {
            owner: owner.clone(),
            epoch,
            revision,
            keys,
        }
    }
    /// Number of potentially visible keys, at most MAX_MARKERS per stream.
    /// Expiry is checked again by allows, without requiring another ROS sample.
    pub fn key_count(&self) -> usize {
        self.keys.len()
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }
    /// `prepared` must be the observation captured with the worker's input.
    /// `now` uses MarkerStream::monotonic_origin, never the renderer's origin.
    pub fn allows(&self, prepared: &Self, key: &Key, version: u64, now: u64) -> bool {
        self.owner == prepared.owner
            && self.epoch == prepared.epoch
            && prepared.revision <= self.revision
            && prepared.keys.get(key).is_some_and(|p| p.version == version)
            && self.keys.get(key).is_some_and(|current| {
                current.since_revision <= prepared.revision
                    && version <= current.version
                    && current.deadline.is_none_or(|t| now < t)
            })
    }
}
