//! Check sequence bounds during deserialization, before retaining excess items.
use serde::{
    de::{Error, IgnoredAny, SeqAccess, Visitor},
    Deserialize, Deserializer,
};
use std::{fmt, marker::PhantomData};
fn bounded<'de, D, T, const MAX: usize>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    struct Values<T, const MAX: usize>(PhantomData<T>);
    impl<'de, T: Deserialize<'de>, const MAX: usize> Visitor<'de> for Values<T, MAX> {
        type Value = Vec<T>;
        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "at most {MAX} items")
        }
        fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
            if seq.size_hint().is_some_and(|count| count > MAX) {
                return Err(A::Error::custom("Sequence exceeds its declared limit"));
            }
            let mut values = Vec::with_capacity(seq.size_hint().unwrap_or(0).min(MAX));
            for _ in 0..MAX {
                match seq.next_element()? {
                    Some(value) => values.push(value),
                    None => return Ok(values),
                }
            }
            if seq.next_element::<IgnoredAny>()?.is_some() {
                return Err(A::Error::custom("Sequence exceeds its declared limit"));
            }
            Ok(values)
        }
    }
    deserializer.deserialize_seq(Values::<T, MAX>(PhantomData))
}
macro_rules! sequence {
    ($name:ident,$maximum:expr) => {
        pub(crate) fn $name<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
        where
            D: Deserializer<'de>,
            T: Deserialize<'de>,
        {
            bounded::<D, T, { $maximum }>(deserializer)
        }
    };
}
sequence!(points, crate::marker::MAX_POINTS);
sequence!(entries, crate::marker::MAX_MARKERS);
sequence!(diagnostics, crate::marker::MAX_DIAGNOSTICS);
sequence!(operations, crate::marker::MAX_OPERATIONS);
sequence!(pose_history, crate::pose_snapshot::MAX_HISTORY);
sequence!(distortion, crate::camera_info::MAX_DISTORTION);

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    static DECODED: AtomicUsize = AtomicUsize::new(0);
    #[derive(Debug)]
    struct Item;
    impl<'de> Deserialize<'de> for Item {
        fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
            let _ = u8::deserialize(d)?;
            DECODED.fetch_add(1, Ordering::Relaxed);
            Ok(Self)
        }
    }
    #[derive(Deserialize)]
    struct Message {
        #[serde(deserialize_with = "entries")]
        items: Vec<Item>,
    }
    #[test]
    fn excess_items_are_discarded_without_constructing_more_retained_elements() {
        let bytes = serde_json::to_vec(
            &serde_json::json!({"items":vec![0;crate::marker::MAX_MARKERS+1000]}),
        )
        .unwrap();
        DECODED.store(0, Ordering::Relaxed);
        assert!(serde_json::from_slice::<Message>(&bytes).is_err());
        assert_eq!(DECODED.load(Ordering::Relaxed), crate::marker::MAX_MARKERS);
        let valid: Message = serde_json::from_str("{\"items\":[0,0]}").unwrap();
        assert_eq!(valid.items.len(), 2);
    }
}
