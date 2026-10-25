//! Only known operation pointers become CLI fields; arbitrary daemon text is refused.
use super::{Request, api};

pub(super) fn command(request: &Request) -> Option<&'static str> {
    Some(match request {
        #[cfg(feature = "comb")]
        Request::Evidence(_) => "evidence",
        #[cfg(feature = "comb")]
        Request::CollectorAdmin(args) => return crate::cli::collectors::admin_command(args),
        Request::Batch(_) => "batch",
        Request::Approvals(_) => "approvals",
        Request::Approval(_) => "approvals show",
        Request::Respond(_) => "respond",
        Request::Agents(_) => "agents",
        Request::Read(_) => "read",
        Request::Messages(_) => "messages",
        Request::Output(_) => "output",
        Request::Status(_) => "status",
        Request::Spawn(_) => "spawn",
        Request::Send(_) => "send",
        Request::Complete(_) => "complete",
        Request::Terminate(_) => "terminate",
        Request::Stop(_) => "stop",
        Request::Resume(_) => "resume",
        Request::Interrupt(_) => "interrupt",
        Request::Purge(_) => "purge",
        Request::SetOptions(_) => "set-options",
        Request::SetAuth(_) => "set-auth",
        Request::SetName(_) => "set-name",
        _ => return None,
    })
}

pub(super) fn field(request: &Request, error: &hive_input::InputError) -> Option<&'static str> {
    if error.field() == "/id" && error.code() == hive_input::Code::InvalidRequestId {
        // This is the control envelope identity, never an output artifact ID.
        return request_id_field(request);
    }
    Some(match (request, error.field()) {
        (Request::Batch(_), pointer) => return crate::cli::batch::field(pointer),
        (Request::Approvals(_), pointer) => return crate::cli::approvals::list_field(pointer),
        #[cfg(feature = "comb")]
        (Request::Evidence(_), pointer) => return crate::cli::collectors::field(pointer),
        #[cfg(feature = "comb")]
        (Request::CollectorAdmin(args), pointer) => {
            return crate::cli::collectors::admin_field(args, pointer);
        }
        (request, "/target") => return target_field(request),
        (Request::Output(_), "/id") => "ID",
        (
            Request::Agents(_) | Request::Read(_) | Request::Messages(_) | Request::Output(_),
            "/limit",
        ) => "--limit",
        (Request::Agents(_) | Request::Read(_), "/after") => "--after",
        (Request::Messages(_), "/before") => "--before",
        (Request::Output(_), "/offset") => "--offset",
        (Request::Agents(_), "/parent") => "--parent",
        (Request::Agents(_), "/stalled_seconds") => "--stalled",
        (
            Request::Agents(_),
            "/stalled_after" | "/stalled_after/root_id" | "/stalled_after/idle_since",
        ) => "--stalled-after",
        (Request::Agents(_) | Request::Spawn(_), "/auth") => "--auth",
        (Request::Status(_), "/instructions") => "--instructions",
        (Request::Spawn(_), "/role") => "ROLE",
        (Request::Spawn(_), "/preset") => "--preset",
        (Request::Spawn(_), "/from_agent") => "--from",
        (Request::Spawn(_), "/cwd") => "--cwd",
        (Request::Spawn(_), "/provider") => "--provider",
        (Request::Spawn(_), "/sandbox") => "--sandbox",
        (Request::Spawn(_), "/lifetime") => "--lifetime",
        (Request::Spawn(_), "/api_fallback") => "--no-api-fallback",
        (Request::Spawn(_), "/instructions") => "--instructions-file",
        (Request::Spawn(_), "/model") => "--model",
        (Request::SetOptions(_) | Request::Spawn(_), field)
            if field == "/options" || field.starts_with("/options/") =>
        {
            "--option"
        }
        (Request::SetOptions(_) | Request::Spawn(_), "/config") => "--native-config",
        (Request::Spawn(_), "/effort") => "--effort",
        (Request::SetAuth(_), "/auth") => "AUTH",
        (Request::SetName(_), "/name") => "NAME",
        (Request::SetName(_), "/clear") => "--clear",
        (Request::Spawn(_), "/name") => "--name",
        (Request::Spawn(_) | Request::Send(_) | Request::Complete(_), "/body") => "--body",
        (Request::Spawn(_) | Request::Send(_), "/attachments") => "--attach",
        (Request::Send(_), "/mode") => "--mode",
        (Request::Purge(_), "/recursive") => "--recursive",
        (Request::Purge(_), "/dry_run") => "--dry-run",
        (Request::Approval(_) | Request::Respond(_), "/approval_id") => "APPROVAL_ID",
        (Request::Respond(_), "/content") => "--data",
        (Request::Respond(_), "/reason") => "--reason",
        (Request::Respond(args), "/action") => match args.action.copied()? {
            api::ApprovalAction::Accept => "--accept",
            api::ApprovalAction::Decline => "--decline",
            api::ApprovalAction::Cancel => "--cancel",
        },
        _ => return None,
    })
}

fn request_id_field(request: &Request) -> Option<&'static str> {
    match request {
        #[cfg(feature = "comb")]
        Request::Evidence(_) | Request::CollectorAdmin(_) => Some("--request-id"),
        Request::Spawn(_)
        | Request::Send(_)
        | Request::Complete(_)
        | Request::Terminate(_)
        | Request::Stop(_)
        | Request::Resume(_)
        | Request::Interrupt(_)
        | Request::Purge(_)
        | Request::Batch(_)
        | Request::SetOptions(_)
        | Request::SetAuth(_)
        | Request::SetName(_)
        | Request::Respond(_) => Some("--request-id"),
        _ => None,
    }
}

fn target_field(request: &Request) -> Option<&'static str> {
    match request {
        Request::Read(_)
        | Request::Messages(_)
        | Request::Status(_)
        | Request::Send(_)
        | Request::SetOptions(_)
        | Request::SetAuth(_)
        | Request::SetName(_)
        | Request::Complete(_)
        | Request::Terminate(_)
        | Request::Stop(_)
        | Request::Resume(_)
        | Request::Interrupt(_)
        | Request::Purge(_) => Some("TARGET"),
        _ => None,
    }
}
