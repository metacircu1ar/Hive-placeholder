//! Auth source references; configuration may omit fields, persisted records may not.
use super::{Method, Profile, Provider, invalid, valid_env_name};
use serde::{Deserialize, Deserializer, Serialize};
use std::path::{Component, PathBuf};

impl Method {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Subscription => "subscription",
            Self::ApiKey => "api_key",
        }
    }
}

impl Profile {
    pub(crate) fn validate(&self) -> crate::error::Result<()> {
        match (&self.method, &self.api_key_env, &self.api_key_file) {
            (Method::Subscription, None, None) => Ok(()),
            (Method::ApiKey, Some(name), None) if valid_env_name(name) => Ok(()),
            (Method::ApiKey, None, Some(path))
                if !path.as_os_str().is_empty()
                    && path.file_name().is_some()
                    && !path
                        .components()
                        .any(|part| matches!(part, Component::ParentDir)) =>
            {
                Ok(())
            }
            _ => Err(invalid()),
        }
    }
}

pub(crate) fn stored_profile<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Profile, D::Error> {
    use crate::runtime::domain::decode::{object, string_enum};
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Fields {
        #[serde(deserialize_with = "string_enum")]
        provider: Provider,
        #[serde(deserialize_with = "string_enum")]
        method: Method,
        #[serde(deserialize_with = "serde::Deserialize::deserialize")]
        api_key_env: Option<String>,
        #[serde(deserialize_with = "serde::Deserialize::deserialize")]
        api_key_file: Option<PathBuf>,
    }
    let fields: Fields = object(deserializer)?;
    let profile = Profile {
        provider: fields.provider,
        method: fields.method,
        api_key_env: fields.api_key_env,
        api_key_file: fields.api_key_file,
    };
    if profile.validate().is_err()
        || profile
            .api_key_file
            .as_ref()
            .is_some_and(|path| !path.is_absolute())
    {
        return Err(serde::de::Error::custom(
            "Invalid persisted authentication source.",
        ));
    }
    Ok(profile)
}

/// Full digest identity; a shortened display value is never a compatibility key.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct KeyFingerprint(String);
impl KeyFingerprint {
    pub(crate) fn digest(bytes: &[u8]) -> Self {
        use sha2::Digest;
        Self(format!("{:x}", sha2::Sha256::digest(bytes)))
    }
}

impl<'de> Deserialize<'de> for KeyFingerprint {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        if value.len() != 64
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(serde::de::Error::custom("Invalid key fingerprint."));
        }
        Ok(Self(value))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    #[test]
    fn key_source_configuration_is_exclusive_and_resolved_once() {
        for fields in [
            "method: subscription, api_key_file: key",
            "method: api_key",
            "method: api_key, api_key_env: KEY, api_key_file: key",
            "method: api_key, api_key_file: ''",
            "method: api_key, api_key_file: ../key",
        ] {
            assert!(
                Config::parse(&format!(
                    "auth_profiles: {{main: {{provider: codex, {fields}}}}}"
                ))
                .is_err()
            );
        }
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let path = root.join("config.yaml");
        std::fs::write(
            &path,
            "auth_profiles: {main: {provider: codex, method: api_key, api_key_file: key}}\n",
        )
        .unwrap();
        let config = Config::load(&path).unwrap();
        assert_eq!(
            config.profile("main").unwrap().api_key_file,
            Some(root.join("key"))
        );
    }

    #[test]
    fn persisted_profile_and_key_identity_require_exact_current_shape() {
        #[derive(Deserialize)]
        struct Record {
            #[serde(deserialize_with = "stored_profile")]
            profile: Profile,
        }
        let profile = Profile {
            provider: Provider::Codex,
            method: Method::Subscription,
            api_key_env: None,
            api_key_file: None,
        };
        let value = serde_json::to_value(&profile).unwrap();
        let record: Record = serde_json::from_value(serde_json::json!({"profile":value})).unwrap();
        assert_eq!(record.profile, profile);
        for key in ["provider", "method", "api_key_env", "api_key_file"] {
            let mut missing = value.clone();
            missing.as_object_mut().unwrap().remove(key);
            assert!(
                serde_json::from_value::<Record>(serde_json::json!({"profile":missing})).is_err()
            );
        }
        for text in ["", "abcdefabcdef", &"G".repeat(64), &"a".repeat(63)] {
            assert!(serde_json::from_value::<KeyFingerprint>(serde_json::json!(text)).is_err());
        }
        assert!(
            serde_json::from_value::<KeyFingerprint>(serde_json::json!("a".repeat(64))).is_ok()
        );
    }
}
