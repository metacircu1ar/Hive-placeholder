//! Public diagnostic catalogue. Keep wording changes explicit and independently reviewed.
use super::{Error, ErrorCode};
macro_rules! messages {
    ($($name:ident: $code:ident => $message:literal),+ $(,)?) => {
        impl Error { $(pub fn $name() -> Self { Self::new(ErrorCode::$code, $message) })+ }
    };
}
messages! {
    incompatible_protocol: IncompatibleProtocol => "Unsupported Comb protocol; this build \
        supports protocol 1. Update the Comb client or server to a compatible build.",
    request_not_object: InvalidJson => "Request must be an object.",
    unsupported_format: UnsupportedFormat => "Unsupported development format. Keep the \
        original data and use a fresh root; no data was modified.",
    newer_format: NewerFormat => "This data uses a newer format. The required product release \
        is unknown to this build; keep the data unchanged and use its producing build or a \
        newer supported build.",
    internal_serialization: InternalError => "Cannot encode internal data.",
    migration_index_integrity: IndexError => "Migration index integrity check failed; the \
        original files were retained.",
    already_open: CombBusy => "Comb is already open; use its serving process.",
    socket_in_use: CombBusy => "The socket already exists. Confirm its service is stopped \
        before removing it, or choose another --socket path.",
    service_not_listening: CombUnavailable => "Comb service is not listening; start comb serve.",
    service_connection_failed: CombUnavailable => "Cannot connect to Comb service.",
    chunk_limit: ChunkLimit => "Too many chunks in one page.",
    conflict: Conflict => "Page changed or already exists; get its current hash before updating.",
    connection_lost: ConnectionLost => "Comb closed before replying.",
    missing_coverage_evidence: ConsolidationConflict => "Missing prior coverage evidence.",
    coverage_lost_after_external_edits: ConsolidationConflict => "No authenticated coverage \
        survives the external edits. Inspect the history and use recopy for an explicit full \
        copy.",
    coverage_witness_mismatch: ConsolidationConflict => "Previously copied evidence no longer \
        matches its coverage witness. Inspect the history and use recopy for an explicit full \
        copy.",
    source_not_covered: ConsolidationConflict => "Target does not cover the current source note.",
    source_coverage_lost: ConsolidationConflict => "Target no longer contains authenticated \
        source coverage.",
    checkpoint_evidence_lost: ConsolidationConflict => "Target no longer contains the \
        checkpoint's copied evidence. Inspect the history and use recopy for an explicit full \
        copy.",
    consolidation_target_limit: ConsolidationLimit => "An inbox note supports at most 128 \
        consolidation targets.",
    credential_name_exists: CredentialExists => "Credential name already exists.",
    credential_file_exists: CredentialExists => "Refusing to overwrite an existing credential \
        file.",
    credential_registry_size_limit: CredentialLimit => "Credential registry exceeds 1 MiB; \
        revoke unused entries.",
    credential_count_limit: CredentialLimit => "Revoke unused credentials first.",
    credential_registry_uncertain: RecoveryRequired => "Restart Comb after an uncertain \
        credential registry update.",
    embedding_credential_missing: EmbeddingCredentials => "The configured embedding credential \
        variable is not set.",
    embedding_endpoint_rejected: EmbeddingEndpointRejected => "Embedding endpoint also rejects \
        a minimal input; check its model and request configuration.",
    embedding_input_byte_limit: EmbeddingInputTooLarge => "Composed input exceeds its \
        65536-byte limit.",
    serve_selection_changed: CombBusy => "The selected embedding model changed during startup; \
        retry serve with the current selection.",
    serve_embedding_replace_required: EmbeddingReplaceRequired => "Changing the served \
        embedding space requires --replace-embeddings.",
    embedding_replace_required: EmbeddingReplaceRequired => "Changing the active embedding \
        space requires --replace.",
    embedding_activation_changed: EmbeddingSpaceChanged => "Embedding activation changed while \
        work was in flight.",
    query_space_mismatch: EmbeddingSpaceChanged => "Query configuration differs from the \
        active embedding space.",
    embedder_space_mismatch: EmbeddingSpaceChanged => "The embedder does not match the active \
        space.",
    http_transport_initialization: EmbeddingTransport => "Cannot initialize HTTP embedding \
        transport.",
    embedding_batch_deadline: EmbeddingTransport => "Embedding batch deadline reached.",
    embedding_reply_incomplete: EmbeddingTransport => "Embedding reply was incomplete or timed \
        out.",
    embedding_request_failed: EmbeddingTransport => "Embedding request failed or timed out.",
    embeddings_busy: EmbeddingsBusy => "Embedding query queue is full or its one-second start \
        deadline expired.",
    active_space_missing: EmbeddingsUnavailable => "No embedding space is active.",
    embed_first: EmbeddingsUnavailable => "No embedding space is active; run embed first.",
    query_model_missing: EmbeddingsUnavailable => "No query embedding model is configured.",
    filtered_vectors_missing: EmbeddingsUnavailable => "No vectors are available for the \
        filtered corpus.",
    collector_capture_only: Forbidden => "Collectors may only capture evidence.",
    external_mutation_forbidden: Forbidden => "External files are admitted through sync, not \
        mutation calls.",
    invalid_capabilities_reply: InvalidFrame => "Invalid Comb capabilities reply.",
    forbidden: Forbidden => "Invalid credential, claim or capability.",
    human_delete_only: Forbidden => "Only the human administration CLI deletes pages.",
    configured_reviewer_required: Forbidden => "Promotion requires a configured reviewer.",
    promotion_reviewer_required: Forbidden => "Promotion requires a human or configured reviewer.",
    decision_reviewer_required: Forbidden => "Proposal decisions require a human or configured \
        reviewer.",
    graph_page_limit: GraphLimit => "Graph exceeds 1000 pages; reduce depth.",
    immutable_timeline: ImmutableTimeline => "Existing evidence cannot be changed or removed; \
        append a correction.",
    import_collision: ImportCollision => "Multiple input files map to the same lowercase slug; \
        rename them before importing.",
    import_page_limit: ImportLimit => "Import at most 1000 pages at a time; select a smaller \
        directory.",
    import_byte_limit: ImportLimit => "Import at most 16 MiB at a time; select a smaller \
        directory.",
    vector_cache_integrity: IndexError => "Cannot verify vector cache integrity; the cache was \
        preserved. Check storage health. To discard cached embeddings, stop Comb, remove only \
        <root>/.comb/vectors.sqlite and its -wal, -shm and -journal files, then run comb sync \
        --rebuild with that root.",
    invalid_actor: InvalidActor => "Actor fields must contain 1 to 512 bytes without control \
        characters.",
    consolidation_run_missing: InvalidArguments => "Consolidation needs a run name.",
    incoming_argument_forbidden: InvalidArguments => "Do not supply incoming.",
    op_argument_forbidden: InvalidArguments => "Do not supply op.",
    request_identity_missing: InvalidArguments => "Missing request identity.",
    tool_arguments_not_object: InvalidArguments => "Tool arguments must be an object.",
    cache_budget_overflow: InvalidCacheBudget => "Vector cache budget is too large.",
    invalid_cache_budget: InvalidCacheBudget => "Vector cache budget must be 1..8192 MiB.",
    cache_budget_too_small: InvalidCacheBudget => "Vector cache must fit at least one vector.",
    invalid_capture: InvalidCapture => "Capture requires bounded nonempty text.",
    invalid_consolidation: InvalidConsolidation => "Consolidation needs an inbox note, an \
        ordinary target page, a summary and a run name.",
    consolidation_run_required: InvalidConsolidation => "Consolidation requires --run NAME.",
    reviewer_role_limit: InvalidCredential => "At most 64 reviewer roles are allowed.",
    invalid_credential_name: InvalidCredential => "Names must be 1..512 bytes without controls.",
    invalid_credential_identity: InvalidCredential => "Invalid credential record identity.",
    invalid_credential_count: InvalidCredential => "Too many credential records.",
    invalid_decision: InvalidDecision => "Decide a review page with a nonempty reason of at \
        most 4096 bytes.",
    graph_depth_limit: InvalidDepth => "Graph depth is bounded to 8.",
    cached_vector_size: InvalidEmbedding => "Cached vector has the wrong size; discard the \
        derived vector cache.",
    cached_vector_norm: InvalidEmbedding => "Cached vector is not finite/unit length; discard \
        the derived vector cache.",
    invalid_embedding_indices: InvalidEmbedding => "Embedding indices must be unique and in \
        range.",
    embedding_response_count: InvalidEmbedding => "Embedding response count does not match \
        inputs.",
    embedding_response_size: InvalidEmbedding => "Embedding response exceeds 8 MiB.",
    embedding_batch_result_count: InvalidEmbedding => "Embedding result count does not match \
        input count.",
    embedding_result_count: InvalidEmbedding => "Embedding result count does not match inputs.",
    embedding_input_count: InvalidEmbedding => "Expected 1..32 embedding inputs.",
    malformed_embedding_response: InvalidEmbedding => "Malformed embedding response.",
    embedding_index_missing: InvalidEmbedding => "Missing embedding index.",
    query_result_missing: InvalidEmbedding => "Missing query result.",
    query_input_count: InvalidEmbedding => "Query batch must contain 1..32 inputs.",
    query_result_count: InvalidEmbedding => "Query result count differs from input count.",
    invalid_vector_norm: InvalidEmbedding => "Vector must have a finite nonzero norm.",
    invalid_vector_components: InvalidEmbedding => "Vector must have the configured dimensions \
        and finite components.",
    embedding_batch_limit: InvalidEmbeddingBatch => "Batch limit must be 1..32.",
    embedding_job_count: InvalidEmbeddingBatch => "Batch must contain 1..32 jobs.",
    invalid_completed_batch: InvalidEmbeddingBatch => "Invalid completed batch.",
    invalid_embedding_config_file: InvalidEmbeddingConfig => "Use a valid embedding \
        configuration of at most 16384 bytes.",
    invalid_embedding_asset: InvalidEmbeddingAsset => "Embedding assets must be readable \
        regular files.",
    embedding_asset_path_required: InvalidEmbeddingConfig => "Embedding asset paths must not \
        be empty.",
    embedding_asset_base_required: InvalidEmbeddingConfig => "Use absolute asset paths when \
        reading embedding configuration from a pipe.",
    http_tokenizer_pair_required: InvalidEmbeddingConfig => "HTTP tokenizer and max_tokens \
        must be configured together.",
    http_tokenizer_token_limit: InvalidEmbeddingConfig => "HTTP tokenizer max_tokens must be \
        16..8192.",
    invalid_embedding_space: InvalidEmbeddingConfig => "Invalid embedding space or dimensions \
        (1..4096).",
    invalid_embedding_url: InvalidEmbeddingConfig => "Invalid embeddings URL.",
    unsafe_embedding_endpoint: InvalidEmbeddingConfig => "Use HTTPS (or loopback HTTP), no URL \
        credentials/query/fragment, and a 1..120 second timeout.",
    invalid_active_space: InvalidEmbeddingState => "Invalid active embedding space; rebuild \
        the derived index.",
    vector_source_missing: InvalidEmbeddingState => "Missing vector source.",
    invalid_evidence: InvalidEvidence => "Evidence needs bounded text and a source.",
    invalid_input_reply: InvalidFrame => "Comb returned an input diagnostic that does not \
        match the submitted command.",
    incomplete_frame: InvalidFrame => "Incomplete JSON frame.",
    invalid_authority_key: InvalidIdentity => "Invalid authority key.",
    invalid_import_directory: InvalidImport => "Import source must be a directory.",
    invalid_import_id: InvalidImport => "Import batch ID must be a canonical UUID from comb \
        import --status.",
    invalid_import_progress: InvalidState => "Invalid import progress record.",
    invalid_import_journal: InvalidState => "Invalid import journal; preserve it for inspection.",
    import_pending: RecoveryRequired => "An import is pending. Resume with its unchanged \
        source, or inspect comb import --status and explicitly abandon it with comb import \
        --abandon BATCH_ID.",
    import_source_changed: Conflict => "Import source changed or is unavailable. Restore its \
        original input, or use comb import --status and comb import --abandon BATCH_ID.",
    import_target_changed: Conflict => "An unfinished import target changed. Keep the edit and \
        use comb import --status and comb import --abandon BATCH_ID before starting a new \
        import.",
    import_overlap: InvalidImport => "Import destinations must be outside the source directory \
        tree.",
    import_batch_not_current: Conflict => "That import batch is not current. Use comb import \
        --status; no batch was abandoned.",
    invalid_link_limit: InvalidLimit => "Link page limit must be 1..1000.",
    list_limit_overflow: InvalidLimit => "List limit is too large.",
    invalid_list_limit: InvalidLimit => "List limit must be 1 to 1000.",
    read_operation_required: InvalidOperation => "Expected a read operation.",
    write_operation_required: InvalidOperation => "Expected a write or administration operation.",
    imported_page_not_utf8: InvalidPage => "Imported page is not UTF-8.",
    invalid_page_metadata: InvalidPage => "Invalid title, type, tags or aliases.",
    invalid_markdown_structure: InvalidPage => "Markdown does not preserve the page structure; \
        close fences and keep evidence markers reserved.",
    page_roundtrip_failed: InvalidPage => "Page does not round-trip through its Markdown \
        representation.",
    invalid_page_content: InvalidPage => "Page exceeds its limit or contains NUL.",
    page_not_utf8: InvalidPage => "Page is not UTF-8.",
    frontmatter_missing: InvalidPage => "Page needs YAML frontmatter.",
    invalid_page_timestamps: InvalidPage => "Page timestamps must be RFC3339.",
    unclosed_frontmatter: InvalidPage => "Unclosed frontmatter.",
    invalid_request: InvalidRequest => "Invalid request ID or run attribution.",
    invalid_request_id: InvalidRequestId => "Request ID must contain 1 to 128 bytes without \
        control characters.",
    invalid_caller_hash: InvalidRequest => "Content hash must be 64 lowercase hexadecimal \
        characters.",
    reversed_search_dates: InvalidSearch => "The after timestamp must not be later than before.",
    duplicate_reviewer_role: InvalidCredential => "Reviewer roles must be unique.",
    invalid_search_dates: InvalidSearch => "Date filters require RFC3339 timestamps.",
    invalid_query: InvalidSearch => "Query must contain 1..4096 bytes.",
    invalid_search: InvalidSearch => "Search needs a bounded query and limit from 1 to 100.",
    import_path_not_utf8: InvalidSlug => "Imported paths must be UTF-8.",
    page_path_not_utf8: InvalidSlug => "Non-UTF-8 page path.",
    invalid_slug: InvalidSlug => "Use slash-separated lowercase ASCII names containing \
        letters, digits, - or _.",
    socket_not_absolute: InvalidSocket => "Credential socket path must be absolute.",
    socket_filename_missing: InvalidSocket => "Socket requires a filename.",
    socket_parent_missing: InvalidSocket => "Socket requires a parent directory.",
    file_changed_during_read: Conflict => "File changed while it was being read; retry the \
        operation.",
    unsafe_socket_parent: UnsafePath => "Socket requires a private, owned directory beneath \
        safe ancestors; use a mode-0700 directory.",
    conflicting_receipt: InvalidState => "Conflicting durable replay receipt.",
    invalid_authentication_key: InvalidState => "Invalid authentication key.",
    invalid_content_digest: InvalidState => "Invalid content digest in Comb administration state.",
    invalid_consolidation_intent: InvalidState => "Invalid consolidation recovery record; \
        preserve it for inspection.",
    incomplete_consolidation: RecoveryRequired => "Consolidation receipts are incomplete \
        without a recovery intent; preserve state and resolve manually.",
    invalid_population_marker: InvalidState => "Invalid index population marker.",
    invalid_seal: InvalidState => "Invalid seal.",
    pending_receipt_wrong_page: InvalidState => "Pending receipt belongs to another page.",
    pending_receipt_conflict: InvalidState => "Pending receipt conflicts with saved authority.",
    receipt_identity_mismatch: InvalidState => "Receipt identity disagrees with its filename.",
    receipt_slug_mismatch: InvalidState => "Receipt slug disagrees with its file.",
    duplicate_evidence_id: InvalidTimeline => "Duplicate evidence ID.",
    invalid_evidence_id: InvalidTimeline => "Evidence IDs must be UUIDs.",
    invalid_evidence_source: InvalidTimeline => "Evidence needs a bounded source.",
    invalid_evidence_marker: InvalidTimeline => "Invalid evidence marker.",
    invalid_evidence_timestamp: InvalidTimeline => "Invalid evidence timestamp.",
    unstructured_timeline: InvalidTimeline => "Timeline entries need structured evidence \
        markers; use append.",
    credential_not_found: NotFound => "Credential does not exist.",
    not_found: NotFound => "Page does not exist.",
    page_limit: PageLimit => "Page exceeds 1 MiB.",
    interrupted_page_changed: RecoveryRequired => "Page changed outside the interrupted \
        mutation; preserve it and resolve manually.",
    pending_page_invalid_seal: RecoveryRequired => "Pending page does not have a valid seal.",
    shared_state_poisoned: RecoveryRequired => "Shared Comb state was poisoned; restart Comb.",
    pending_state: RecoveryRequired => "Reopen Comb before reading pending state.",
    pending_index: RecoveryRequired => "Reopen Comb before using the pending index.",
    pending_mutation: RecoveryRequired => "Reopen Comb to reconcile the pending file mutation.",
    pending_upgrade: RecoveryRequired => "An interrupted upgrade needs its registered \
        released-format reader; retain the root and backups, and run comb upgrade with the \
        matching build.",
    request_conflict: RequestConflict => "Request ID already belongs to different input or actor.",
    result_removed: ResultRemoved => "This request succeeded previously, but its page has \
        since been removed.",
    server_embedding_config: ServerEmbeddingConfig => "The running service owns its embedding \
        configuration; omit --embedding-config or stop it for offline inference.",
    file_size_limit: SizeLimit => "File exceeds its size limit.",
    input_size_limit: SizeLimit => "Input exceeds 1 MiB.",
    frame_size_limit: SizeLimit => "JSON frame exceeds its size limit.",
    output_size_limit: SizeLimit => "JSON output exceeds its size limit.",
    intent_size_limit: SizeLimit => "Mutation recovery record exceeds 16 KiB; no intent was \
        written.",
    socket_path_too_long: SocketPathTooLong => "Use --socket with a shorter absolute path.",
    reply_timeout: Timeout => "Comb reply was not received before the connection or deadline \
        ended.",
    connection_timeout: Timeout => "Connecting to Comb timed out; nothing was submitted.",
    mcp_output_timeout: Timeout => "MCP output is blocked.",
    request_frame_timeout: Timeout => "Request frame timed out.",
    uncertain_inspect: Uncertain => "Comb reply was not received. Work may have committed; \
        inspect the affected state before retrying.",
    uncertain_retry: Uncertain => "Comb reply was not received. Work may have committed; retry \
        with the same request ID.",
    unknown_tool: UnknownTool => "Unknown Comb tool.",
    unsafe_credential: UnsafeCredential => "Credential files must be private (0600).",
    unsafe_admin_mode: UnsafePath => "Comb administration directory must be private (0700).",
    unsafe_admin_directory: UnsafePath => "Comb administration directory must be private.",
    cross_device_comb: UnsafePath => "Comb content and private state must share one \
        filesystem; nested mounts are unsupported.",
    scan_symlink: UnsafePath => "Comb scan refuses symlinks.",
    unsafe_directory: UnsafePath => "Expected an owned directory without symlinks or shared \
        write access.",
    unsafe_file: UnsafePath => "Expected an owned regular file without shared write access or \
        hard links.",
    import_path_escaped: UnsafePath => "Import path escaped root.",
    import_symlink: UnsafePath => "Import refuses symlinks.",
    index_path_not_utf8: UnsafePath => "Index path must be UTF-8.",
    parent_directory_missing: UnsafePath => "Missing parent directory.",
    parent_missing: UnsafePath => "Missing parent.",
    noncanonical_filename: UnsafePath => "Noncanonical on-disk filename.",
    path_spelling_mismatch: UnsafePath => "On-disk path spelling differs from the requested \
        name; normalize it before use.",
    unsafe_admin_preparation: UnsafePath => "Refusing to clean Comb preparations with unsafe \
        or changed ownership.",
    unsafe_index_removal: UnsafePath => "Refusing to remove unsafe index files.",
    unsafe_lock: UnsafePath => "Unsafe ownership lock.",
    unsafe_socket: UnsafeSocket => "Expected a private, owned Unix socket.",
    worker_failed: WorkerFailed => "Comb worker failed; restart the serving process.",
    query_worker_stopped: WorkerFailed => "Embedding query worker stopped.",
    managed_serve_only: InvalidRequest => "Managed execution is only valid for serve.",
    service_environment_conflict: InvalidArguments => "Existing service uses another \
        environment file; uninstall it before changing service settings.",
    managed_root_missing: InvalidEmbeddingConfig => "Managed service has no root.",
    model_activation_pending: RecoveryRequired => "Model activation intent is durable; \
        configuration/index publication is pending. Retry model use none or model use-http \
        with the same --root, or serve that root to recover.",
    managed_model_changed: InvalidEmbeddingConfig => "Serving embedding identity differs from \
        the saved selection; rerun setup or model use-http for this root.",
    service_operation_committed: IoError => "Root operation committed.",
    unmanaged_service: InvalidEmbeddingConfig => "An unmanaged instance is serving this root; \
        stop it first.",
    service_setup_required: InvalidEmbeddingConfig => "Run comb setup before installing its \
        service.",
    invoked_executable_missing: IoError => "Cannot resolve invoked executable.",
    service_path_conflict: InvalidArguments => "Existing service uses another PATH; uninstall \
        it before changing service settings.",
    service_disappeared: InvalidState => "Service disappeared.",
}
impl Error {
    pub fn io(e: std::io::Error) -> Self {
        Self::new(
            ErrorCode::IoError,
            format!("I/O operation failed ({:?}).", e.kind()),
        )
        .caused_by(e)
    }
    pub fn index(e: rusqlite::Error) -> Self {
        let message = e.sqlite_error().map_or_else(
            || "Index operation failed (database wrapper error).".to_owned(),
            |cause| {
                format!(
                    "Index operation failed (SQLite primary code {}).",
                    cause.extended_code & 255
                )
            },
        );
        Self::new(ErrorCode::IndexError, message).caused_by(e)
    }
    pub fn json(e: serde_json::Error) -> Self {
        Self::new(
            ErrorCode::InvalidJson,
            format!(
                "Invalid JSON ({:?}, line {}, column {}).",
                e.classify(),
                e.line(),
                e.column()
            ),
        )
        .caused_by(e)
    }
    pub fn frontmatter(e: serde_yaml_ng::Error) -> Self {
        let message = e.location().map_or_else(
            || "Invalid YAML.".to_owned(),
            |at| format!("Invalid YAML (line {}, column {}).", at.line(), at.column()),
        );
        Self::new(ErrorCode::InvalidFrontmatter, message).caused_by(e)
    }
}
impl Error {
    pub fn unsupported_index(path: &std::path::Path) -> Self {
        Self::new(
            ErrorCode::UnsupportedIndex,
            format!(
                "Unsupported index structure at {}. Rebuild index.sqlite with sync \
                    --rebuild; an unsupported vectors.sqlite cache may be removed for \
                    recomputation.",
                path.display()
            ),
        )
    }
    pub fn http_input_too_large(limit: usize) -> Self {
        Self::new(
            ErrorCode::EmbeddingInputTooLarge,
            format!("Input exceeds the local HTTP tokenizer limit of {limit} tokens."),
        )
    }
    pub fn embedding_input_rejected(status: u16) -> Self {
        Self::new(
            ErrorCode::EmbeddingInputRejected,
            format!("Embedding endpoint rejected input (HTTP {status})."),
        )
    }
    pub fn embedding_http_status(status: u16) -> Self {
        Self::new(
            ErrorCode::EmbeddingTransport,
            format!("Embedding endpoint returned HTTP {status}."),
        )
    }
    pub fn embedding_asset_size(path: &std::path::Path) -> Self {
        Self::new(
            ErrorCode::EmbeddingAssetSize,
            format!("Embedding asset exceeds its size limit: {}", path.display()),
        )
    }
    pub fn http_tokenizer_load(e: Box<dyn std::error::Error + Send + Sync>) -> Self {
        Self::new(
            ErrorCode::EmbeddingModel,
            format!("Cannot load HTTP preflight tokenizer: {e}"),
        )
        .caused_by_boxed(e)
    }
    pub fn http_tokenizer_truncation(e: Box<dyn std::error::Error + Send + Sync>) -> Self {
        Self::new(
            ErrorCode::EmbeddingModel,
            format!("Cannot disable HTTP tokenizer truncation: {e}"),
        )
        .caused_by_boxed(e)
    }
    pub fn http_tokenizer_encode(e: Box<dyn std::error::Error + Send + Sync>) -> Self {
        Self::new(
            ErrorCode::EmbeddingModel,
            format!("Cannot tokenize HTTP input: {e}"),
        )
        .caused_by_boxed(e)
    }
    pub fn busy(lane: &str) -> Self {
        Self::new(
            ErrorCode::CombBusy,
            format!("Comb {lane} capacity is unavailable."),
        )
    }
}

impl Error {
    pub fn credential_save_failed(id: impl std::fmt::Display, error: Self, revoked: bool) -> Self {
        let message = if revoked {
            format!("Credential {id} could not be saved and has been revoked.")
        } else {
            format!(
                "Credential {id} could not be saved; revocation is unconfirmed. Inspect \
                    credential list and revoke it with credential revoke {id}."
            )
        };
        Self::new(error.code, message).caused_by(error)
    }
    pub fn credential_publication_uncertain(id: impl std::fmt::Display, error: Self) -> Self {
        Self::new(
            error.code,
            format!(
                "Credential {id} was published, but its durability or output path is \
            uncertain. It remains active; inspect the file before retrying."
            ),
        )
        .committed()
        .caused_by(error)
    }
    pub fn individual_input_rejected(error: Self) -> Self {
        Self::new(
            error.code,
            if error.code == ErrorCode::EmbeddingInputTooLarge {
                "Composed input exceeds its byte or tokenizer limit."
            } else {
                "Embedding endpoint rejected this individual input; inspect content and \
                model configuration, then retry failed inputs."
            },
        )
        .caused_by(error)
    }
}

impl Error {
    pub fn io_open(e: std::io::Error) -> Self {
        Self::new(
            ErrorCode::IoError,
            format!("I/O operation failed ({:?}).", e.kind()),
        )
        .caused_by(super::OpeningFile(e))
    }
}

impl Error {
    pub fn setup_configuration(message: impl Into<std::borrow::Cow<'static, str>>) -> Self {
        Self::new(ErrorCode::InvalidEmbeddingConfig, message)
    }
    pub fn provision_configuration(message: impl Into<std::borrow::Cow<'static, str>>) -> Self {
        Self::new(ErrorCode::InvalidEmbeddingConfig, message)
    }
    pub fn maintenance_configuration(message: impl Into<std::borrow::Cow<'static, str>>) -> Self {
        Self::new(ErrorCode::InvalidArguments, message)
    }
    pub fn service_io(error: &std::io::Error) -> Self {
        Self::new(ErrorCode::IoError, error.to_string())
    }
    pub fn service_restart_failed(mut error: Self, restart: &std::io::Error) -> Self {
        error.message = format!("{}; service restart failed: {}", error.message, restart).into();
        error
    }
    pub fn service_environment_missing(missing: &[String]) -> Self {
        Self::new(
            ErrorCode::InvalidEmbeddingConfig,
            format!("Service environment is missing: {}", missing.join(", ")),
        )
    }
    pub fn service_environment_required(missing: &[String]) -> Self {
        Self::new(
            ErrorCode::InvalidArguments,
            format!(
                "Service needs --service-environment containing: {}",
                missing.join(", ")
            ),
        )
    }
    pub fn service_model_environment_missing(missing: &[String]) -> Self {
        Self::new(
            ErrorCode::InvalidArguments,
            format!(
                "Owned service environment file needs: {}. No model change was made.",
                missing.join(", ")
            ),
        )
    }
}
