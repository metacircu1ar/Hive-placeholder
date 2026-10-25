//! Conservative native-image eligibility and retained-history accounting.
use super::{Attachment, Delivery, ReferenceReason as Reason, Source};

pub(in crate::runtime) const IMAGE_BYTES: u64 = 4 * 1024 * 1024;
pub(in crate::runtime) const TURN_BYTES: u64 = 8 * 1024 * 1024;
pub(in crate::runtime) const HISTORY_BYTES: u64 = 8 * 1024 * 1024;
pub(in crate::runtime) const HISTORY_IMAGES: u32 = 20;
pub(in crate::runtime) const MAX_DIMENSION: u32 = 2000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::runtime) struct Usage {
    pub bytes: u64,
    pub images: u32,
}
impl Attachment {
    pub(in crate::runtime) fn native_bytes(&self) -> std::result::Result<u64, Reason> {
        if !matches!(self.source, Source::Snapshot { .. }) {
            return Err(Reason::LivePath);
        }
        if !matches!(
            self.media_type.as_str(),
            "image/png" | "image/jpeg" | "image/gif" | "image/webp"
        ) {
            return Err(Reason::NotImage);
        }
        if !self
            .dimensions
            .is_some_and(|d| d.iter().all(|n| *n > 0 && *n <= MAX_DIMENSION))
        {
            return Err(Reason::ImageDimensions);
        }
        let encoded = self.bytes.div_ceil(3) * 4;
        if encoded > IMAGE_BYTES {
            return Err(Reason::ImageBytes);
        }
        Ok(encoded)
    }
}

/// Called once for a pending message, inside the same transaction as its charge.
pub(in crate::runtime) fn select(items: &mut [Attachment], retained: Usage) -> Usage {
    let mut added = Usage::default();
    for item in items {
        let candidate = item.native_bytes().and_then(|bytes| {
            if added.bytes + bytes > TURN_BYTES {
                Err(Reason::MessageImageBudget)
            } else if retained.bytes + added.bytes + bytes > HISTORY_BYTES
                || retained.images + added.images >= HISTORY_IMAGES
            {
                Err(Reason::ConversationImageBudget)
            } else {
                Ok(bytes)
            }
        });
        item.delivery = match candidate {
            Ok(bytes) => {
                added.bytes += bytes;
                added.images += 1;
                Delivery::NativeImage
            }
            Err(reason) => Delivery::Reference { reason },
        };
    }
    added
}
