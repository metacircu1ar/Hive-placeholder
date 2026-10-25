-- Current runtime schema template. Shared counter expressions are assembled by schema/queries.rs.
CREATE TABLE agent_instructions(
    agent TEXT PRIMARY KEY REFERENCES agents(id),
    snapshot INTEGER NOT NULL REFERENCES instruction_snapshots(id)
);

CREATE TABLE agents(
    id TEXT PRIMARY KEY,
    data TEXT NOT NULL,
    process_options TEXT NOT NULL,
    reserved_process_options TEXT,
    comb_bound INTEGER GENERATED ALWAYS AS (json_extract(data, '$.comb') IS NOT NULL) VIRTUAL,
    parent_id TEXT,
    auth TEXT,
    auth_group TEXT,
    auth_checked_revision INTEGER NOT NULL,
    waiting_reason TEXT,
    quota_wait_until INTEGER,
    waiting INTEGER GENERATED ALWAYS AS (waiting_reason IS NOT NULL) VIRTUAL,
    lifetime TEXT,
    state TEXT,
    task_outcome TEXT,
    paused INTEGER,
    expired INTEGER,
    thread_id TEXT,
    execution_id TEXT,
    turn_id TEXT,
    control TEXT,
    completion_pending TEXT,
    result TEXT,
    error TEXT,
    provider_id TEXT,
    provider_pid INTEGER,
    provider_spawn_before INTEGER,
    provider_spawn_after INTEGER,
    listing_seq INTEGER,
    ended_at INTEGER CHECK (ended_at IS NULL
            OR (typeof(ended_at) = 'integer'
                AND ended_at >= 0)),
    branch_hold INTEGER NOT NULL DEFAULT 0 CHECK (branch_hold >= 0),
    branch_gate TEXT CHECK (branch_gate IS NULL
            OR branch_gate IN ('waiting_for_source', 'ready_to_fork', 'forking', 'bound', 'failed',
                'uncertain')),
    branch_priority INTEGER NOT NULL DEFAULT 1 CHECK (branch_priority IN (0, 1)),
    coordinator INTEGER NOT NULL DEFAULT 0,
    claimed INTEGER NOT NULL DEFAULT 0,
    blocked INTEGER NOT NULL DEFAULT 0,
    pending_count INTEGER NOT NULL DEFAULT 0,
    mail_count INTEGER NOT NULL DEFAULT 0,
    pending_seq INTEGER,
    head_startup INTEGER,
    priority INTEGER GENERATED ALWAYS AS (CASE
        WHEN coordinator = 1 THEN 0
        WHEN mail_count > 0 THEN 1
        ELSE 2
        END) VIRTUAL,
    admission_class INTEGER GENERATED ALWAYS AS (CASE
        WHEN branch_gate = 'ready_to_fork' THEN branch_priority
        WHEN head_startup = 0 THEN CASE
        WHEN coordinator = 1 THEN 0
        ELSE 1
        END
        WHEN parent_id IS NOT NULL THEN 2
        ELSE 3
        END) VIRTUAL,
    runnable INTEGER GENERATED ALWAYS AS (branch_hold = 0
            AND (branch_gate IS NULL
                OR branch_gate IN ('bound', 'ready_to_fork'))
            AND claimed = 0
            AND blocked = 0
            AND waiting = 0
            AND paused = 0
            AND expired = 0
            AND task_outcome NOT IN ('completed', 'cancelled')
            AND state IN ('starting', 'idle', 'stopped')
            AND control IS NULL
            AND completion_pending IS NULL
            AND (pending_count > 0
                OR state = 'starting')) VIRTUAL,
    CHECK (((task_outcome IN ('completed', 'cancelled')
                    OR expired = 1) = (ended_at IS NOT NULL)) IS TRUE),
    CHECK ((pending_seq IS NULL
                AND head_startup IS NULL)
            OR (pending_seq IS NOT NULL
                AND head_startup IS NOT NULL
                AND head_startup IN (0, 1)))
);

CREATE TABLE artifacts(
    id TEXT PRIMARY KEY,
    bytes INTEGER NOT NULL,
    created_at INTEGER NOT NULL,
    agent TEXT REFERENCES agents(id),
    join_id TEXT REFERENCES joins(id),
    CHECK (agent IS NOT NULL OR join_id IS NOT NULL)
);

CREATE TABLE approvals(
    id TEXT PRIMARY KEY,
    agent TEXT NOT NULL REFERENCES agents(id),
    execution TEXT NOT NULL,
    turn TEXT NOT NULL,
    title TEXT,
    message TEXT NOT NULL,
    schema TEXT,
    destructive INTEGER NOT NULL CHECK (destructive IN (0, 1)),
    state TEXT NOT NULL CHECK (state IN ('pending', 'answered', 'superseded', 'withdrawn')),
    action TEXT CHECK (action IN ('accept', 'decline', 'cancel')),
    content TEXT,
    reason TEXT,
    created_at INTEGER NOT NULL CHECK (created_at >= 0),
    closed_at INTEGER,
    response_message TEXT,
    CHECK ((state = 'pending'
                AND closed_at IS NULL
                AND action IS NULL
                AND content IS NULL
                AND reason IS NULL
                AND response_message IS NULL)
            OR (state = 'answered'
                AND closed_at IS NOT NULL
                AND action IS NOT NULL
                AND response_message IS NOT NULL
                AND (action = 'accept'
                    OR content IS NULL)
                AND (action = 'decline'
                    OR reason IS NULL))
            OR (state IN ('superseded', 'withdrawn')
                AND closed_at IS NOT NULL
                AND action IS NULL
                AND content IS NULL
                AND reason IS NULL
                AND response_message IS NULL))
);

CREATE UNIQUE INDEX approvals_pending
ON approvals(agent)
WHERE state = 'pending';

CREATE INDEX approvals_listing
ON approvals(created_at, id)
WHERE state = 'pending';

CREATE INDEX approvals_history
ON approvals(created_at, id);

CREATE INDEX approvals_agent_history
ON approvals(agent, created_at, id);

CREATE INDEX artifacts_agent
ON artifacts(agent);

CREATE TABLE artifact_deletions(
    id TEXT PRIMARY KEY
);

CREATE TRIGGER artifacts_no_pending_reuse BEFORE INSERT ON artifacts
WHEN EXISTS (
    SELECT 1
    FROM artifact_deletions
    WHERE id = new.id) BEGIN
    SELECT RAISE(ABORT, 'Artifact identity is pending deletion');
END;

CREATE TABLE purged_agents(
    id TEXT PRIMARY KEY,
    parent_id TEXT
);

CREATE INDEX purged_agents_parent
ON purged_agents(parent_id);

CREATE INDEX agent_instructions_snapshot
ON agent_instructions(snapshot);

CREATE TABLE deliveries(
    message_id TEXT PRIMARY KEY,
    sender_turn TEXT,
    recipient_turn TEXT,
    execution TEXT,
    provider TEXT,
    thread TEXT,
    status TEXT NOT NULL DEFAULT 'accepted',
    error TEXT,
    call_expired INTEGER NOT NULL DEFAULT 0 CHECK (call_expired IN (0, 1))
);

CREATE TABLE call_results(
    message_id TEXT PRIMARY KEY REFERENCES deliveries(message_id),
    accepted_at INTEGER NOT NULL,
    start_seq INTEGER,
    output TEXT,
    output_final INTEGER NOT NULL DEFAULT 0 CHECK (output_final IN (0, 1))
);

CREATE TABLE event_retention(
    agent TEXT PRIMARY KEY,
    pruned_through INTEGER NOT NULL
);

CREATE TABLE events(
    seq INTEGER PRIMARY KEY AUTOINCREMENT,
    agent TEXT NOT NULL,
    execution TEXT,
    at INTEGER NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('auth_switched', 'auth_switch_requested', 'auth_switch_deferred',
                'member_skipped', 'continuation_queued', 'approval_requested', 'approval_closed',
                'assistant_message', 'completion_requested', 'control_requested', 'execution_failed',
                'execution_starting', 'execution_stopped', 'lifetime_expired', 'message_accepted',
                'message_promoted', 'message_schedule_failed', 'redirect_accepted', 'steering_submitted',
                'steering_rejected', 'steering_uncertain', 'model_rerouted', 'options_changed',
                'provider_error', 'provider_session_released', 'provider_shutdown', 'provider_started',
                'session_quarantined', 'session_ready', 'spawned', 'join_requested', 'join_completed', 'join_cancelled', 'join_deadline_passed', 'branch_requested', 'branch_ready',
                'branch_failed', 'branch_context_dropped', 'task_completed', 'termination_requested',
                'terminal_cleanup_observed', 'thread_bound', 'tool_completed', 'turn_finished',
                'turn_started', 'evidence_submitted', 'collector_overflow')),
    data TEXT NOT NULL
);

CREATE TABLE instruction_snapshots(
    id INTEGER PRIMARY KEY,
    body TEXT NOT NULL UNIQUE
);

CREATE TABLE messages(
    seq INTEGER PRIMARY KEY AUTOINCREMENT,
    id TEXT NOT NULL,
    sender TEXT NOT NULL,
    recipient TEXT NOT NULL REFERENCES agents(id),
    body TEXT NOT NULL,
    has_attachments INTEGER NOT NULL DEFAULT 0 CHECK (has_attachments IN (0, 1)),
    state TEXT NOT NULL,
    startup INTEGER NOT NULL DEFAULT 0 CHECK (startup IN (0, 1)),
    mode TEXT NOT NULL DEFAULT 'next-turn',
    redirect INTEGER NOT NULL DEFAULT 0 CHECK (redirect IN (0, 1)),
    kind TEXT CHECK (kind IS NULL
            OR kind IN ('approval_answer', 'continuation', 'join_result')),
    join_id TEXT REFERENCES joins(id) ON DELETE SET NULL,
    continuation_root TEXT,
    continuation_attempt INTEGER,
    not_before INTEGER CHECK (not_before IS NULL
            OR (typeof(not_before) = 'integer'
                AND not_before >= 0)),
    schedule_integrity TEXT CHECK (schedule_integrity IS NULL
            OR (schedule_integrity = 'invalid_delivery_proof'
                AND not_before IS NOT NULL
                AND state IN ('failed', 'uncertain'))),
    CHECK ((kind IS 'continuation'
                AND continuation_root IS NOT NULL
                AND typeof(continuation_attempt) = 'integer'
                AND continuation_attempt > 0)
            OR (kind IS NOT 'continuation'
                AND continuation_root IS NULL
                AND continuation_attempt IS NULL)),
    CHECK (state != 'scheduled'
            OR (not_before IS NOT NULL
                AND mode = 'next-turn'
                AND startup = 0
                AND redirect = 0
                AND kind IS NULL
                AND schedule_integrity IS NULL)),
    UNIQUE(id)
);

CREATE TABLE agent_image_usage(
    agent TEXT PRIMARY KEY REFERENCES agents(id) ON DELETE CASCADE,
    bytes INTEGER NOT NULL CHECK(bytes >= 0 AND bytes <= 8388608),
    images INTEGER NOT NULL CHECK(images >= 0 AND images <= 20)
);
CREATE TABLE attachment_blobs(
    key TEXT PRIMARY KEY CHECK (length(key) = 64),
    filename TEXT NOT NULL,
    bytes INTEGER NOT NULL CHECK (bytes >= 0 AND bytes <= 20971520)
);
CREATE TABLE message_attachments(
    message_id TEXT NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    position INTEGER NOT NULL CHECK (position >= 0 AND position < 10),
    snapshot_key TEXT REFERENCES attachment_blobs(key),
    data TEXT NOT NULL CHECK (json_valid(data)),
    PRIMARY KEY(message_id, position)
);
CREATE INDEX message_attachment_snapshot ON message_attachments(snapshot_key)
WHERE snapshot_key IS NOT NULL;
CREATE TABLE pending_attachment_deletions(
    key TEXT PRIMARY KEY REFERENCES attachment_blobs(key)
);
CREATE TRIGGER attachment_last_reference AFTER DELETE ON message_attachments
WHEN OLD.snapshot_key IS NOT NULL BEGIN
    INSERT OR IGNORE INTO pending_attachment_deletions(key)
    SELECT OLD.snapshot_key
    WHERE NOT EXISTS (SELECT 1 FROM message_attachments WHERE snapshot_key = OLD.snapshot_key);
END;
CREATE TRIGGER attachment_reference_insert AFTER INSERT ON message_attachments BEGIN
    DELETE FROM pending_attachment_deletions WHERE key = NEW.snapshot_key;
    UPDATE messages SET has_attachments = 1 WHERE id = NEW.message_id AND has_attachments = 0;
END;

CREATE TABLE provider_events(
    seq INTEGER PRIMARY KEY AUTOINCREMENT,
    provider TEXT NOT NULL,
    at INTEGER NOT NULL,
    kind TEXT NOT NULL,
    data TEXT NOT NULL
);

CREATE TABLE redirect_interrupts(
    agent TEXT NOT NULL REFERENCES agents(id),
    execution TEXT NOT NULL,
    input_message TEXT NOT NULL,
    turn TEXT,
    state TEXT NOT NULL CHECK (state IN ('pending', 'sent', 'resolved', 'target_lost')),
    PRIMARY KEY(execution, input_message)
);

CREATE INDEX redirect_agent
ON redirect_interrupts(agent, state);

CREATE TABLE requests(
    sender TEXT NOT NULL,
    id TEXT NOT NULL,
    body TEXT NOT NULL,
    response TEXT NOT NULL,
    protocol INTEGER NOT NULL CHECK (typeof(protocol) = 'integer' AND protocol > 0 AND protocol <= 4294967295),
    PRIMARY KEY(sender, id)
);

CREATE TABLE runtime_counts(
    id INTEGER PRIMARY KEY CHECK (id = 1),
    active INTEGER NOT NULL,
    last_agent_seq INTEGER NOT NULL CHECK (typeof(last_agent_seq) = 'integer'
            AND last_agent_seq >= 0)
);

CREATE VIEW agent_records AS
SELECT listing_seq AS seq, id, json_set(data, '$.branch_hold', branch_hold, '$.branch_phase',
        branch_gate, '$.ended_at', ended_at, '$.parent_id', parent_id, '$.auth', auth, '$.auth_group',
        auth_group, '$.auth_checked_revision', auth_checked_revision, '$.waiting_reason', waiting_reason,
        '$.quota_wait_until', quota_wait_until, '$.lifetime', lifetime, '$.state', state, '$.task_outcome',
        task_outcome, '$.paused', json(CASE paused
        WHEN 1 THEN 'true'
        ELSE 'false'
        END), '$.expired', json(CASE expired
        WHEN 1 THEN 'true'
        ELSE 'false'
        END), '$.thread_id', thread_id, '$.execution_id', execution_id, '$.turn_id', turn_id,
        '$.control', control, '$.completion_pending', completion_pending, '$.result', result,
        '$.error', error, '$.provider_id', provider_id, '$.provider_pid', provider_pid, '$.provider_spawn_before',
        provider_spawn_before, '$.provider_spawn_after', provider_spawn_after) AS data
FROM agents;

CREATE INDEX agents_ended
ON agents(ended_at, id)
WHERE ended_at IS NOT NULL;

CREATE TRIGGER agents_end_immutable BEFORE UPDATE OF ended_at ON agents
WHEN old.ended_at IS NOT NULL
    AND new.ended_at IS NOT old.ended_at BEGIN
    SELECT RAISE(ABORT, 'Agent ending timestamp is immutable');
END;

CREATE INDEX agents_auth
ON agents(auth);

CREATE TABLE auth_load(
    profile TEXT NOT NULL PRIMARY KEY,
    bound_agents INTEGER NOT NULL CHECK(bound_agents > 0)
);

CREATE INDEX agents_auth_active
ON agents(auth)
WHERE turn_id IS NOT NULL;

CREATE TRIGGER agents_load_insert AFTER INSERT ON agents
WHEN new.ended_at IS NULL BEGIN
    INSERT INTO auth_load VALUES(new.auth, 1)
        ON CONFLICT(profile) DO UPDATE SET bound_agents = bound_agents + 1;
END;

CREATE TRIGGER agents_load_update AFTER UPDATE OF auth, ended_at ON agents
WHEN old.auth IS NOT new.auth OR old.ended_at IS NOT new.ended_at BEGIN
    DELETE FROM auth_load WHERE old.ended_at IS NULL AND profile = old.auth AND bound_agents = 1;
    UPDATE auth_load SET bound_agents = bound_agents - 1
        WHERE old.ended_at IS NULL AND profile = old.auth;
    INSERT INTO auth_load SELECT new.auth, 1 WHERE new.ended_at IS NULL
        ON CONFLICT(profile) DO UPDATE SET bound_agents = bound_agents + 1;
END;

CREATE TRIGGER agents_load_delete AFTER DELETE ON agents
WHEN old.ended_at IS NULL BEGIN
    DELETE FROM auth_load WHERE profile = old.auth AND bound_agents = 1;
    UPDATE auth_load SET bound_agents = bound_agents - 1 WHERE profile = old.auth;
END;

CREATE TABLE process_variants(
    auth TEXT NOT NULL,
    process_options TEXT NOT NULL,
    refs INTEGER NOT NULL CHECK(refs >= 0),
    PRIMARY KEY(auth, process_options)
);

CREATE TRIGGER agents_variant_insert AFTER INSERT ON agents
WHEN new.ended_at IS NULL BEGIN
    INSERT INTO process_variants VALUES(new.auth, new.process_options, 1)
        ON CONFLICT(auth, process_options) DO UPDATE SET refs = refs + 1;
    INSERT INTO process_variants SELECT new.auth, new.reserved_process_options, 1
        WHERE new.reserved_process_options IS NOT NULL
        ON CONFLICT(auth, process_options) DO UPDATE SET refs = refs + 1;
END;

CREATE TRIGGER agents_variant_update AFTER UPDATE OF auth, process_options, reserved_process_options, ended_at ON agents
WHEN old.auth IS NOT new.auth OR old.process_options IS NOT new.process_options
    OR old.reserved_process_options IS NOT new.reserved_process_options
    OR old.ended_at IS NOT new.ended_at BEGIN
    UPDATE process_variants SET refs = refs - 1
        WHERE old.ended_at IS NULL AND auth = old.auth AND process_options = old.process_options;
    UPDATE process_variants SET refs = refs - 1
        WHERE old.ended_at IS NULL AND auth = old.auth AND process_options = old.reserved_process_options;
    DELETE FROM process_variants WHERE refs = 0;
    INSERT INTO process_variants SELECT new.auth, new.process_options, 1 WHERE new.ended_at IS NULL
        ON CONFLICT(auth, process_options) DO UPDATE SET refs = refs + 1;
    INSERT INTO process_variants SELECT new.auth, new.reserved_process_options, 1
        WHERE new.ended_at IS NULL AND new.reserved_process_options IS NOT NULL
        ON CONFLICT(auth, process_options) DO UPDATE SET refs = refs + 1;
END;

CREATE TRIGGER agents_variant_delete AFTER DELETE ON agents
WHEN old.ended_at IS NULL BEGIN
    UPDATE process_variants SET refs = refs - 1
        WHERE auth = old.auth AND process_options = old.process_options;
    UPDATE process_variants SET refs = refs - 1
        WHERE auth = old.auth AND process_options = old.reserved_process_options;
    DELETE FROM process_variants WHERE refs = 0;
END;

CREATE INDEX agents_controls
ON agents(id)
WHERE claimed = 0
    AND (control = 'stop'
        OR completion_pending IS NOT NULL);

CREATE UNIQUE INDEX agents_listing
ON agents(listing_seq);

CREATE INDEX agents_parent
ON agents(parent_id, lifetime);

CREATE INDEX agents_dependents
ON agents(parent_id, listing_seq)
WHERE lifetime = 'with_parent'
    AND expired = 0;

CREATE INDEX agents_provider
ON agents(provider_id);

CREATE INDEX agents_ready
ON agents(admission_class, pending_seq, id)
WHERE runnable = 1;

CREATE INDEX events_agent_sequence
ON events(agent, seq);

CREATE INDEX events_time
ON events(at, seq);

CREATE INDEX messages_scheduled
ON messages(not_before, seq)
WHERE state = 'scheduled';

CREATE INDEX messages_inbox
ON messages(recipient, state, mode, seq);

CREATE INDEX messages_pending
ON messages(recipient, mode, startup DESC, redirect DESC, seq)
WHERE state = 'accepted';

CREATE INDEX messages_recipient_history
ON messages(recipient, seq);

CREATE INDEX messages_sender_history
ON messages(sender, seq);

CREATE INDEX messages_steering
ON messages(recipient, state)
WHERE mode = 'steer';

CREATE INDEX provider_events_sequence
ON provider_events(provider, seq);

CREATE TRIGGER agents_count_delete AFTER DELETE ON agents BEGIN
    UPDATE runtime_counts
    SET active = active - ({{active:old}})
    WHERE id = 1;
END;

CREATE TRIGGER agents_count_insert AFTER INSERT ON agents BEGIN
    UPDATE runtime_counts
    SET last_agent_seq = max(last_agent_seq, coalesce(new.listing_seq, last_agent_seq + 1))
    WHERE id = 1;
    UPDATE agents
    SET listing_seq = (
        SELECT last_agent_seq
        FROM runtime_counts
        WHERE id = 1)
    WHERE id = new.id
        AND listing_seq IS NULL;
    UPDATE runtime_counts
    SET active = active + ({{active:new}})
    WHERE id = 1;
    UPDATE agents
    SET coordinator = 1
    WHERE id = new.parent_id
        AND coordinator != 1;
    UPDATE agents
    SET coordinator = 1
    WHERE id = new.id
        AND new.parent_id IS NULL
        AND coordinator != 1;
END;

CREATE TRIGGER agents_no_reuse BEFORE INSERT ON agents
WHEN EXISTS (
    SELECT 1
    FROM purged_agents
    WHERE id = new.id) BEGIN
    SELECT RAISE(ABORT, 'Purged agent identities cannot be reused');
END;

CREATE TRIGGER agents_count_update AFTER UPDATE OF state, task_outcome, expired ON agents
WHEN (({{active:new}}) = ({{active:old}})) IS NOT TRUE BEGIN
    UPDATE runtime_counts
    SET active = active + ({{active:new}}) - ({{active:old}})
    WHERE id = 1;
END;

CREATE TRIGGER delivery_state AFTER UPDATE OF state ON messages BEGIN
    UPDATE deliveries
    SET status = new.state, error = CASE
    WHEN new.state = 'stale_turn' THEN 'stale_turn'
    ELSE error
    END
    WHERE message_id = new.id;
END;

-- A non-pending message cannot affect either mailbox counter. IS NOT FALSE
-- keeps unexpected NULL contributions on the rejecting arithmetic path.
CREATE TRIGGER messages_ready_delete AFTER DELETE ON messages
WHEN ({{pending:old}}) IS NOT FALSE BEGIN
    UPDATE agents
    SET pending_count = pending_count - ({{pending:old}}), mail_count = mail_count - ({{mail:old}}),
        pending_seq = ({{pending_sequence:old}}), head_startup = ({{head_startup:old}})
    WHERE id = old.recipient;
END;

CREATE TRIGGER messages_ready_insert AFTER INSERT ON messages
WHEN ({{pending:new}}) IS NOT FALSE BEGIN
    UPDATE agents
    SET pending_count = pending_count + ({{pending:new}}), mail_count = mail_count + ({{mail:new}}),
        pending_seq = ({{pending_sequence:new}}), head_startup = ({{head_startup:new}})
    WHERE id = new.recipient;
END;

-- One recipient needs one net change. Admission age comes from the oldest
-- waiting input; next-work kind comes from the priority head, in one UPDATE.
CREATE TRIGGER messages_ready_update AFTER UPDATE OF state, startup, recipient, mode, redirect ON messages
WHEN old.recipient = new.recipient
    AND (({{pending:old}}) = ({{pending:new}})
        AND ({{mail:old}}) = ({{mail:new}})
        AND (old.redirect = new.redirect
            OR ({{pending:new}}) = 0)) IS NOT TRUE BEGIN
    UPDATE agents
    SET pending_count = pending_count + ({{pending:new}}) - ({{pending:old}}), mail_count = mail_count + ({{mail:new}}) - ({{mail:old}}),
        pending_seq = ({{pending_sequence:new}}), head_startup = ({{head_startup:new}})
    WHERE id = new.recipient;
END;

CREATE TRIGGER messages_ready_move AFTER UPDATE OF state, startup, recipient, mode, redirect ON messages
WHEN old.recipient != new.recipient BEGIN
    UPDATE agents
    SET pending_count = pending_count - ({{pending:old}}), mail_count = mail_count - ({{mail:old}}),
        pending_seq = ({{pending_sequence:old}}), head_startup = ({{head_startup:old}})
    WHERE id = old.recipient
        AND ({{pending:old}}) IS NOT FALSE;
    UPDATE agents
    SET pending_count = pending_count + ({{pending:new}}), mail_count = mail_count + ({{mail:new}}),
        pending_seq = ({{pending_sequence:new}}), head_startup = ({{head_startup:new}})
    WHERE id = new.recipient
        AND ({{pending:new}}) IS NOT FALSE;
END;

INSERT INTO runtime_counts
VALUES (1, 0, 0);

CREATE INDEX agents_waiting
ON agents(waiting_reason, quota_wait_until, id)
WHERE waiting = 1;

CREATE TABLE login_availability(
    profile TEXT NOT NULL,
    generation TEXT NOT NULL,
    revision INTEGER NOT NULL,
    key_authority TEXT CHECK (key_authority IS NULL OR json_valid(key_authority)),
    state TEXT NOT NULL CHECK (state IN ('available', 'limited', 'unavailable')),
    until INTEGER,
    retry_at INTEGER,
    probe_id TEXT,
    probe_agent TEXT,
    probe_message TEXT,
    probe_deadline INTEGER,
    probe_error TEXT,
    window TEXT,
    reason TEXT,
    confirmed_at INTEGER,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY(profile, generation),
    CHECK ((probe_id IS NULL
                AND probe_agent IS NULL
                AND probe_message IS NULL
                AND probe_deadline IS NULL
                AND probe_error IS NULL)
            OR (probe_id IS NOT NULL
                AND probe_agent IS NOT NULL
                AND probe_message IS NOT NULL
                AND probe_deadline IS NOT NULL))
);

CREATE INDEX login_deadline
ON login_availability(coalesce(until, retry_at))
WHERE state = 'limited'
    AND probe_error IS NULL;

CREATE INDEX login_probe_deadline
ON login_availability(probe_deadline)
WHERE probe_id IS NOT NULL
    AND probe_error IS NULL;

CREATE TABLE branches(
    agent TEXT PRIMARY KEY REFERENCES agents(id),
    source TEXT NOT NULL,
    source_thread TEXT NOT NULL,
    source_execution TEXT NOT NULL,
    source_turn TEXT NOT NULL,
    phase TEXT NOT NULL CHECK (phase IN ('waiting_for_source', 'ready_to_fork', 'forking', 'bound',
                'failed', 'uncertain')),
    deadline INTEGER,
    holding INTEGER NOT NULL DEFAULT 1 CHECK (holding IN (0, 1)),
    source_outcome TEXT,
    source_handoff TEXT,
    native_thread TEXT,
    error TEXT,
    CHECK (deadline IS NULL
            OR (typeof(deadline) = 'integer'
                AND deadline >= 0))
);

CREATE INDEX branches_source
ON branches(source, agent)
WHERE holding = 1;

CREATE INDEX branches_deadline
ON branches(deadline, agent)
WHERE holding = 1
    AND deadline IS NOT NULL;

CREATE INDEX branches_phase
ON branches(phase, agent);

CREATE TRIGGER branches_insert AFTER INSERT ON branches BEGIN
    UPDATE agents
    SET branch_hold = branch_hold + new.holding
    WHERE id = new.source;
    UPDATE agents
    SET branch_gate = new.phase
    WHERE id = new.agent;
END;

CREATE TRIGGER branches_update AFTER UPDATE OF phase, holding ON branches BEGIN
    UPDATE agents
    SET branch_hold = branch_hold + new.holding - old.holding
    WHERE id = new.source;
    UPDATE agents
    SET branch_gate = new.phase
    WHERE id = new.agent;
END;

CREATE TRIGGER branches_delete AFTER DELETE ON branches BEGIN
    UPDATE agents
    SET branch_hold = branch_hold - old.holding
    WHERE id = old.source;
END;

-- A join owns its retained answers independently of the recipient lifetimes.
CREATE TABLE joins(
    id TEXT PRIMARY KEY,
    requester TEXT NOT NULL,
    request_id TEXT NOT NULL,
    created_at INTEGER NOT NULL CHECK (created_at >= 0),
    deadline INTEGER CHECK (deadline IS NULL OR deadline >= created_at),
    state TEXT NOT NULL CHECK (state IN ('collecting', 'deadline_passed', 'completed', 'cancelled')),
    dirty INTEGER NOT NULL DEFAULT 1 CHECK (dirty IN (0, 1)),
    UNIQUE(requester, request_id),
    UNIQUE(id, requester)
);
CREATE INDEX joins_open_requester ON joins(requester, id)
WHERE state IN ('collecting', 'deadline_passed');
CREATE INDEX joins_dirty ON joins(id) WHERE dirty = 1;
CREATE INDEX joins_deadline ON joins(deadline, id)
WHERE state = 'collecting' AND deadline IS NOT NULL;
CREATE TABLE join_members(
    join_id TEXT NOT NULL,
    requester TEXT NOT NULL,
    position INTEGER NOT NULL CHECK (position BETWEEN 0 AND 63),
    message_id TEXT NOT NULL UNIQUE,
    target TEXT NOT NULL,
    outcome TEXT NOT NULL CHECK (outcome IN ('pending', 'completed', 'failed', 'interrupted', 'undeliverable', 'uncertain')),
    output TEXT,
    output_final INTEGER NOT NULL DEFAULT 0 CHECK (output_final IN (0, 1)),
    task_completed TEXT,
    delivered INTEGER NOT NULL DEFAULT 0 CHECK (delivered IN (0, 1)),
    PRIMARY KEY(join_id, position),
    FOREIGN KEY(join_id, requester) REFERENCES joins(id, requester)
);
CREATE INDEX join_members_requester ON join_members(requester);
CREATE INDEX messages_join ON messages(join_id) WHERE join_id IS NOT NULL;
CREATE TRIGGER join_artifact_owner BEFORE INSERT ON artifacts
WHEN new.join_id IS NOT NULL AND NOT EXISTS (
    SELECT 1 FROM joins WHERE id = new.join_id AND requester = coalesce(new.agent, 'human')) BEGIN
    SELECT RAISE(ABORT, 'Join artifact ownership mismatch');
END;
CREATE TRIGGER join_message_finished AFTER UPDATE OF state ON messages
WHEN new.join_id IS NOT NULL AND new.state IN ('completed', 'failed', 'interrupted', 'undeliverable', 'uncertain') BEGIN
    UPDATE join_members SET outcome = new.state
    WHERE message_id = new.id AND outcome = 'pending'
        AND EXISTS (SELECT 1 FROM joins WHERE id = new.join_id AND state IN ('collecting', 'deadline_passed'));
    UPDATE joins SET dirty = 1 WHERE id = new.join_id AND state IN ('collecting', 'deadline_passed');
END;
-- Scheduled quarantine replaces a message while retaining its identity and proof.
CREATE TRIGGER join_message_terminal_insert AFTER INSERT ON messages
WHEN new.join_id IS NOT NULL AND new.state IN ('completed', 'failed', 'interrupted', 'undeliverable', 'uncertain') BEGIN
    UPDATE join_members SET outcome = new.state
    WHERE message_id = new.id AND outcome = 'pending'
        AND EXISTS (SELECT 1 FROM joins WHERE id = new.join_id AND state IN ('collecting', 'deadline_passed'));
    UPDATE joins SET dirty = 1 WHERE id = new.join_id AND state IN ('collecting', 'deadline_passed');
END;
CREATE TRIGGER join_requester_ended AFTER UPDATE OF task_outcome, expired ON agents
WHEN new.task_outcome IN ('completed', 'cancelled') OR new.expired = 1 BEGIN
    UPDATE joins SET dirty = 1 WHERE requester = new.id AND state IN ('collecting', 'deadline_passed');
END;

CREATE INDEX artifacts_join ON artifacts(join_id) WHERE join_id IS NOT NULL;

-- Installation identity separates idempotency keys across independent runtimes.
CREATE TABLE runtime_identity(id INTEGER PRIMARY KEY CHECK(id = 1), identity TEXT NOT NULL);
INSERT INTO runtime_identity VALUES(1, lower(hex(randomblob(16))));
CREATE TABLE comb_collectors(
    name TEXT PRIMARY KEY,
    credential_file TEXT NOT NULL,
    sources TEXT NOT NULL,
    enabled INTEGER NOT NULL CHECK(enabled IN (0, 1)),
    comb_id TEXT,
    credential_id TEXT,
    revision INTEGER NOT NULL DEFAULT 0,
    paused INTEGER NOT NULL DEFAULT 0 CHECK(paused IN (0, 1)),
    last_error TEXT,
    retry_at INTEGER NOT NULL DEFAULT 0,
    gap_count INTEGER NOT NULL DEFAULT 0,
    gap_first INTEGER,
    gap_last INTEGER,
    pending INTEGER NOT NULL DEFAULT 0 CHECK(pending BETWEEN 0 AND 1024),
    bytes INTEGER NOT NULL DEFAULT 0 CHECK(bytes BETWEEN 0 AND 16777216)
);
-- No event/agent foreign key: accepted evidence survives pruning and purge.
CREATE TABLE comb_outbox(
    id TEXT PRIMARY KEY,
    destination TEXT NOT NULL REFERENCES comb_collectors(name),
    sequence INTEGER NOT NULL,
    body TEXT NOT NULL,
    bytes INTEGER NOT NULL,
    generation INTEGER NOT NULL DEFAULT 0,
    comb_id TEXT,
    credential_id TEXT,
    retry_at INTEGER NOT NULL DEFAULT 0,
    last_error TEXT,
    CHECK((comb_id IS NULL) = (credential_id IS NULL))
);
CREATE INDEX comb_outbox_delivery ON comb_outbox(destination, retry_at, sequence);
CREATE TRIGGER comb_outbox_insert AFTER INSERT ON comb_outbox BEGIN
    UPDATE comb_collectors SET pending = pending + 1, bytes = bytes + NEW.bytes WHERE name = NEW.destination;
END;
CREATE TRIGGER comb_outbox_delete AFTER DELETE ON comb_outbox BEGIN
    UPDATE comb_collectors SET pending = pending - 1, bytes = bytes - OLD.bytes WHERE name = OLD.destination;
END;
