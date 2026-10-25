//! External tool descriptions, separate from schema construction.

pub(super) const HIVE_CALL_DESCRIPTION: &str = concat!(
    "External callers only: durably send once and await that message's ",
    "consuming turn. Prefer Codex final_answer text; otherwise last complete ",
    "assistant text (Claude text blocks joined by newline). Large output is ",
    "an artifact. Timeout never cancels delivery; resume by message_id. Reuse ",
    "request_id and identical send arguments after a lost acceptance ",
    "response. No steer; explicit replies across later turns are not ",
    "supported. Optional attachments are ordered absolute source paths (max \
                10), captured at acceptance.",
);

pub(super) const HIVE_WAIT_MESSAGE_DESCRIPTION: &str = concat!(
    "Resume observation of an external send by its actual message_id, without ",
    "resending. Bounded timeout stops observation only. Compacted replies may ",
    "report result_expired.",
);

pub(super) const HIVE_OUTPUT_DESCRIPTION: &str = concat!(
    "Read a bounded UTF-8 page from retained output's immutable artifact. ",
    "Follow next_offset until eof; the artifact is a JSON payload containing ",
    "text.",
);
