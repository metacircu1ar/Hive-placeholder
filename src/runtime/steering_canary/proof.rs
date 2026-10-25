//! One current native capability proof. No migrations or inferred enablement.
use crate::runtime::domain::decode::{object, optional_object, string_enum};
use crate::{
    error::{Error, Result},
    runtime::{
        locks::MutexExt,
        provider::{catalogs::BinaryIdentity, claude::ReceiptMode},
    },
    state,
};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
};

pub(super) const ADAPTER: u32 = 2;
// A single proof record is bounded independently of user-controlled message text.
const RECORD_BYTES: u64 = 64 * 1024;
const RECORD_READ_BYTES: u64 = RECORD_BYTES + 1;
// SHA-256 encodes 32 digest bytes as two hexadecimal characters per byte.
const DIGEST_HEX_BYTES: usize = 32 * 2;
// ELF and Mach-O executable identification consumes the same four-byte prefix.
const EXECUTABLE_MAGIC_BYTES: usize = 4;
// Hash executable files in 16 KiB chunks and refuse binaries larger than 512 MiB.
const DIGEST_BUFFER_BYTES: usize = 16 * 1024;
const EXECUTABLE_BYTES: usize = 512 * 1024 * 1024;
const _: () = assert!(EXECUTABLE_MAGIC_BYTES <= DIGEST_BUFFER_BYTES);
const _: () = assert!(DIGEST_BUFFER_BYTES <= EXECUTABLE_BYTES);
const GUIDANCE_PREFIX: &str = "Include this exact marker in your final answer: ";

pub(super) fn guidance(nonce: &str) -> String {
    format!("{GUIDANCE_PREFIX}{nonce}")
}

crate::runtime::domain::domain_enum!(DeliveryObservation {
    NativeEcho => "native_echo", VerifiedAssistantContent => "verified_assistant_content",
});

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(in crate::runtime) struct Receipt {
    turn_id: String,
    #[serde(deserialize_with = "object")]
    delivery_evidence: Delivery,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Delivery {
    #[serde(deserialize_with = "string_enum")]
    kind: ReceiptMode,
    observation: DeliveryObservation,
    #[serde(deserialize_with = "object")]
    hook_reply: HookReply,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct HookReply {
    #[serde(rename = "type")]
    kind: String,
    #[serde(deserialize_with = "object")]
    response: HookResponse,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct HookResponse {
    subtype: String,
    request_id: String,
    #[serde(deserialize_with = "object")]
    response: HookOutput,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct HookOutput {
    #[serde(deserialize_with = "object")]
    hook_specific_output: HookContext,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct HookContext {
    hook_event_name: String,
    additional_context: String,
}

impl Receipt {
    pub(super) fn turn(&self) -> &str {
        &self.turn_id
    }
    pub(super) fn mode(&self) -> ReceiptMode {
        self.delivery_evidence.kind
    }
}

// Keep malformed receipt content in the incomplete-proof error category. The field
// is still required, and read() rejects None before it can authorize a capability.
fn receipt<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Option<Receipt>, D::Error> {
    // Consume the complete value before validating its shape, so a rejected field
    // cannot leave the surrounding JSON parser partway through the receipt.
    let value = Value::deserialize(deserializer)?;
    Ok(object(&value).ok())
}

fn valid_receipt(proof: &Proof) -> bool {
    let Some(receipt) = &proof.receipt else {
        return false;
    };
    let delivery = &receipt.delivery_evidence;
    let response = &delivery.hook_reply.response;
    let context = &response.response.hook_specific_output;
    receipt.turn_id == proof.turn
        && delivery.kind == proof.receipt_mode
        && delivery.observation
            == match delivery.kind {
                ReceiptMode::NativeEcho => DeliveryObservation::NativeEcho,
                ReceiptMode::WrittenContinuation => DeliveryObservation::VerifiedAssistantContent,
            }
        && delivery.hook_reply.kind == "control_response"
        && response.subtype == "success"
        && !response.request_id.is_empty()
        && context.hook_event_name == "PostToolUse"
        && context
            .additional_context
            .strip_prefix(GUIDANCE_PREFIX)
            .is_some_and(|nonce| {
                uuid::Uuid::parse_str(nonce).is_ok() && hash(nonce.as_bytes()) == proof.nonce_sha256
            })
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::runtime) struct Proof {
    #[serde(deserialize_with = "object")]
    pub binary: BinaryIdentity,
    pub digest: String,
    pub version: String,
    pub adapter: u32,
    pub model: String,
    pub effort: String,
    #[serde(deserialize_with = "string_enum")]
    pub receipt_mode: ReceiptMode,
    pub session: String,
    pub turn: String,
    #[serde(deserialize_with = "receipt")]
    pub receipt: Option<Receipt>,
    pub final_output_sha256: String,
    pub nonce_sha256: String,
    pub verified_at: u64,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    #[serde(deserialize_with = "optional_object")]
    proof: Option<Proof>,
}

crate::runtime::domain::domain_enum!(SteeringObservation {
    ProofInvalid => "proof_invalid",
    ProofMissing => "proof_missing",
    ProofMissingOrRevoked => "proof_missing_or_revoked",
    ProofStaleAdapterChanged => "proof_stale_adapter_changed",
    ProofStaleBinaryChanged => "proof_stale_binary_changed",
    ProofNotVerifiedForModelEffort => "proof_not_verified_for_model_effort",
    Verified => "verified",
});

#[derive(Clone)]
pub(in crate::runtime) enum Candidate {
    Invalid(crate::error::ErrorCode),
    Unavailable(SteeringObservation),
    Verified(Arc<Proof>),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::runtime) enum CapabilitySelection {
    Disabled(SteeringObservation),
    Verified(ReceiptMode),
}
impl CapabilitySelection {
    pub fn mode(self) -> Option<ReceiptMode> {
        match self {
            Self::Verified(mode) => Some(mode),
            Self::Disabled(_) => None,
        }
    }
    pub fn reason(self) -> SteeringObservation {
        match self {
            Self::Verified(_) => SteeringObservation::Verified,
            Self::Disabled(reason) => reason,
        }
    }
}
impl Candidate {
    pub fn invalid(error: &Error) -> Self {
        static FAILURES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        if let Some(count) = crate::observability::sample(&FAILURES) {
            tracing::warn!(name: "steering.proof_invalid",
                 code = error.code(),
                 count = count,
                 error = error as &(dyn std::error::Error + 'static),
                 decision = "disable_steering");
        }
        Self::Invalid(error.kind())
    }
    pub fn error_code(&self) -> Option<crate::error::ErrorCode> {
        match self {
            Self::Invalid(code) => Some(*code),
            Self::Unavailable(_) | Self::Verified(_) => None,
        }
    }
    pub fn selection(&self, model: Option<&str>, effort: &str) -> CapabilitySelection {
        let proof = match self {
            Self::Invalid(_) => {
                return CapabilitySelection::Disabled(SteeringObservation::ProofInvalid);
            }
            Self::Unavailable(reason) => return CapabilitySelection::Disabled(*reason),
            Self::Verified(proof) => proof,
        };
        if !proof.binary.unchanged() {
            return CapabilitySelection::Disabled(SteeringObservation::ProofStaleBinaryChanged);
        }
        if model != Some(proof.model.as_str()) || effort != proof.effort {
            return CapabilitySelection::Disabled(
                SteeringObservation::ProofNotVerifiedForModelEffort,
            );
        }
        CapabilitySelection::Verified(proof.receipt_mode)
    }
    pub fn check_launch(&self) -> Result<()> {
        if matches!(self, Self::Verified(proof) if !proof.binary.unchanged()) {
            return Err(Error::provider_unavailable(
                "Claude executable changed during steering-proof validation; retry.",
            ));
        }
        Ok(())
    }
}
struct Cached {
    file: BinaryIdentity,
    binary: BinaryIdentity,
    candidate: Candidate,
}
static CACHE: OnceLock<Mutex<Option<Cached>>> = OnceLock::new();

pub(super) fn path(root: &Path) -> PathBuf {
    root.join("runtime/claude-steering-proof.json")
}
fn exists(path: &Path) -> Result<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => {
            state::check_path(path, false)?;
            Ok(true)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(Error::state_io_from(error)),
    }
}
pub(super) fn read(path: &Path) -> Result<Option<Proof>> {
    if !exists(path)? {
        return Ok(None);
    }
    let data = crate::os::read_capped(state::private_reader(path)?, RECORD_READ_BYTES)
        .map_err(Error::state_io_from)?;
    if data.len() as u64 > RECORD_BYTES {
        return Err(Error::output_limit(
            "Steering proof exceeds its record limit.",
        ));
    }
    let malformed = |cause| {
        Error::state_inconsistent("Malformed steering capability proof.").with_source(cause)
    };
    let mut deserializer = serde_json::Deserializer::from_slice(&data);
    let record: Record = object(&mut deserializer).map_err(malformed)?;
    deserializer.end().map_err(malformed)?;
    if let Some(proof) = &record.proof
        && (!valid_hash(&proof.digest)
            || proof.model.is_empty()
            || proof.effort.is_empty()
            || proof.version.is_empty()
            || proof.session.is_empty()
            || proof.turn.is_empty()
            || !valid_hash(&proof.nonce_sha256)
            || !valid_hash(&proof.final_output_sha256)
            || !valid_receipt(proof))
    {
        return Err(Error::state_inconsistent(
            "Incomplete steering capability proof.",
        ));
    }
    Ok(record.proof)
}
pub(super) fn write(path: &Path, proof: Option<Proof>) -> Result<()> {
    let record = Record { proof };
    let bytes = serde_json::to_vec(&record).map_err(|cause| {
        Error::state_inconsistent("Cannot encode steering proof.").with_source(cause)
    })?;
    if bytes.len() as u64 > RECORD_BYTES {
        return Err(Error::output_limit(
            "Steering proof exceeds its record limit.",
        ));
    }
    state::write_json(path, &record)
}
fn valid_hash(value: &str) -> bool {
    value.len() == DIGEST_HEX_BYTES && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

// Callers pin the actual launch command to this canonical target before loading.
pub(in crate::runtime) fn load(root: &Path, binary: &Path) -> Result<Candidate> {
    let path = path(root);
    if !exists(&path)? {
        return Ok(Candidate::Unavailable(SteeringObservation::ProofMissing));
    }
    let file = BinaryIdentity::read(&path)?;
    let identity = BinaryIdentity::read(binary)?;
    let mut cache = CACHE.get_or_init(|| Mutex::new(None)).lock_or_panic();
    if let Some(cached) = cache
        .as_ref()
        .filter(|entry| entry.file == file && entry.binary == identity)
    {
        return Ok(cached.candidate.clone());
    }
    let proof = read(&path)?;
    let candidate = match proof {
        None => Candidate::Unavailable(SteeringObservation::ProofMissingOrRevoked),
        Some(proof) if proof.adapter != ADAPTER => {
            Candidate::Unavailable(SteeringObservation::ProofStaleAdapterChanged)
        }
        Some(proof) if proof.binary != identity => {
            Candidate::Unavailable(SteeringObservation::ProofStaleBinaryChanged)
        }
        Some(proof) => {
            let digest = native_digest(binary)?;
            if proof.digest != digest || !identity.unchanged() {
                Candidate::Unavailable(SteeringObservation::ProofStaleBinaryChanged)
            } else {
                Candidate::Verified(Arc::new(proof))
            }
        }
    };
    if !file.unchanged() {
        return Err(Error::state_inconsistent(
            "Steering proof changed during validation.",
        ));
    }
    match &candidate {
        Candidate::Verified(_) => {
            tracing::info!(name: "steering.verified", provider = "claude");
        }
        Candidate::Unavailable(reason) => {
            tracing::info!(name: "steering.proof_unavailable",
                 phase = reason.as_str(),
                 decision = "disable_steering");
        }
        Candidate::Invalid(_) => unreachable!("load returns invalid proof errors"),
    }
    *cache = Some(Cached {
        file,
        binary: identity,
        candidate: candidate.clone(),
    });
    Ok(candidate)
}
pub(super) fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
pub(crate) fn native_digest(path: &Path) -> Result<String> {
    let mut file = File::open(path.canonicalize().map_err(Error::state_io_from)?)
        .map_err(Error::state_io_from)?;
    let mut magic = [0; EXECUTABLE_MAGIC_BYTES];
    file.read_exact(&mut magic).map_err(Error::state_io_from)?;
    if !matches!(
        magic,
        [0x7f, b'E', b'L', b'F']
            | [0xcf | 0xce, 0xfa, 0xed, 0xfe]
            | [0xfe, 0xed, 0xfa, 0xcf | 0xce]
            | [0xca, 0xfe, 0xba, 0xbe]
            | [0xbe, 0xba, 0xfe, 0xca]
    ) {
        return Err(Error::unsupported_operation(concat!(
            "Native Claude capability verification requires its executable; wrapper ",
            "scripts and npm launchers are not supported.",
        )));
    }
    let mut hash = Sha256::new();
    hash.update(magic);
    let mut bytes = [0; DIGEST_BUFFER_BYTES];
    let mut total = EXECUTABLE_MAGIC_BYTES;
    loop {
        let count = file.read(&mut bytes).map_err(Error::state_io_from)?;
        if count == 0 {
            break;
        }
        total += count;
        if total > EXECUTABLE_BYTES {
            return Err(Error::output_limit(
                "Claude executable exceeds the capability digest limit.",
            ));
        }
        hash.update(&bytes[..count]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

#[cfg(test)]
pub(in crate::runtime::steering_canary) mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    pub(in crate::runtime::steering_canary) fn fixture() -> (tempfile::TempDir, PathBuf, Proof) {
        let root = crate::runtime::test_directory();
        drop(
            state::State::new(root.path().into())
                .unwrap()
                .initialize()
                .unwrap(),
        );
        state::directory(&root.path().join("runtime")).unwrap();
        let binary = root.path().join("native");
        std::fs::write(&binary, b"\x7fELFdeterministic-native-fixture").unwrap();
        let proof = Proof {
            binary: BinaryIdentity::read(&binary).unwrap(),
            digest: native_digest(&binary).unwrap(),
            version: "2.1.test".into(),
            adapter: ADAPTER,
            model: "claude-sonnet-test".into(),
            effort: "low".into(),
            receipt_mode: ReceiptMode::NativeEcho,
            session: "session".into(),
            turn: "turn".into(),
            receipt: Some(
                object(&serde_json::json!({"turnId":"turn","deliveryEvidence":{
                "kind":"native_echo","observation":"native_echo","hookReply":{
                    "type":"control_response","response":{"subtype":"success","request_id":"post",
                    "response":{"hookSpecificOutput":{"hookEventName":"PostToolUse",
                    "additionalContext":guidance("01234567-89ab-4cde-8f01-23456789abcd")}}}}}}))
                .unwrap(),
            ),
            final_output_sha256: hash(b"output"),
            nonce_sha256: hash(b"01234567-89ab-4cde-8f01-23456789abcd"),
            verified_at: 1,
        };
        (root, binary, proof)
    }
    #[test]
    fn proof_is_scoped_to_binary_model_effort_and_exact_receipt_mode() {
        let (root, binary, proof) = fixture();
        assert_eq!(
            load(root.path(), &binary)
                .unwrap()
                .selection(Some("claude-sonnet-test"), "low")
                .reason(),
            SteeringObservation::ProofMissing
        );
        write(&path(root.path()), Some(proof)).unwrap();
        let candidate = load(root.path(), &binary).unwrap();
        assert_eq!(
            candidate
                .selection(Some("claude-sonnet-test"), "low")
                .mode(),
            Some(ReceiptMode::NativeEcho)
        );
        assert!(
            candidate
                .selection(Some("claude-opus-test"), "low")
                .mode()
                .is_none()
        );
        assert!(
            candidate
                .selection(Some("claude-sonnet-test"), "high")
                .mode()
                .is_none()
        );
        std::fs::write(&binary, b"\x7fELFreplacement-same-version").unwrap();
        assert!(candidate.check_launch().is_err());
        assert_eq!(
            load(root.path(), &binary)
                .unwrap()
                .selection(None, "low")
                .reason(),
            SteeringObservation::ProofStaleBinaryChanged
        );
    }
    #[test]
    fn cache_observes_revocation_and_digest_disagreement() {
        let (root, binary, mut proof) = fixture();
        write(&path(root.path()), Some(proof.clone())).unwrap();
        assert!(
            load(root.path(), &binary)
                .unwrap()
                .selection(Some(&proof.model), &proof.effort)
                .mode()
                .is_some()
        );
        write(&path(root.path()), None).unwrap();
        assert!(
            load(root.path(), &binary)
                .unwrap()
                .selection(Some(&proof.model), &proof.effort)
                .mode()
                .is_none()
        );
        proof.digest = hash(b"different content");
        write(&path(root.path()), Some(proof)).unwrap();
        assert_eq!(
            load(root.path(), &binary)
                .unwrap()
                .selection(None, "low")
                .reason(),
            SteeringObservation::ProofStaleBinaryChanged
        );
    }
    #[test]
    fn corrupt_receipt_cannot_enable_an_otherwise_current_proof() {
        let (root, binary, proof) = fixture();
        for (pointer, value) in [
            ("", Value::Null),
            ("", serde_json::json!({"unexpected":true})),
            ("/deliveryEvidence", serde_json::json!([])),
            ("/turnId", serde_json::json!("another-turn")),
            (
                "/deliveryEvidence/kind",
                serde_json::json!("written_continuation"),
            ),
            (
                "/deliveryEvidence/observation",
                serde_json::json!("successful_terminal_result"),
            ),
            (
                "/deliveryEvidence/hookReply/type",
                serde_json::json!("control_request"),
            ),
            (
                "/deliveryEvidence/hookReply/response/subtype",
                serde_json::json!("error"),
            ),
            (
                "/deliveryEvidence/hookReply/response/request_id",
                serde_json::json!(""),
            ),
            (
                "/deliveryEvidence/hookReply/response/response/hookSpecificOutput/hookEventName",
                serde_json::json!("PreToolUse"),
            ),
            (
                "/deliveryEvidence/hookReply/response/response/hookSpecificOutput/additionalContext",
                serde_json::json!(guidance("01234567-89ab-4cde-8f01-23456789abce")),
            ),
        ] {
            let mut corrupt = serde_json::to_value(Record {
                proof: Some(proof.clone()),
            })
            .unwrap();
            *corrupt["proof"]["receipt"].pointer_mut(pointer).unwrap() = value;
            state::write_json(&path(root.path()), &corrupt).unwrap();
            let Err(error) = load(root.path(), &binary) else {
                panic!("corrupt receipt was accepted: {pointer}");
            };
            assert!(
                matches!(error, Error::StateInconsistent { .. }),
                "{pointer}"
            );
            assert_eq!(error.message(), "Incomplete steering capability proof.");
        }
    }
    #[test]
    fn corrupt_oversized_and_symlink_proofs_fail_explicitly() {
        let (root, binary, _) = fixture();
        let file = path(root.path());
        state::write_json(&file, &serde_json::json!({"proof":{"unexpected":true}})).unwrap();
        assert!(matches!(
            load(root.path(), &binary),
            Err(Error::StateInconsistent { .. })
        ));
        state::write_json(&file, &"x".repeat(usize::try_from(RECORD_BYTES).unwrap())).unwrap();
        assert!(matches!(
            load(root.path(), &binary),
            Err(Error::OutputLimit { .. })
        ));
        std::fs::remove_file(&file).unwrap();
        std::os::unix::fs::symlink(root.path().join("missing"), &file).unwrap();
        assert!(matches!(
            load(root.path(), &binary),
            Err(Error::UnsafeState { .. })
        ));
    }

    const PROOF_OBJECTS: &[(&str, &[&str])] = &[
        ("", &["proof"]),
        (
            "/proof",
            &[
                "binary",
                "digest",
                "version",
                "adapter",
                "model",
                "effort",
                "receipt_mode",
                "session",
                "turn",
                "receipt",
                "final_output_sha256",
                "nonce_sha256",
                "verified_at",
            ],
        ),
        (
            "/proof/binary",
            &["path", "device", "inode", "length", "modified", "changed"],
        ),
        ("/proof/receipt", &["turnId", "deliveryEvidence"]),
        (
            "/proof/receipt/deliveryEvidence",
            &["kind", "observation", "hookReply"],
        ),
        (
            "/proof/receipt/deliveryEvidence/hookReply",
            &["type", "response"],
        ),
        (
            "/proof/receipt/deliveryEvidence/hookReply/response",
            &["subtype", "request_id", "response"],
        ),
        (
            "/proof/receipt/deliveryEvidence/hookReply/response/response",
            &["hookSpecificOutput"],
        ),
        (
            "/proof/receipt/deliveryEvidence/hookReply/response/response/hookSpecificOutput",
            &["hookEventName", "additionalContext"],
        ),
    ];

    #[test]
    fn proof_records_require_exact_named_fields_and_string_enums() {
        let (root, _, proof) = fixture();
        let file = path(root.path());
        let valid = serde_json::to_value(Record { proof: Some(proof) }).unwrap();
        state::write_json(&file, &valid).unwrap();
        assert!(read(&file).unwrap().is_some());
        for (pointer, fields) in PROOF_OBJECTS {
            for field in *fields {
                let mut invalid = valid.clone();
                invalid
                    .pointer_mut(pointer)
                    .unwrap()
                    .as_object_mut()
                    .unwrap()
                    .remove(*field);
                state::write_json(&file, &invalid).unwrap();
                assert_eq!(
                    read(&file).err().unwrap().code(),
                    "state_inconsistent",
                    "missing {pointer}/{field}"
                );
            }
            let mut invalid = valid.clone();
            invalid.pointer_mut(pointer).unwrap()["unexpected"] = Value::Bool(true);
            state::write_json(&file, &invalid).unwrap();
            assert_eq!(
                read(&file).err().unwrap().code(),
                "state_inconsistent",
                "unknown field at {pointer}"
            );

            let positional: Vec<_> = fields
                .iter()
                .map(|field| valid.pointer(pointer).unwrap()[*field].clone())
                .collect();
            let mut invalid = valid.clone();
            *invalid.pointer_mut(pointer).unwrap() = Value::Array(positional);
            state::write_json(&file, &invalid).unwrap();
            assert_eq!(
                read(&file).err().unwrap().code(),
                "state_inconsistent",
                "positional object at {pointer}"
            );
        }
        for pointer in [
            "/proof/receipt_mode",
            "/proof/receipt/deliveryEvidence/kind",
        ] {
            for bad in [
                serde_json::json!("unknown"),
                serde_json::json!({"native_echo":null}),
            ] {
                let mut invalid = valid.clone();
                *invalid.pointer_mut(pointer).unwrap() = bad;
                state::write_json(&file, &invalid).unwrap();
                assert_eq!(
                    read(&file).err().unwrap().code(),
                    "state_inconsistent",
                    "invalid enum at {pointer}"
                );
            }
        }
        state::write_json(&file, &serde_json::json!({"proof":null})).unwrap();
        assert!(read(&file).unwrap().is_none());
        state::write_json(&file, &serde_json::json!([null])).unwrap();
        assert_eq!(read(&file).err().unwrap().code(), "state_inconsistent");
    }
    #[test]
    fn scripts_are_not_capability_identity_and_symlinks_resolve_to_native_content() {
        let (root, binary, proof) = fixture();
        let alias = root.path().join("alias");
        std::os::unix::fs::symlink(&binary, &alias).unwrap();
        assert_eq!(native_digest(&alias).unwrap(), proof.digest);
        let script = root.path().join("wrapper");
        std::fs::write(&script, b"#!/bin/sh\nexec claude \"$@\"\n").unwrap();
        assert!(matches!(
            native_digest(&script),
            Err(Error::UnsupportedOperation { .. })
        ));
    }
    #[test]
    fn retargeted_installation_requires_a_new_proof_for_its_next_launch() {
        let (root, binary, proof) = fixture();
        write(&path(root.path()), Some(proof.clone())).unwrap();
        let alias = root.path().join("alias");
        std::os::unix::fs::symlink(&binary, &alias).unwrap();
        let pinned = alias.canonicalize().unwrap();
        let validated = load(root.path(), &pinned).unwrap();
        let replacement = root.path().join("replacement");
        std::fs::write(&replacement, b"\x7fELFreplacement-native").unwrap();
        std::fs::remove_file(&alias).unwrap();
        std::os::unix::fs::symlink(&replacement, &alias).unwrap();
        // Already pinned launches still execute the verified image. The next
        // launch resolves the installation again and must not reuse that proof.
        validated.check_launch().unwrap();
        assert!(
            validated
                .selection(Some(&proof.model), &proof.effort)
                .mode()
                .is_some()
        );
        assert_eq!(
            load(root.path(), &alias.canonicalize().unwrap())
                .unwrap()
                .selection(Some(&proof.model), &proof.effort)
                .reason(),
            SteeringObservation::ProofStaleBinaryChanged
        );
    }
}
