use super::{invalid, Error};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// Configuration-only ordering and scene allocation. Activation is never saved.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Layout {
    pub order: Vec<String>,
    pub scene_percent: u8,
}
impl Layout {
    pub fn for_views(views: &[super::ViewConfig]) -> Self {
        Self {
            order: views.iter().map(|v| v.id.clone()).collect(),
            scene_percent: 65,
        }
    }
    pub fn validate(&self, views: &[super::ViewConfig]) -> Result<(), Error> {
        let known: HashSet<_> = views.iter().map(|v| &v.id).collect();
        let ordered: HashSet<_> = self.order.iter().collect();
        if self.order.len() > super::MAX_VISUALIZER_VIEWS
            || self.order.len() != views.len()
            || ordered.len() != self.order.len()
            || known != ordered
        {
            return Err(invalid("layout must reference every display exactly once"));
        }
        if !(35..=80).contains(&self.scene_percent) {
            return Err(invalid("scene allocation must be 35–80 percent"));
        }
        Ok(())
    }
    pub fn reorder(&mut self, id: &str, target: usize) -> Result<(), Error> {
        let index = self
            .order
            .iter()
            .position(|v| v == id)
            .ok_or_else(|| invalid("unknown display"))?;
        if target >= self.order.len() {
            return Err(invalid("invalid display placement"));
        }
        let id = self.order.remove(index);
        self.order.insert(target, id);
        Ok(())
    }
}
