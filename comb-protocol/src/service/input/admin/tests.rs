use super::*;
use crate::service::{Operation, input};
use serde_json::{Value, json};
const ADMIN: [Admin; 11] = [
    Admin::Capabilities,
    Admin::Health,
    Admin::Stats,
    Admin::Doctor,
    Admin::Sync,
    Admin::Import,
    Admin::ImportStatus,
    Admin::ImportAbandon,
    Admin::CredentialCreate,
    Admin::CredentialList,
    Admin::CredentialRevoke,
];
fn raw(operation: &str) -> Vec<u8> {
    format!(r#"{{"protocol":1,"credential":{{"comb_id":"12345678-1234-4234-8234-123456789abc","id":"human","secret":"private-secret"}},"operation":{operation}}}"#).into_bytes()
}
fn socket(operation: Value) -> Result<Operation> {
    input::request(&raw(&operation.to_string())).map(|r| r.operation)
}
fn check(error: Error, code: Code, field: &str) {
    let input = error.input().unwrap();
    assert_eq!(input.code(), code);
    assert_eq!(input.field(), format!("/operation{field}"));
    assert!(!serde_json::to_string(&error).unwrap().contains("private"));
    assert!(!error.file_committed);
    assert!(error.request_id.is_none());
}
#[test]
fn admin_examples_unknown_fields_and_original_byte_duplicates() {
    for admin in ADMIN {
        socket(admin.sample()).unwrap();
        let mut unknown = admin.sample();
        unknown["private/key"] = json!("private-value");
        check(socket(unknown).err().unwrap(), Code::UnknownArgument, "");
        let duplicate =
            admin
                .sample()
                .to_string()
                .replacen('{', &format!(r#"{{"op":"{}","#, admin.name()), 1);
        let error = input::request(&raw(&duplicate)).err().unwrap();
        check(error, Code::InvalidValue, "");
    }
}
#[test]
fn admin_top_level_fields_report_types_missing_and_nullable_prefix() {
    for admin in [
        Admin::Import,
        Admin::ImportAbandon,
        Admin::CredentialCreate,
        Admin::CredentialRevoke,
    ] {
        for field in admin.fields() {
            for bad in [json!(false), json!(["private"]), Value::Null] {
                let mut operation = admin.sample();
                operation[field] = bad.clone();
                if *field == "prefix" && bad.is_null() {
                    socket(operation).unwrap();
                    continue;
                }
                check(
                    socket(operation).err().unwrap(),
                    Code::InvalidValue,
                    &format!("/{field}"),
                );
            }
            let mut missing = admin.sample();
            missing.as_object_mut().unwrap().remove(*field);
            if *field == "prefix" {
                socket(missing).unwrap();
            } else {
                check(
                    socket(missing).err().unwrap(),
                    Code::MissingArgument,
                    &format!("/{field}"),
                );
            }
        }
    }
}
#[test]
fn grant_structure_errors_are_precise_and_do_not_echo_unknown_fields() {
    for (grant, code, field) in [
        (json!({}), Code::MissingArgument, "/grant/kind"),
        (json!({"kind":"private"}), Code::InvalidValue, "/grant/kind"),
        (json!({"kind":false}), Code::InvalidValue, "/grant/kind"),
        (
            json!({"kind":"client"}),
            Code::MissingArgument,
            "/grant/name",
        ),
        (
            json!({"kind":"client","name":null}),
            Code::InvalidValue,
            "/grant/name",
        ),
        (
            json!({"kind":"client","name":"name","reviewer":null}),
            Code::InvalidValue,
            "/grant/reviewer",
        ),
        (
            json!({"kind":"client","name":"name","private":true}),
            Code::UnknownArgument,
            "/grant",
        ),
        (
            json!({"kind":"bridge","name":"name","reviewer_roles":null}),
            Code::InvalidValue,
            "/grant/reviewer_roles",
        ),
        (
            json!({"kind":"bridge","name":"name","reviewer_roles":["valid",null]}),
            Code::InvalidValue,
            "/grant/reviewer_roles/1",
        ),
        (
            json!({"kind":"bridge","name":"name","reviewer":true}),
            Code::UnknownArgument,
            "/grant",
        ),
    ] {
        check(
            socket(json!({"op":"credential_create","grant":grant}))
                .err()
                .unwrap(),
            code,
            field,
        );
    }
    for grant in [
        json!({"kind":"client","name":"name"}),
        json!({"kind":"bridge","name":"name"}),
    ] {
        socket(json!({"op":"credential_create","grant":grant})).unwrap();
    }
    let bytes = raw(
        r#"{"op":"credential_create","grant":{"kind":"client","name":"private","name":"name"}}"#,
    );
    check(
        input::request(&bytes).err().unwrap(),
        Code::InvalidValue,
        "",
    );
}
#[test]
fn admin_primitive_errors_retain_native_causes_and_accept_native_forms() {
    for (operation, field) in [
        (
            json!({"op":"import","directory":".","prefix":"../private"}),
            "/prefix",
        ),
        (json!({"op":"credential_revoke","id":"private"}), "/id"),
    ] {
        let error = socket(operation).err().unwrap();
        assert!(std::error::Error::source(&error).is_some());
        check(error, Code::InvalidValue, field);
    }
    for id in ["human", "ABCDEFAB-CDEF-4ABC-8ABC-ABCDEFABCDEF"] {
        socket(json!({"op":"credential_revoke","id":id})).unwrap();
    }
    // Authority envelope errors never acquire an operation pointer.
    let bytes = String::from_utf8(raw(r#"{"op":"health"}"#))
        .unwrap()
        .replace("human", "not-an-id");
    assert!(
        input::request(bytes.as_bytes())
            .err()
            .unwrap()
            .input()
            .is_none()
    );
}
#[test]
fn grant_value_checks_remain_deferred_and_keep_native_boundaries() {
    for (grant, code, field) in [
        (
            json!({"kind":"client","name":""}),
            Code::InvalidValue,
            "/grant/name",
        ),
        (
            json!({"kind":"client","name":"é".repeat(257)}),
            Code::InvalidValue,
            "/grant/name",
        ),
        (
            json!({"kind":"bridge","name":"n","reviewer_roles":vec!["r";65]}),
            Code::OutOfRange,
            "/grant/reviewer_roles",
        ),
        (
            json!({"kind":"bridge","name":"n","reviewer_roles":["r","r"]}),
            Code::InvalidValue,
            "/grant/reviewer_roles/1",
        ),
        (
            json!({"kind":"bridge","name":"n","reviewer_roles":["private\n"]}),
            Code::InvalidValue,
            "/grant/reviewer_roles/0",
        ),
    ] {
        let Operation::CredentialCreate { grant } =
            socket(json!({"op":"credential_create","grant":grant})).unwrap()
        else {
            panic!()
        };
        let error = validate_grant(&grant).unwrap_err();
        assert!(std::error::Error::source(&error).is_some());
        check(error, code, field);
        assert!(grant.validate().unwrap_err().input().is_none());
    }
    for name in [" ".into(), "é".repeat(256)] {
        validate_grant(&Grant::Bridge {
            name,
            reviewer_roles: (0..64).map(|i| format!("r{i}")).collect(),
        })
        .unwrap();
    }
}
