use super::{Context, Failure, Result};
use comb::setup::{Options, Prepared};

pub(in crate::cli) fn prepare(options: Options) -> Result<Prepared> {
    options
        .validate_input()
        .map_err(|error| Failure::remote(error, "setup", None))?;
    let config = options
        .embedding_config
        .as_deref()
        .map(|path| Context::new("setup", "--embedding-config").read_embedding(path))
        .transpose()?;
    let path = options.embedding_config.clone();
    Prepared::from_config(options, config)
        .map_err(|error| Failure::remote(error, "setup", None).path(path.as_deref()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error as _;

    #[test]
    fn selected_configuration_causes_stay_private_and_available() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("selected");
        for malformed in [false, true] {
            if malformed {
                std::fs::write(&path, b"PRIVATE: [").unwrap();
            }
            let error = prepare(Options {
                embedding_config: Some(path.clone()),
                ..Options::default()
            })
            .err()
            .unwrap();
            let cause = error.cause().unwrap();
            if malformed {
                assert_eq!(cause.code, comb::ErrorCode::InvalidEmbeddingConfig);
                assert!(
                    cause
                        .source()
                        .unwrap()
                        .downcast_ref::<serde_yaml_ng::Error>()
                        .is_some()
                );
            } else {
                assert_eq!(
                    cause
                        .source()
                        .unwrap()
                        .downcast_ref::<std::io::Error>()
                        .unwrap()
                        .kind(),
                    std::io::ErrorKind::NotFound
                );
            }
            let output = error.render();
            assert_eq!(output.exit_code, 2);
            let json: serde_json::Value = serde_json::from_str(&output.stdout).unwrap();
            assert_eq!(json["error"]["field"], "--embedding-config");
            assert!(json["error"].get("file_committed").is_none());
            assert!(!output.stdout.contains("PRIVATE"));
            assert!(!output.stderr.contains("PRIVATE"));
        }
    }
}
