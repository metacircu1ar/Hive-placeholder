//! Resolve environment/file presence off the writer, after matching daemon config.
use super::{Caller, Error, Hub, Result};
#[derive(serde::Serialize)]
struct Reply {
    #[serde(flatten)]
    report: crate::auth::Report,
    availability: Vec<crate::runtime::auth_fallback::Availability>,
}
use crate::runtime::api::{self, KeySourceArgs};

impl Hub {
    pub(super) async fn key_source(
        &self,
        caller: Caller,
        args: KeySourceArgs,
    ) -> Result<serde_json::Value> {
        let name = args.auth.clone();
        let register = args.register;
        let input = self
            .with(move |core| {
                caller.check(core)?;
                if !caller.is_human() {
                    return Err(Error::unsupported_operation(
                        "API key source status is for humans only.",
                    ));
                }
                let profile = core.config().profile(&args.auth)?;
                if *profile != args.profile || profile.method != crate::config::Method::ApiKey {
                    return Err(Error::binding_conflict(
                        "Daemon API key profile differs from the selected configuration.",
                    ));
                }
                Ok(crate::auth::source::Input {
                    name: args.auth,
                    profile: profile.clone(),
                    state: core.state().clone(),
                    binary: core.binaries().resolve(profile.provider),
                    register,
                })
            })
            .await?;
        let mut report = self.preflight.key_source(input).await?;
        if register && report.state == crate::auth::Status::PresentUnverified {
            let restored = self
                .prepared::<super::auth::AuthPreparation>(
                    Caller::Human,
                    uuid::Uuid::new_v4().to_string(),
                    api::RequestEnvelope::new(api::value(api::Request::LoginRestored(
                        api::AuthArgs {
                            auth: name.clone().into(),
                        },
                    ))),
                )
                .await;
            if let Err(error) = restored {
                report.warnings.push(error);
            }
        }
        let availability = self
            .with(move |core| core.store().auth_status(&name))
            .await?
            .availability;
        Ok(api::value(Reply {
            report,
            availability,
        }))
    }
}
