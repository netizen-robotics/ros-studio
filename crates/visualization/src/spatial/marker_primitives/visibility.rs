//! Local presentation scopes, never serialized as source or ROS authority.
use super::*;
pub const UNKEYED: u16 = u16::MAX;
pub const MAX_KEYS: usize = 2 * studio_protocol::marker::MAX_MARKERS;
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyIdentity {
    pub owner: u64,
    pub source_epoch: u64,
    pub scene_epoch: u64,
    pub asset_epoch: Option<u64>,
    pub key: studio_protocol::marker::Key,
    pub version: u64,
}
#[derive(Clone, Debug)]
pub struct Permit {
    frame: Weak<Sealed>,
    allowed: [u32; 16],
}
/// One frame and its local presentation decision, borrowed together for a draw.
#[derive(Clone, Copy, Debug)]
pub struct MarkerLayer<'a> {
    pub(crate) frame: &'a ReadyFrame,
    pub(crate) permit: Option<&'a Permit>,
}
impl<'a> MarkerLayer<'a> {
    pub fn new(frame: &'a ReadyFrame, permit: &'a Permit) -> Result<Self, String> {
        permit.validate_for(frame)?;
        Ok(Self {
            frame,
            permit: Some(permit),
        })
    }
    pub(crate) fn unkeyed(frame: &'a ReadyFrame) -> Self {
        Self {
            frame,
            permit: None,
        }
    }
}
impl Permit {
    pub(crate) fn allows(&self, slot: u16) -> bool {
        slot == UNKEYED
            || ((slot as usize) < MAX_KEYS
                && self.allowed[slot as usize / 32] & (1 << (slot % 32)) != 0)
    }
    pub fn validate_for(&self, frame: &ReadyFrame) -> Result<(), String> {
        if self
            .frame
            .upgrade()
            .is_some_and(|old| Arc::ptr_eq(&old, &frame.0))
        {
            Ok(())
        } else {
            Err("Marker visibility permit belongs to another frame".into())
        }
    }
}
impl ReadyFrame {
    /// Bounded metadata only. The caller compares identities to its current
    /// source/asset/scene ledger, including Clear/Stop/TF availability.
    pub fn permit(&self, mut current: impl FnMut(&KeyIdentity) -> bool) -> Permit {
        let mut allowed = [0u32; 16];
        for (i, key) in self.keys.iter().enumerate() {
            if current(key) {
                allowed[i / 32] |= 1 << (i % 32);
            }
        }
        Permit {
            frame: Arc::downgrade(&self.0),
            allowed,
        }
    }
    pub fn has_visible(&self, permit: Option<&Permit>) -> bool {
        if permit.is_some_and(|p| p.validate_for(self).is_err()) {
            return false;
        }
        self.0.has_unkeyed
            || self
                .0
                .used_keys
                .iter()
                .enumerate()
                .any(|(i, word)| *word & permit.map_or(0, |p| p.allowed[i]) != 0)
    }
    pub fn opaque_visible(
        &self,
        permit: Option<&Permit>,
    ) -> impl Iterator<Item = (Kind, Range<u32>)> + '_ {
        let permitted = permit
            .filter(|p| p.validate_for(self).is_ok())
            .map(|p| p.allowed);
        let mut pending: Option<(Kind, Range<u32>)> = None;
        let mut i = 0;
        std::iter::from_fn(move || {
            while i < self.0.opaque.len() {
                let slot = self.0.opaque_keys[i];
                let (kind, range) = &self.0.opaque[i];
                i += 1;
                if slot != UNKEYED
                    && permitted
                        .is_none_or(|bits| bits[slot as usize / 32] & (1 << (slot % 32)) == 0)
                {
                    continue;
                }
                if let Some((old, run)) = &mut pending {
                    if old == kind && run.end == range.start {
                        run.end = range.end;
                        continue;
                    }
                }
                let previous = pending.replace((*kind, range.clone()));
                if previous.is_some() {
                    return previous;
                }
            }
            pending.take()
        })
    }
}
impl Frame {
    pub fn owned(clock: MarkerClock) -> Self {
        Self {
            require_keys: true,
            ..Self::with_clock(clock)
        }
    }
    /// One key's body/geometry is atomic. A cancelled/failed append cannot leave
    /// records, stale texture references or a half-registered identity behind.
    pub fn append_key<T>(
        &mut self,
        key: KeyIdentity,
        build: impl FnOnce(&mut Self) -> Result<T, String>,
    ) -> Result<T, String> {
        key.key
            .validate()
            .map_err(|_| "Invalid Marker key identity")?;
        if key.owner == 0
            || key.source_epoch == 0
            || key.scene_epoch == 0
            || key.version == 0
            || key.asset_epoch == Some(0)
            || self.current_key.is_some()
            || self.keys.len() >= MAX_KEYS
        {
            return Err("Invalid Marker key scope/limit".into());
        }
        for old in &self.keys {
            if old.scene_epoch != key.scene_epoch
                || (old.owner == key.owner
                    && (old.source_epoch != key.source_epoch
                        || old.key == key.key
                        || (old.asset_epoch.is_some()
                            && key.asset_epoch.is_some()
                            && old.asset_epoch != key.asset_epoch)))
            {
                return Err("Marker frame mixes source/key scopes".into());
            }
        }
        if !self.keys.iter().any(|old| old.owner == key.owner) {
            let owners = self
                .keys
                .iter()
                .map(|k| k.owner)
                .collect::<std::collections::BTreeSet<_>>();
            if owners.len() >= 2 {
                return Err("Marker owner limit".into());
            }
        }
        let start = self.len();
        let old_projection = self.projection.clone();
        let glyphs = self.glyph_count;
        let meshes = self.mesh_count;
        let images = self.textures.len();
        let atlas = self.atlas.clone();
        self.current_key = Some(self.keys.len() as u16);
        self.keys.push(key);
        let result = build(self);
        self.current_key = None;
        if result.is_err() {
            self.instances.truncate(start);
            self.kinds.truncate(start);
            self.key_slots.truncate(start);
            self.projection = old_projection;
            self.glyph_count = glyphs;
            self.mesh_count = meshes;
            self.textures.truncate(images);
            self.atlas = atlas;
        }
        if result.is_err() || self.len() == start {
            self.keys.pop();
        }
        result
    }
}
