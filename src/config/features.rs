//! Reject unavailable integration keys by presence, before optional decoding.
#[cfg(not(feature = "comb"))]
pub(super) fn check(value: &serde_yaml_ng::Value) -> crate::error::Result<()> {
    fn reject(key: &str) -> crate::error::Error {
        crate::error::Error::config_invalid(match key {
            "comb" => {
                "Configuration key comb requires an Comb-capable Hive build; this is a core \
                    build."
            }
            "comb_collectors" => {
                "Configuration key comb_collectors requires an Comb-capable Hive build; \
                    this is a core build."
            }
            _ => {
                "Role configuration key comb requires an Comb-capable Hive build; this is a \
                    core build."
            }
        })
    }
    if let Some(map) = value.as_mapping() {
        for key in ["comb", "comb_collectors"] {
            if map.contains_key(key) {
                return Err(reject(key));
            }
        }
        if let Some(roles) = map.get("roles").and_then(serde_yaml_ng::Value::as_mapping) {
            for definition in roles.values() {
                if definition
                    .as_mapping()
                    .is_some_and(|map| map.contains_key("comb"))
                {
                    return Err(reject("role.comb"));
                }
            }
        }
    }
    Ok(())
}

#[cfg(all(test, not(feature = "comb")))]
mod tests {
    #[test]
    fn core_rejects_integration_keys_by_presence_before_optional_decoding() {
        for yaml in [
            "comb: null",
            "comb: {}",
            "comb_collectors: null",
            "comb_collectors: {}",
            "roles: {worker: {comb: null}}",
            "roles: {worker: {comb: {}}}",
        ] {
            let error = super::super::Config::parse(yaml).unwrap_err();
            assert_eq!(error.code(), "config_invalid");
            assert!(error.message().contains("Comb-capable"), "{yaml}: {error}");
        }
        assert!(super::super::Config::parse("auth_profiles: {}\nroles: {}").is_ok());
    }
}
