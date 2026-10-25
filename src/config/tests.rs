use super::*;

#[test]
fn spawn_widening_is_opt_in_and_strictly_boolean() {
    for text in [
        "auth_profiles: {}",
        "auth_profiles: {}\nruntime: {}",
        "auth_profiles: {}\nruntime: {allow_spawn_widening: false}",
    ] {
        assert!(!Config::parse(text).unwrap().runtime.allow_spawn_widening);
    }
    let config = Config::parse("auth_profiles: {}\nruntime: {allow_spawn_widening: true}").unwrap();
    assert!(config.runtime.allow_spawn_widening);
    assert_eq!(
        serde_json::to_value(config.runtime).unwrap()["allow_spawn_widening"],
        true
    );
    for value in ["null", "1", "'true'", "[]", "{}"] {
        assert_eq!(
            Config::parse(&format!(
                "auth_profiles: {{}}\nruntime: {{allow_spawn_widening: {value}}}"
            ))
            .unwrap_err()
            .code(),
            "config_invalid"
        );
    }
}

#[test]
fn logging_configuration_is_strict_and_bounded() {
    for settings in [
        "{level: verbose}",
        "{level: null}",
        "{files: 1}",
        "{files: 65}",
        "{file_bytes: 12287}",
        "{file_bytes: 1073741825}",
        "{unknown: true}",
        "{file_bytes: -1}",
        "{files: 2.5}",
    ] {
        assert_eq!(
            Config::parse(&format!("auth_profiles: {{}}\nlogging: {settings}\n"))
                .unwrap_err()
                .code(),
            "config_invalid"
        );
    }
    let config =
        Config::parse("auth_profiles: {}\nlogging: {level: debug, files: 3, file_bytes: 16384}\n")
            .unwrap();
    let logging = config.logging.backend().unwrap();
    assert_eq!(logging.level, tracing::level_filters::LevelFilter::DEBUG);
    assert_eq!((logging.files, logging.file_bytes), (3, 16384));
}
#[test]
fn million_agent_capacity_is_valid_but_bounded() {
    assert!(Config::parse("auth_profiles: {}\nruntime: {max_agents: 1000000}\n").is_ok());
    assert!(Config::parse("auth_profiles: {}\nruntime: {max_agents: 1000001}\n").is_err());
}

#[test]
fn strict_configuration() {
    assert!(
        Config::parse(
            "auth_profiles:\n  main:\n    provider: codex\n    method: \
        subscription\n"
        )
        .is_ok()
    );
    for bad in [
        "auth_profiles: {}\nauth_profiles: {}",
        "auth_profiles:\n  a: {provider: codex, method: subscription}\n  \
             a: {provider: claude, method: subscription}",
        "auth_profiles:\n  a: {provider: codex, provider: claude, \
            method: subscription}",
        "auth_profiles: &a {}",
        "auth_profiles: !thing {}",
        "auth_profiles: {a: {provider: codex, method: subscription, \
            api_key_env: SECRET}}",
        "auth_profiles: {a: {provider: claude, method: api_key, \
            api_key_env: '1KEY'}}",
        "auth_profiles: {}\ngraphs: {}",
        "version: 1\nauth_profiles: {}",
        "roles: {}",
        "auth_profiles: {a: {provider: unknown, method: subscription}}",
        "auth_profiles: {a: {provider: codex, method: unknown}}",
        "auth_profiles: {a: {<<: {}, provider: codex, method: \
            subscription}}",
    ] {
        assert_eq!(
            Config::parse(bad).unwrap_err().code(),
            "config_invalid",
            "accepted {bad}"
        );
    }
}
