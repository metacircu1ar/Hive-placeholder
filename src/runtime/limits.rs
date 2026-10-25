//! Share control-operation bounds between validation, storage and command-line defaults.
// Bound one atomic batch before request replay storage or dispatch.
pub(crate) const BATCH_OPERATION_LIMIT: usize = 64;
// Bound serialized batch input and its durable request fingerprint to 512 KiB.
pub(crate) const BATCH_INPUT_BYTES: usize = 512 * 1024;
// Cursor identities have the same bounded width across approval and stall reads.
pub(crate) const CURSOR_ID_BYTES: usize = 128;
// SQL-backed public pages accept at least one row and at most 200 rows.
pub(crate) const MIN_PAGE_ROWS: u64 = 1;
pub(crate) const MAX_PAGE_ROWS: u64 = 200;
// Four bytes always fit one UTF-8 scalar; byte pages share the control-text cap.
pub(crate) const MIN_OUTPUT_BYTES: u64 = 4;
pub(crate) const MAX_OUTPUT_BYTES: u64 = crate::limits::CONTROL_INPUT_BYTES as u64;
// History and approval reads default to half a maximum page.
pub(crate) const DEFAULT_HISTORY_PAGE_ROWS: u64 = MAX_PAGE_ROWS / 2;
// GC considers at most this many agent candidates per acknowledged page.
pub(crate) const GC_CANDIDATES: usize = 128;
// Admission's coordinator and startup SQL yields exactly four classes, numbered 0..3.
pub(super) const READY_CLASS_COUNT: usize = 4;
pub(super) const MAX_READY_CLASS: usize = READY_CLASS_COUNT - 1;

const _: () = assert!(MIN_PAGE_ROWS <= DEFAULT_HISTORY_PAGE_ROWS);
const _: () = assert!(DEFAULT_HISTORY_PAGE_ROWS <= MAX_PAGE_ROWS);
const _: () = assert!(BATCH_OPERATION_LIMIT < BATCH_INPUT_BYTES);

// Maximum accepted delayed-message interval: seven days in seconds.
pub(crate) const MAX_DELAY: u64 = 604_800;
