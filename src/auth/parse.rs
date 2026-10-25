//! Bounded native authentication metadata decoding.
use super::{
    Deserialize, Error, ObservedMethod, Operation, Provider, Report, Result, Status, process,
};
use crate::runtime::domain::wire_enum;
use serde_json::Value;
use std::str;

pub(super) fn verify_pending(operation: &Operation, report: &Report) -> Result<()> {
    match report.state {
        Status::Authenticated => operation.confirm_account(report.account_identity.clone()),
        Status::Missing | Status::ReauthenticationRequired => operation.confirm_missing(),
        Status::MethodConflict => Err(Error::policy_conflict(concat!(
            "The selected authentication method conflicts with native credentials. ",
            "Correct provider configuration or authenticate an isolated replacement.",
        ))),
        _ => Err(report.diagnostic.clone().unwrap_or_else(|| {
            Error::recovery_required(concat!(
                "Login verification is incomplete. Retry auth recover or auth login after ",
                "the provider becomes available; the original credential namespace and ",
                "evidence were retained.",
            ))
        })),
    }
}

// Maximum provider version text accepted for an authentication diagnostic.
const VERSION_BYTES: usize = 40;

pub(crate) fn parse_version(provider: Provider, output: &[u8]) -> Result<String> {
    let text = str::from_utf8(output)
        .map_err(|cause| process::malformed().with_source(cause))?
        .trim();
    let version = match provider {
        Provider::Codex => text.strip_prefix("codex-cli "),
        Provider::Claude => text.strip_suffix(" (Claude Code)"),
    }
    .ok_or_else(process::malformed)?;
    if version.is_empty()
        || version.len() > VERSION_BYTES
        || !version.bytes().next().is_some_and(|c| c.is_ascii_digit())
        || !version
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '+' | '-'))
    {
        return Err(process::malformed());
    }
    Ok(version.into())
}

pub(super) fn parse_codex(value: &Value, report: &mut Report) -> Result<()> {
    let result = value.get("result").ok_or(Error::provider_protocol_error(
        "Account probe failed; provider details were withheld.",
    ))?;
    let requires_auth = result
        .get("requiresOpenaiAuth")
        .and_then(Value::as_bool)
        .ok_or_else(process::malformed)?;
    if !requires_auth {
        report.state = Status::MethodConflict;
        report.diagnostic = Some(Error::policy_conflict(
            "Provider configuration does not select the required OpenAI authentication.",
        ));
        return Ok(());
    }
    match result.get("account") {
        Some(Value::Null) => {
            report.state = Status::Missing;
            report.credential_present = Some(false);
        }
        Some(account) => {
            report.credential_present = Some(true);
            match account
                .get("type")
                .and_then(|v| v.as_str())
                .map(AuthAccountKind::parse)
            {
                Some(AuthAccountKind::Chatgpt) => {
                    report.state = Status::Authenticated;
                    report.observed_method = Some(ObservedMethod::Subscription);
                    report.account_identity = account
                        .get("email")
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                }
                Some(AuthAccountKind::ApiKey | AuthAccountKind::AmazonBedrock) => {
                    report.state = Status::MethodConflict;
                    report.observed_method = Some(ObservedMethod::Other);
                }
                _ => return Err(process::malformed()),
            }
        }
        _ => return Err(process::malformed()),
    }
    Ok(())
}

wire_enum!(AuthAccountKind {
    Chatgpt => "chatgpt",
    ApiKey => "apiKey",
    AmazonBedrock => "amazonBedrock",
});
wire_enum!(ClaudeAuthMethod {
    ClaudeAi => "claude.ai",
    None => "none",
    ApiKeySnake => "api_key",
    ApiKey => "apiKey",
    OauthToken => "oauth_token",
    ApiKeyHelper => "apiKeyHelper",
    Bedrock => "bedrock",
    Vertex => "vertex",
});
wire_enum!(ClaudeApiProvider {
    FirstParty => "firstParty",
    Bedrock => "bedrock",
    Vertex => "vertex",
    Foundry => "foundry",
});

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClaudeStatus {
    logged_in: bool,
    auth_method: ClaudeAuthMethod,
    api_provider: ClaudeApiProvider,
    email: Option<String>,
}
pub(super) fn parse_claude(exit: Option<i32>, bytes: &[u8], report: &mut Report) -> Result<()> {
    let status: ClaudeStatus =
        serde_json::from_slice(bytes).map_err(|cause| process::malformed().with_source(cause))?;
    if !matches!(exit, Some(libc::EXIT_SUCCESS | libc::EXIT_FAILURE)) {
        return Err(Error::provider_failed("Provider status command failed."));
    }
    report.credential_present = Some(status.logged_in);
    match (status.logged_in, status.auth_method, status.api_provider) {
        (true, ClaudeAuthMethod::ClaudeAi, ClaudeApiProvider::FirstParty)
            if exit == Some(libc::EXIT_SUCCESS) =>
        {
            report.state = Status::Authenticated;
            report.observed_method = Some(ObservedMethod::Subscription);
            report.account_identity = status.email;
        }
        (false, ClaudeAuthMethod::None, ClaudeApiProvider::FirstParty) => {
            report.state = Status::Missing;
        }
        (false, ClaudeAuthMethod::ClaudeAi, ClaudeApiProvider::FirstParty) => {
            report.state = Status::ReauthenticationRequired;
        }
        (
            _,
            ClaudeAuthMethod::ApiKeySnake
            | ClaudeAuthMethod::ApiKey
            | ClaudeAuthMethod::OauthToken
            | ClaudeAuthMethod::ApiKeyHelper
            | ClaudeAuthMethod::Bedrock
            | ClaudeAuthMethod::Vertex,
            _,
        )
        | (
            _,
            _,
            ClaudeApiProvider::Bedrock | ClaudeApiProvider::Vertex | ClaudeApiProvider::Foundry,
        ) => {
            report.state = Status::MethodConflict;
            report.observed_method = Some(ObservedMethod::Other);
        }
        _ => return Err(process::malformed()),
    }
    Ok(())
}
