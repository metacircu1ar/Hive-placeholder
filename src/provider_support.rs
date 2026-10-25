//! Provider release admission is separate from native feature evidence.
mod codex;
mod probe;
use crate::{
    config::Provider,
    error::{Error, Result},
};
pub(crate) use probe::{Evidence, observe};
use serde::Serialize;

#[derive(Clone, Copy, Debug, Serialize)]
pub(crate) struct Registry {
    pub provider: Provider,
    pub minimum: &'static str,
    pub last_tested: &'static str,
    pub corpus_versions: &'static [&'static str],
}
pub(crate) const REGISTRIES: &[Registry] = &[
    Registry {
        provider: Provider::Codex,
        minimum: "0.154.0",
        last_tested: "0.161.0",
        corpus_versions: &["0.154.0", "0.161.0"],
    },
    Registry {
        provider: Provider::Claude,
        minimum: "2.1.285",
        last_tested: "2.1.285",
        corpus_versions: &["2.1.285"],
    },
];
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Support {
    Tested,
    Untested,
    Unsupported,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct Observation {
    pub version: String,
    pub support: Support,
    pub registry: Registry,
}
pub(crate) const fn registry(provider: Provider) -> Registry {
    match provider {
        Provider::Codex => REGISTRIES[0],
        Provider::Claude => REGISTRIES[1],
    }
}
pub(crate) fn classify(provider: Provider, version: &str) -> Result<Observation> {
    let registry = registry(provider);
    let observed = semver::Version::parse(version).map_err(|cause| {
        Error::version_unrecognized("Provider version must be a semantic version.")
            .with_source(cause)
    })?;
    let minimum = semver::Version::parse(registry.minimum).map_err(|cause| {
        Error::internal_error("Invalid provider compatibility registry.").with_source(cause)
    })?;
    let support = if observed < minimum {
        Support::Unsupported
    } else if registry.corpus_versions.contains(&version) {
        Support::Tested
    } else {
        Support::Untested
    };
    Ok(Observation {
        version: version.into(),
        support,
        registry,
    })
}
impl Observation {
    pub(crate) fn admit(&self) -> Result<()> {
        if self.support == Support::Unsupported {
            return Err(Error::UnsupportedProvider {
                message: format!(
                    "The selected provider is older than the supported minimum {}. Update it \
                    and restart the daemon with the same executable.",
                    self.registry.minimum
                )
                .into(),
                source: None,
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn versions_are_ordered_and_untested_is_not_a_capability() {
        for (provider, version, expected) in [
            (Provider::Codex, "0.153.99", Support::Unsupported),
            (Provider::Codex, "0.154.0-alpha.1", Support::Unsupported),
            (Provider::Codex, "0.154.0", Support::Tested),
            (Provider::Codex, "0.160.0", Support::Untested),
            (Provider::Codex, "0.161.0", Support::Tested),
            (Provider::Codex, "0.162.1", Support::Untested),
            (Provider::Claude, "2.1.99", Support::Unsupported),
            (Provider::Claude, "2.1.285", Support::Tested),
            (Provider::Claude, "2.1.286", Support::Untested),
            (Provider::Claude, "3.0.0", Support::Untested),
        ] {
            let observation = classify(provider, version).unwrap();
            assert_eq!(observation.support, expected);
            assert_eq!(
                observation.admit().is_err(),
                expected == Support::Unsupported
            );
        }
        for invalid in ["", "2", "2.1", "02.1.285", "2.1.285\n"] {
            assert!(classify(Provider::Claude, invalid).is_err());
        }
    }
}
