//! Attribute layout availability is explicit; values align with retained XYZ.
use super::{Field, Metadata, MAX_SOURCE_POINTS};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "status",
    content = "values",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum Channel<T> {
    #[default]
    Missing,
    Unsupported,
    Values(Vec<T>),
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Channels {
    pub color: Channel<[u8; 4]>,
    pub intensity: Channel<Option<f64>>,
}
impl Channels {
    /// Hide geometry without misreporting the source channel layout as missing.
    pub fn empty_values(&self) -> Self {
        fn empty<T>(channel: &Channel<T>) -> Channel<T> {
            match channel {
                Channel::Missing => Channel::Missing,
                Channel::Unsupported => Channel::Unsupported,
                Channel::Values(_) => Channel::Values(Vec::new()),
            }
        }
        Self {
            color: empty(&self.color),
            intensity: empty(&self.intensity),
        }
    }
    pub fn validate(&self, points: usize) -> Result<(), String> {
        if points > MAX_SOURCE_POINTS
            || matches!(&self.color, Channel::Values(v) if v.len()!=points)
            || matches!(&self.intensity, Channel::Values(v) if v.len()!=points || v.iter().flatten().any(|n|!n.is_finite()))
        {
            return Err("Cloud attributes must be finite where available and align exactly with retained XYZ".into());
        }
        Ok(())
    }
}
enum Layout<'a> {
    Missing,
    Unsupported,
    Field(&'a Field),
}
impl Layout<'_> {
    fn channel<T>(&self) -> Channel<T> {
        match self {
            Self::Missing => Channel::Missing,
            Self::Unsupported => Channel::Unsupported,
            Self::Field(_) => Channel::Values(Vec::new()),
        }
    }
}
pub(super) struct Plan<'a> {
    color: Layout<'a>,
    intensity: Layout<'a>,
}
impl<'a> Plan<'a> {
    pub fn new(metadata: &'a Metadata) -> Self {
        let find = |name| metadata.fields.iter().find(|f| f.name == name);
        let packed = |field: &&Field| field.count == 1 && matches!(field.datatype, 6 | 7);
        let color = find("rgba")
            .filter(packed)
            .or_else(|| find("rgb").filter(packed));
        let color = color.map_or_else(
            || {
                if metadata
                    .fields
                    .iter()
                    .any(|f| matches!(f.name.as_str(), "rgb" | "rgba" | "r" | "g" | "b"))
                {
                    Layout::Unsupported
                } else {
                    Layout::Missing
                }
            },
            Layout::Field,
        );
        let intensity = match find("intensity") {
            None => Layout::Missing,
            Some(f) if f.count == 1 && (1..=8).contains(&f.datatype) => Layout::Field(f),
            _ => Layout::Unsupported,
        };
        Self { color, intensity }
    }
    pub fn channels(&self) -> Channels {
        Channels {
            color: self.color.channel(),
            intensity: self.intensity.channel(),
        }
    }
    /// Metadata and raw row lengths were checked before this per-point read.
    pub fn push(&self, channels: &mut Channels, raw: &[u8], big: bool) {
        if let (Layout::Field(field), Channel::Values(values)) = (&self.color, &mut channels.color)
        {
            let bytes = raw[field.offset as usize..field.offset as usize + 4]
                .try_into()
                .unwrap();
            let bits = if big {
                u32::from_be_bytes(bytes)
            } else {
                u32::from_le_bytes(bytes)
            };
            values.push([
                (bits >> 16) as u8,
                (bits >> 8) as u8,
                bits as u8,
                if field.name == "rgba" {
                    (bits >> 24) as u8
                } else {
                    255
                },
            ]);
        }
        if let (Layout::Field(field), Channel::Values(values)) =
            (&self.intensity, &mut channels.intensity)
        {
            let raw = &raw[field.offset as usize..];
            macro_rules! number {
                ($ty:ty,$len:expr) => {{
                    let bytes = raw[..$len].try_into().unwrap();
                    if big {
                        <$ty>::from_be_bytes(bytes)
                    } else {
                        <$ty>::from_le_bytes(bytes)
                    }
                }};
            }
            let value = match field.datatype {
                1 => f64::from(raw[0] as i8),
                2 => f64::from(raw[0]),
                3 => f64::from(number!(i16, 2)),
                4 => f64::from(number!(u16, 2)),
                5 => f64::from(number!(i32, 4)),
                6 => f64::from(number!(u32, 4)),
                7 => f64::from(number!(f32, 4)),
                8 => number!(f64, 8),
                _ => unreachable!("validated scalar intensity"),
            };
            values.push(value.is_finite().then_some(value));
        }
    }
}
