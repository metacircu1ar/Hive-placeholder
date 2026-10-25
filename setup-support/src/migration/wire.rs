//! Require named JSON objects, including nested records, without discarding
//! duplicate fields before the derived strict reader sees them.
use serde::{Deserialize, Deserializer, de};
use std::{fmt, marker::PhantomData};

struct Object<T>(T);
impl<'de, T: Deserialize<'de>> Deserialize<'de> for Object<T> {
    fn deserialize<D: Deserializer<'de>>(decoder: D) -> Result<Self, D::Error> {
        struct Visitor<T>(PhantomData<T>);
        impl<'de, T: Deserialize<'de>> de::Visitor<'de> for Visitor<T> {
            type Value = Object<T>;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a named object")
            }
            fn visit_map<M: de::MapAccess<'de>>(self, map: M) -> Result<Self::Value, M::Error> {
                T::deserialize(de::value::MapAccessDeserializer::new(map)).map(Object)
            }
        }
        decoder.deserialize_map(Visitor(PhantomData))
    }
}

pub(super) fn optional_object<'de, D: Deserializer<'de>, T: Deserialize<'de>>(
    decoder: D,
) -> Result<Option<T>, D::Error> {
    Option::<Object<T>>::deserialize(decoder).map(|v| v.map(|Object(value)| value))
}

pub(super) fn object<'de, D: Deserializer<'de>, T: Deserialize<'de>>(
    decoder: D,
) -> Result<T, D::Error> {
    Object::<T>::deserialize(decoder).map(|Object(value)| value)
}

pub(super) fn objects<'de, D: Deserializer<'de>, T: Deserialize<'de>>(
    decoder: D,
) -> Result<Vec<T>, D::Error> {
    Vec::<Object<T>>::deserialize(decoder)
        .map(|v| v.into_iter().map(|Object(value)| value).collect())
}
