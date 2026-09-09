use serde::Deserializer;
use serde::de::{self, Deserialize, MapAccess, SeqAccess, Visitor};
use std::fmt;
use std::marker::PhantomData;

// serde's derived struct visitor also accepts positional JSON arrays. Our wire
// records are objects only; force map entry before delegating field validation.
pub(crate) fn object<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    struct ObjectVisitor<T>(PhantomData<T>);
    impl<'de, T: Deserialize<'de>> Visitor<'de> for ObjectVisitor<T> {
        type Value = T;
        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("a JSON object record")
        }
        fn visit_map<M: MapAccess<'de>>(self, map: M) -> Result<T, M::Error> {
            T::deserialize(de::value::MapAccessDeserializer::new(map))
        }
    }
    deserializer.deserialize_map(ObjectVisitor(PhantomData))
}

pub(crate) fn objects<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    struct Object<T>(T);
    impl<'de, T: Deserialize<'de>> Deserialize<'de> for Object<T> {
        fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
            object(d).map(Self)
        }
    }
    struct ObjectsVisitor<T>(PhantomData<T>);
    impl<'de, T: Deserialize<'de>> Visitor<'de> for ObjectsVisitor<T> {
        type Value = Vec<T>;
        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("an array of JSON object records")
        }
        fn visit_seq<S: SeqAccess<'de>>(self, mut seq: S) -> Result<Vec<T>, S::Error> {
            let mut result = Vec::new();
            while let Some(Object(value)) = seq.next_element()? {
                if result.len() == crate::contract::CASE_CAP {
                    return Err(de::Error::custom("too many object records"));
                }
                result.push(value);
            }
            Ok(result)
        }
    }
    deserializer.deserialize_seq(ObjectsVisitor(PhantomData))
}

// Unit enum derives also accept {"variant":null}; the wire schema requires strings.
pub(crate) fn string_enum<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    struct StringVisitor<T>(PhantomData<T>);
    impl<'de, T: Deserialize<'de>> Visitor<'de> for StringVisitor<T> {
        type Value = T;
        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("an enum string")
        }
        fn visit_str<E: de::Error>(self, value: &str) -> Result<T, E> {
            T::deserialize(de::value::StrDeserializer::new(value))
        }
    }
    deserializer.deserialize_str(StringVisitor(PhantomData))
}

// Option<T> derives accept positional/map spellings for the inner value; require
// null or the closed object/string form.
pub(crate) fn nullable_object<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    struct Nullable<T>(PhantomData<T>);
    impl<'de, T: Deserialize<'de>> Visitor<'de> for Nullable<T> {
        type Value = Option<T>;
        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("null or a JSON object record")
        }
        fn visit_none<E: de::Error>(self) -> Result<Option<T>, E> {
            Ok(None)
        }
        fn visit_unit<E: de::Error>(self) -> Result<Option<T>, E> {
            Ok(None)
        }
        fn visit_some<D2: Deserializer<'de>>(self, d: D2) -> Result<Option<T>, D2::Error> {
            object(d).map(Some)
        }
    }
    deserializer.deserialize_option(Nullable(PhantomData))
}

pub(crate) fn nullable_string_enum<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    struct Nullable<T>(PhantomData<T>);
    impl<'de, T: Deserialize<'de>> Visitor<'de> for Nullable<T> {
        type Value = Option<T>;
        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("null or an enum string")
        }
        fn visit_none<E: de::Error>(self) -> Result<Option<T>, E> {
            Ok(None)
        }
        fn visit_unit<E: de::Error>(self) -> Result<Option<T>, E> {
            Ok(None)
        }
        fn visit_some<D2: Deserializer<'de>>(self, d: D2) -> Result<Option<T>, D2::Error> {
            string_enum(d).map(Some)
        }
    }
    deserializer.deserialize_option(Nullable(PhantomData))
}

// Profile-gated controls are optional strictly by absence: a present field must
// carry its real declaration and an explicit null is a loud spelling error, so
// readers without these fields and readers with them reject the same records.
// Some(value) always means declared; None only ever means absent.
pub(crate) fn present_string_enum<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    string_enum(deserializer).map(Some)
}

pub(crate) fn present_bool<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<bool>, D::Error> {
    bool::deserialize(deserializer).map(Some)
}

pub(crate) fn parse<T: de::DeserializeOwned>(bytes: &[u8]) -> Result<T, serde_json::Error> {
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let result = object(&mut deserializer)?;
    deserializer.end()?;
    Ok(result)
}

// Nonempty, bounded arrays reject excess elements before allocating their value.
#[derive(Debug, serde::Serialize, PartialEq, Eq)]
#[serde(transparent)]
pub(crate) struct Bounded<T, const N: usize>(pub Vec<T>);

impl<'de, T: Deserialize<'de>, const N: usize> Deserialize<'de> for Bounded<T, N> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct BoundedVisitor<T, const N: usize>(PhantomData<T>);
        impl<'de, T: Deserialize<'de>, const N: usize> Visitor<'de> for BoundedVisitor<T, N> {
            type Value = Bounded<T, N>;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "an array of 1..={N} elements")
            }
            fn visit_seq<S: SeqAccess<'de>>(self, mut seq: S) -> Result<Self::Value, S::Error> {
                let mut values = Vec::new();
                while values.len() < N {
                    match seq.next_element()? {
                        Some(value) => values.push(value),
                        None if values.is_empty() => return Err(de::Error::custom("empty array")),
                        None => return Ok(Bounded(values)),
                    }
                }
                if seq.next_element::<de::IgnoredAny>()?.is_some() {
                    return Err(de::Error::custom("array exceeds element limit"));
                }
                Ok(Bounded(values))
            }
        }
        d.deserialize_seq(BoundedVisitor::<T, N>(PhantomData))
    }
}
