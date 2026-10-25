//! Preflight and confirmation of restored subscription profiles.
use super::{AuthReason, prepare};
use crate::{
    error::{Error, Result},
    runtime::Shared,
};

pub(super) async fn restore_profile(shared: &Shared, auth: String) -> Result<()> {
    let name = auth.clone();
    let input = shared
        .with(move |core| {
            if core.config().profile(&name)?.method == crate::config::Method::ApiKey {
                return Ok(None);
            }
            Ok(Some(prepare::Input::capture(core, name)))
        })
        .await?;
    let Some(input) = input else {
        return Ok(());
    };
    // Detect offline credential repair outside the writer even for diagnostic/spend
    // holds, whose same-generation identity verification cannot clear the hold.
    let prepared = shared.preflight.auth(input).await?;
    let binding = prepared
        .members
        .first()
        .ok_or_else(|| Error::unknown_profile("Missing restoration profile."))?
        .binding
        .as_ref()
        .map_err(Clone::clone)?;
    let (name, generation) = (auth.clone(), binding.id);
    let unchanged = shared
        .with(move |core| {
            let value = core.store().availability(&name, generation)?;
            Ok(value.revision != 0
                && (value.probe_error().is_some() || value.reason == Some(AuthReason::SpendCap)))
        })
        .await?;
    if unchanged {
        return Ok(());
    }
    shared
        .dispatch(
            crate::runtime::hub::Caller::System,
            uuid::Uuid::new_v4().to_string(),
            crate::runtime::api::RequestEnvelope::new(crate::runtime::api::value(
                crate::runtime::api::Request::LoginRestored(crate::runtime::api::AuthArgs {
                    auth: auth.into(),
                }),
            )),
        )
        .await
        .map(|_| ())
}
