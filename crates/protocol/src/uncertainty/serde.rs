//! Exact fixed-size decoding; never allocate a source-controlled covariance Vec.
use super::Covariance;
use ::serde::{
    de::{Error, IgnoredAny, SeqAccess, Visitor},
    ser::SerializeTuple,
    Deserialize, Deserializer, Serialize, Serializer,
};
use std::fmt;
impl Serialize for Covariance {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut tuple = serializer.serialize_tuple(36)?;
        for value in &self.values {
            tuple.serialize_element(value)?;
        }
        tuple.end()
    }
}
impl<'de> Deserialize<'de> for Covariance {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Matrix;
        impl<'de> Visitor<'de> for Matrix {
            type Value = Covariance;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("exactly36 finite row-major covariance values")
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                if seq.size_hint().is_some_and(|n| n != 36) {
                    return Err(A::Error::custom("Covariance requires exactly36 values"));
                }
                let mut values = [0.; 36];
                for (i, value) in values.iter_mut().enumerate() {
                    *value = seq
                        .next_element()?
                        .ok_or_else(|| A::Error::invalid_length(i, &self))?;
                }
                if seq.next_element::<IgnoredAny>()?.is_some() {
                    return Err(A::Error::invalid_length(37, &self));
                }
                Covariance::new(values).map_err(A::Error::custom)
            }
        }
        deserializer.deserialize_tuple(36, Matrix)
    }
}
