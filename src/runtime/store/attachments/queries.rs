//! SQL owned by this store operation; statement text is a profiler key.

pub(super) const LATEST_ATTACHMENTS_SELECT: &str = "SELECT id FROM messages WHERE recipient=?1 \
    AND has_attachments=1 ORDER BY seq DESC LIMIT 1";

pub(super) const UNINDEXED_ATTACHMENTS_SELECT: &str =
    "SELECT EXISTS(SELECT 1 FROM attachment_blobs WHERE key=?1)";

pub(super) const ATTACHMENTS_SELECT: &str = "SELECT a.data,a.snapshot_key,b.filename,b.bytes \
    FROM message_attachments a LEFT JOIN attachment_blobs b ON b.key=a.snapshot_key WHERE \
    a.message_id = ?1 ORDER BY a.position";

pub(super) const PENDING_ATTACHMENTS_SELECT: &str = "SELECT b.key, b.filename FROM \
    pending_attachment_deletions p JOIN attachment_blobs b ON b.key=p.key WHERE NOT EXISTS \
    (SELECT 1 FROM message_attachments a WHERE a.snapshot_key=p.key) LIMIT 128";

pub(super) const ATTACHMENT_BYTES_SELECT: &str =
    "SELECT coalesce(sum(bytes),0) FROM attachment_blobs";

pub(super) const ATTACH_INSERT: &str =
    "INSERT OR IGNORE INTO attachment_blobs(key,filename,bytes) VALUES (?1,?2,?3)";

pub(super) const ATTACH_SELECT: &str =
    "SELECT filename=?2 AND bytes=?3 FROM attachment_blobs WHERE key=?1";

pub(super) const ATTACH_INSERT_2: &str = "INSERT INTO \
    message_attachments(message_id,position,snapshot_key,data) VALUES (?1,?2,?3,?4)";

pub(super) const ACKNOWLEDGE_ATTACHMENTS_SELECT: &str =
    "SELECT EXISTS(SELECT 1 FROM message_attachments WHERE snapshot_key=?1)";

pub(super) const ACKNOWLEDGE_ATTACHMENTS_DELETE: &str =
    "DELETE FROM pending_attachment_deletions WHERE key=?1";

pub(super) const ACKNOWLEDGE_ATTACHMENTS_DELETE_2: &str =
    "DELETE FROM attachment_blobs WHERE key=?1";

pub(super) const RECOVER_ATTACHMENTS_SELECT: &str =
    "SELECT EXISTS(SELECT 1 FROM attachment_blobs WHERE key=?1)";
