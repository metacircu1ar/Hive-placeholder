//! Only pointers belonging to the submitted command become CLI fields.
use comb_protocol::service::input::retrieval::Read;

pub(super) fn field(
    command: &'static str,
    pointer: &str,
    source: Option<&'static str>,
) -> Option<&'static str> {
    if command == "embed" {
        return (pointer == "/options/replace").then_some("--replace");
    }
    if command == "setup" {
        return comb::setup::input::field(pointer);
    }
    if command == "serve" {
        return match pointer {
            "/options/replace_embeddings" => Some("--replace-embeddings"),
            "/options/retry_failed" => Some("--retry-failed"),
            "/options/vector_cache_mib" => Some("--vector-cache-mib"),
            _ => None,
        };
    }
    if let Some(mutation) =
        comb_protocol::service::input::mutation::Write::named(if command == "rm" {
            "remove"
        } else {
            command
        })
    {
        return write_field(mutation, command, pointer, source);
    }
    if let Some(admin) = admin(command) {
        return admin_field(admin, command, pointer);
    }
    let read = Read::named(command)?;
    if pointer == "/operation" {
        return Some(command);
    }
    if matches!(read, Read::Search | Read::List) && pointer == "/operation/request" {
        return Some(command);
    }
    let name = if read == Read::Search && pointer == "/operation/mode" {
        "mode"
    } else if matches!(read, Read::Search | Read::List) {
        pointer.strip_prefix("/operation/request/")?
    } else {
        pointer.strip_prefix("/operation/")?
    };
    if !read.fields().contains(&name) {
        return None;
    }
    match name {
        "slug" => Some("SLUG"),
        "query" => Some("QUERY"),
        "limit" => Some("--limit"),
        "depth" => Some("--depth"),
        "mode" if pointer == "/operation/mode" => Some("--mode"),
        "kind" => Some("--type"),
        "tag" => Some("--tag"),
        "prefix" => Some("--prefix"),
        "after" => Some("--after"),
        "before" => Some("--before"),
        "unverified" => Some("--unverified"),
        _ => None,
    }
}

fn write_field(
    mutation: comb_protocol::service::input::mutation::Write,
    command: &'static str,
    pointer: &str,
    source: Option<&'static str>,
) -> Option<&'static str> {
    use comb_protocol::service::input::mutation::Write;
    if pointer == "/operation"
        || (mutation == Write::Consolidate && pointer == "/operation/request")
    {
        return Some(command);
    }
    let field = mutation
        .fields()
        .iter()
        .find(|field| mutation.pointer(field, false) == pointer)?;
    match *field {
        "slug" => Some("SLUG"),
        "note" => Some("NOTE"),
        "target" => Some("TARGET"),
        "markdown" => source,
        "text" => Some("TEXT"),
        "source" => Some("--source"),
        "summary" => Some("--summary-file"),
        "expected_hash" => Some("--expected-hash"),
        "note_hash" => Some("--note-hash"),
        "target_hash" => Some("--target-hash"),
        "request_id" => Some("--request-id"),
        "run" => Some("--run"),
        "decision" => Some("DECISION"),
        "reason" => Some("--reason"),
        "recopy" => Some("--recopy"),
        _ => None,
    }
}

fn admin(command: &str) -> Option<comb_protocol::service::input::admin::Admin> {
    comb_protocol::service::input::admin::Admin::named(match command {
        "credential create" => "credential_create",
        "credential list" => "credential_list",
        "credential revoke" => "credential_revoke",
        command => command,
    })
}
fn admin_field(
    admin: comb_protocol::service::input::admin::Admin,
    command: &'static str,
    pointer: &str,
) -> Option<&'static str> {
    use comb_protocol::service::input::admin::Admin;
    if pointer == "/operation" {
        return Some(command);
    }
    let field = pointer.strip_prefix("/operation/")?;
    match (admin, field) {
        (Admin::Import, "directory") => Some("DIR"),
        (Admin::Import, "prefix") => Some("--prefix"),
        (Admin::CredentialRevoke, "id") => Some("ID"),
        (Admin::CredentialCreate, "grant") => Some(command),
        (Admin::CredentialCreate, "grant/name") => Some("NAME"),
        (Admin::CredentialCreate, "grant/kind") => Some("--bridge"),
        (Admin::CredentialCreate, "grant/reviewer") => Some("--reviewer"),
        (Admin::CredentialCreate, "grant/reviewer_roles") => Some("--reviewer-role"),
        (Admin::CredentialCreate, field) => {
            let index = field.strip_prefix("grant/reviewer_roles/")?;
            let number = index.parse::<usize>().ok()?;
            (number < comb_protocol::service::limits::REVIEWER_ROLES && number.to_string() == index)
                .then_some("--reviewer-role")
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::field;
    #[test]
    fn administrative_fields_are_command_scoped_and_role_indices_bounded() {
        for (pointer, expected) in [
            ("/operation/grant/name", Some("NAME")),
            ("/operation/grant/kind", Some("--bridge")),
            ("/operation/grant/reviewer", Some("--reviewer")),
            ("/operation/grant/reviewer_roles/0", Some("--reviewer-role")),
            (
                "/operation/grant/reviewer_roles/63",
                Some("--reviewer-role"),
            ),
            ("/operation/grant/reviewer_roles/64", None),
            ("/operation/grant/reviewer_roles/00", None),
            ("/operation/grant/reviewer_roles/+1", None),
            ("/operation/grant/reviewer_roles/1/private", None),
            ("/operation/id", None),
        ] {
            assert_eq!(field("credential create", pointer, None), expected);
        }
        assert_eq!(
            field("credential revoke", "/operation/id", None),
            Some("ID")
        );
        assert_eq!(field("credential list", "/operation/id", None), None);
        assert_eq!(field("import", "/operation/directory", None), Some("DIR"));
        assert_eq!(field("import", "/operation/grant/name", None), None);
        assert_eq!(field("health", "/operation", None), Some("health"));
        assert_eq!(field("health", "/operation/grant", None), None);
    }
    #[test]
    fn consolidation_nested_fields_and_remove_use_only_their_own_coordinates() {
        for (pointer, expected) in [
            ("/operation/request", Some("consolidate")),
            ("/operation/request/note", Some("NOTE")),
            ("/operation/request/target", Some("TARGET")),
            ("/operation/request/note_hash", Some("--note-hash")),
            ("/operation/request/target_hash", Some("--target-hash")),
            ("/operation/request/summary", Some("--summary-file")),
            ("/operation/request/recopy", Some("--recopy")),
            ("/operation/request_id", Some("--request-id")),
            ("/operation/run", Some("--run")),
            ("/operation/note", None),
            ("/operation/request/request_id", None),
            ("/operation/slug", None),
        ] {
            assert_eq!(field("consolidate", pointer, None), expected);
        }
        assert_eq!(
            field("rm", "/operation/expected_hash", None),
            Some("--expected-hash")
        );
        assert_eq!(field("rm", "/operation/run", None), None);
        assert_eq!(field("capture", "/operation/slug", None), None);
    }
}
