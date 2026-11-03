//! Strict map view extension. Prior adapters deliberately cannot deserialize it.
use super::*;
#[derive(Serialize, Deserialize)]
#[serde(untagged)]
pub(super) enum View {
    Map(MapView),
    Previous(v11::View),
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct MapView {
    id: String,
    topic: String,
    message_type: String,
    qos: Qos,
    kind: MapKind,
}
fn required_option<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
    d: D,
) -> Result<Option<T>, D::Error> {
    Option::<T>::deserialize(d)
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "renderer", rename_all = "snake_case", deny_unknown_fields)]
enum MapKind {
    Map {
        fixed_frame: String,
        transforms: SpatialTransforms,
        clock_expectation: ClockExpectation,
        #[serde(deserialize_with = "required_option")]
        updates: Option<crate::map::UpdateSource>,
        appearance: crate::map::Appearance,
    },
}
impl From<&ViewConfig> for View {
    fn from(view: &ViewConfig) -> Self {
        if let ViewKind::Map {
            fixed_frame,
            transforms,
            clock_expectation,
            updates,
            appearance,
        } = &view.kind
        {
            Self::Map(MapView {
                id: view.id.clone(),
                topic: view.topic.clone(),
                message_type: view.message_type.clone(),
                qos: view.qos.clone(),
                kind: MapKind::Map {
                    fixed_frame: fixed_frame.clone(),
                    transforms: transforms.clone(),
                    clock_expectation: *clock_expectation,
                    updates: updates.clone(),
                    appearance: *appearance,
                },
            })
        } else {
            Self::Previous(v11::View::from(view))
        }
    }
}
impl From<View> for ViewConfig {
    fn from(view: View) -> Self {
        match view {
            View::Previous(old) => old.into(),
            View::Map(view) => {
                let MapKind::Map {
                    fixed_frame,
                    transforms,
                    clock_expectation,
                    updates,
                    appearance,
                } = view.kind;
                Self {
                    id: view.id,
                    topic: view.topic,
                    message_type: view.message_type,
                    qos: view.qos,
                    kind: ViewKind::Map {
                        fixed_frame,
                        transforms,
                        clock_expectation,
                        updates,
                        appearance,
                    },
                }
            }
        }
    }
}
pub(super) fn migrate_map(view: &mut ViewConfig) -> bool {
    if let ViewKind::OccupancyGrid {
        fixed_frame,
        transforms,
        clock_expectation,
    } = &view.kind
    {
        view.kind = ViewKind::Map {
            fixed_frame: fixed_frame.clone(),
            transforms: transforms.clone(),
            clock_expectation: *clock_expectation,
            updates: None,
            appearance: Default::default(),
        };
        true
    } else {
        false
    }
}
