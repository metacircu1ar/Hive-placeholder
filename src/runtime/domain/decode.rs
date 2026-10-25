//! Named JSON records and literal enums for `Hive`'s own stored data.
use serde::{
    Deserialize, Deserializer,
    de::{DeserializeOwned, MapAccess, Visitor, value::MapAccessDeserializer},
};
use std::{fmt, marker::PhantomData};

pub(crate) fn object<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    struct Object<T>(PhantomData<T>);
    impl<'de, T: Deserialize<'de>> Visitor<'de> for Object<T> {
        type Value = T;
        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("a named JSON object")
        }
        fn visit_map<M: MapAccess<'de>>(self, map: M) -> Result<T, M::Error> {
            T::deserialize(MapAccessDeserializer::new(map))
        }
    }
    deserializer.deserialize_map(Object(PhantomData))
}

pub(crate) fn optional_object<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    struct Optional<T>(PhantomData<T>);
    impl<'de, T: Deserialize<'de>> Visitor<'de> for Optional<T> {
        type Value = Option<T>;
        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("null or a named JSON object")
        }
        fn visit_none<E: serde::de::Error>(self) -> Result<Self::Value, E> {
            Ok(None)
        }
        fn visit_unit<E: serde::de::Error>(self) -> Result<Self::Value, E> {
            Ok(None)
        }
        fn visit_some<D: Deserializer<'de>>(
            self,
            deserializer: D,
        ) -> Result<Self::Value, D::Error> {
            object(deserializer).map(Some)
        }
    }
    deserializer.deserialize_option(Optional(PhantomData))
}

pub(crate) fn string_enum<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: DeserializeOwned,
{
    struct Literal<T>(PhantomData<T>);
    impl<T: DeserializeOwned> Visitor<'_> for Literal<T> {
        type Value = T;
        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("a literal enum string")
        }
        fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<T, E> {
            T::deserialize(serde::de::value::StrDeserializer::new(value))
        }
    }
    deserializer.deserialize_str(Literal(PhantomData))
}

pub(crate) fn from_str<T: DeserializeOwned>(text: &str) -> serde_json::Result<T> {
    let mut deserializer = serde_json::Deserializer::from_str(text);
    let value = object(&mut deserializer)?;
    deserializer.end()?;
    Ok(value)
}

pub(super) use crate::config::stored_profile as profile;

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::runtime::{
        auth_fallback::{Availability, SwitchIntent},
        branching::{AttemptedTurn, FinishedTurn, Intent},
        domain::{OptionsSelection, ReservedTurn},
    };
    use serde_json::{Value, json};

    fn strict<T: DeserializeOwned>(good: Value, fields: &[&str]) {
        object::<_, T>(&good).unwrap();
        for key in good.as_object().unwrap().keys() {
            let mut missing = good.clone();
            missing.as_object_mut().unwrap().remove(key);
            assert!(object::<_, T>(&missing).is_err(), "missing {key}");
        }
        let mut unknown = good.clone();
        unknown["unknown"] = json!(true);
        assert!(object::<_, T>(&unknown).is_err());
        let positional = Value::Array(fields.iter().map(|key| good[key].clone()).collect());
        assert!(object::<_, T>(&positional).is_err());
    }

    #[test]
    fn persisted_nested_records_require_current_named_fields() {
        strict::<OptionsSelection>(
            json!({"model":"mock","effort":"low","revision":0,"options":{"values":{},"sources":{}}}),
            &["model", "effort", "revision", "options"],
        );
        strict::<ReservedTurn>(
            json!({"message_id":"m","selection":{"model":"mock","effort":"low","revision":0,"options":{"values":{},"sources":{}}}}),
            &["message_id", "selection"],
        );
        strict::<FinishedTurn>(
            json!({"thread":"t","execution":"e","turn":"u","message":null,"outcome":"completed","at":1}),
            &["thread", "execution", "turn", "message", "outcome", "at"],
        );
        strict::<AttemptedTurn>(
            json!({"message":"m","thread":"t","execution":"e"}),
            &["message", "thread", "execution"],
        );
        strict::<SwitchIntent>(
            json!({"target":"login","reason":"usage_limit","source_generation":uuid::Uuid::nil(),"source_execution":null}),
            &["target", "reason", "source_generation", "source_execution"],
        );
        strict::<Intent>(
            json!({"source":"a","source_thread":"t","source_execution":"e","source_turn":"u",
            "phase":"bound","deadline":null,"source_outcome":null,"source_handoff":null,"native_thread":null,"error":null}),
            &[
                "source",
                "source_thread",
                "source_execution",
                "source_turn",
                "phase",
                "deadline",
                "source_outcome",
                "source_handoff",
                "native_thread",
                "error",
            ],
        );
        let availability =
            serde_json::to_value(Availability::available("main", uuid::Uuid::nil())).unwrap();
        strict::<Availability>(
            availability.clone(),
            &[
                "profile",
                "generation",
                "revision",
                "state",
                "until",
                "retry_at",
                "probe_id",
                "probe_agent",
                "probe_message",
                "probe_deadline",
                "probe_error",
                "window",
                "reason",
                "confirmed_at",
                "updated_at",
            ],
        );
        for state in [json!("future"), json!({"available":null})] {
            let mut bad = availability.clone();
            bad["state"] = state;
            assert!(object::<_, Availability>(&bad).is_err());
        }
    }

    #[test]
    fn nested_structs_and_enums_reject_positional_and_object_encodings() {
        assert!(
            object::<_, ReservedTurn>(&json!({"message_id":"m","selection":["mock","low",0]}))
                .is_err()
        );
        for phase in [json!("future"), json!({"bound":null})] {
            let value = json!({"source":"a","source_thread":"t","source_execution":"e","source_turn":"u",
                "phase":phase,"deadline":null,"source_outcome":null,"source_handoff":null,"native_thread":null,"error":null});
            assert!(object::<_, Intent>(&value).is_err());
        }
    }
}
