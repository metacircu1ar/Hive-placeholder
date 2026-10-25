//! SQL owned by this store operation; statement text is a profiler key.

pub(super) const IMAGE_USAGE_SELECT: &str =
    "SELECT bytes,images FROM agent_image_usage WHERE agent=?1";

pub(super) const SAVE_CLAIMED_ATTACHMENT_DELIVERY_SELECT: &str = "SELECT EXISTS(SELECT 1 FROM \
    messages WHERE id=?1 AND recipient=?2 AND state='submitting')";

pub(super) const PLAN_MESSAGE_IMAGES_INSERT: &str = "INSERT INTO \
    agent_image_usage(agent,bytes,images) VALUES(?1,?2,?3) ON CONFLICT(agent) DO UPDATE SET \
    bytes=excluded.bytes,images=excluded.images";

pub(super) const SAVE_ATTACHMENT_DELIVERY_UPDATE: &str = "UPDATE message_attachments SET \
    data=?3 WHERE message_id=?1 AND position=?2 AND snapshot_key IS ?4";

pub(super) const INHERIT_IMAGE_USAGE_INSERT: &str = "INSERT INTO \
    agent_image_usage(agent,bytes,images) SELECT ?1,bytes,images FROM agent_image_usage WHERE \
    agent=?2";
