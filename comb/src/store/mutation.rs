use super::{
    Comb, ContentHash, Document, Error, Fingerprint, Mutation, Page, Receipt, ReceiptOperation,
    Result, Serialize, Slug, Write,
};
use crate::error::CommitExt;
use crate::read::{FileFailure, Publishing};

/// A fingerprinted request can pause between replay, loading and publication.
/// Consolidation uses the same stages while coordinating its two durable writes.
pub(super) struct Ticket<'a, 'actor> {
    write: &'a Write<'actor>,
    operation: ReceiptOperation,
    fingerprint: Fingerprint,
}

impl<'a, 'actor> Ticket<'a, 'actor> {
    pub(super) fn new(
        write: &'a Write<'actor>,
        operation: ReceiptOperation,
        payload: impl Serialize,
    ) -> Result<Self> {
        Ok(Self {
            write,
            operation,
            fingerprint: Fingerprint::digest(crate::error::json_bytes(&(
                &write.principal,
                payload,
            ))?),
        })
    }

    pub(super) fn write(&self) -> &'a Write<'actor> {
        self.write
    }

    pub(super) fn replay(&self, comb: &Comb) -> Result<Option<Mutation>> {
        comb.ready()?;
        let Some(receipt) = comb.matching_receipt(self)? else {
            return Ok(None);
        };
        let slug = receipt.slug;
        let document = comb.get(&slug).map_err(|error| {
            if error.is_not_found() {
                Error::result_removed().caused_by(error)
            } else {
                error
            }
        })?;
        Ok(Some(Mutation {
            slug,
            hash: document.hash,
            request_id: self.write.request_id.clone(),
            replayed: true,
        }))
    }

    pub(super) fn into_receipt(self, slug: &Slug) -> Receipt {
        Receipt {
            principal: self.write.principal.clone(),
            request_id: self.write.request_id.clone(),
            fingerprint: self.fingerprint.into(),
            operation: self.operation.as_str().to_owned(),
            slug: slug.into(),
        }
    }

    pub(super) fn save(
        self,
        comb: &mut Comb,
        slug: &Slug,
        mut page: Page,
        expected: Option<&ContentHash>,
    ) -> Result<Mutation> {
        let write = self.write;
        page.touch();
        let receipt = self.into_receipt(slug);
        comb.disk.seal(slug, &mut page)?;
        let raw = comb.persist(slug, &page, expected, Some(receipt))?;
        Ok(Mutation {
            slug: slug.into(),
            hash: ContentHash::digest(raw),
            request_id: write.request_id.clone(),
            replayed: false,
        })
    }
}

pub(super) struct Change<'a, Payload, Target> {
    pub slug: &'a Slug,
    pub operation: ReceiptOperation,
    pub payload: Payload,
    pub target: Target,
}

/// Required and optional targets keep transformations free of unchecked unwraps.
pub(super) trait Target {
    type Loaded;
    fn load(self, comb: &Comb, slug: &Slug) -> Result<Self::Loaded>;
    fn hash(loaded: &Self::Loaded) -> Option<&ContentHash>;
}

pub(super) enum Cas<'a> {
    Any,
    Matches(Option<&'a ContentHash>),
}

pub(super) struct Required<'a>(pub Cas<'a>);
pub(super) struct Optional<'a>(pub Option<&'a ContentHash>);

impl Target for Required<'_> {
    type Loaded = Document;
    fn hash(loaded: &Document) -> Option<&ContentHash> {
        Some(&loaded.hash)
    }
    fn load(self, comb: &Comb, slug: &Slug) -> Result<Document> {
        let document = comb.get(slug)?;
        if let Cas::Matches(expected) = self.0 {
            check_expected(Some(&document.hash), expected)?;
        }
        Ok(document)
    }
}

impl Target for Optional<'_> {
    type Loaded = Option<Document>;
    fn hash(loaded: &Option<Document>) -> Option<&ContentHash> {
        loaded.as_ref().map(|document| &document.hash)
    }
    fn load(self, comb: &Comb, slug: &Slug) -> Result<Option<Document>> {
        let existing = match comb.get(slug) {
            Ok(document) => Some(document),
            Err(error) if error.is_not_found() => None,
            Err(error) => return Err(error),
        };
        check_expected(existing.as_ref().map(|document| &document.hash), self.0)?;
        Ok(existing)
    }
}

impl Comb {
    pub(super) fn persist(
        &mut self,
        slug: &Slug,
        page: &Page,
        expected: Option<&ContentHash>,
        receipt: Option<Receipt>,
    ) -> Result<String> {
        self.ready()?;
        let publication = self.publication.clone();
        let mut publishing =
            Publishing::file(&mut self.dirty, &publication, FileFailure::RestoreClean);
        publishing.disk_result(self.disk.write(slug, Some(page), expected, receipt))?;
        #[cfg(test)]
        if let Some(hook) = self.before_index.take() {
            hook();
        }
        publishing.publish()?;
        let raw = page.render().committed()?;
        self.index.upsert(slug, &raw, page).committed()?;
        self.disk.finish()?;
        publishing.commit();
        Ok(raw)
    }

    // Admission stays with each caller: page replay checks ready here, while
    // removal must check ready and human authority before even hashing a request.
    pub(super) fn matching_receipt(&self, ticket: &Ticket<'_, '_>) -> Result<Option<Receipt>> {
        let receipt = self
            .disk
            .receipt(&ticket.write.principal, &ticket.write.request_id)?;
        if let Some(receipt) = &receipt
            && receipt.fingerprint != ticket.fingerprint.as_str()
        {
            return Err(Error::request_conflict());
        }
        Ok(receipt)
    }

    pub(super) fn mutate<Payload: Serialize, T: Target, Prepared>(
        &mut self,
        write: &Write<'_>,
        change: Change<'_, Payload, T>,
        prepare: impl FnOnce() -> Result<Prepared>,
        transform: impl FnOnce(Prepared, T::Loaded) -> Result<Page>,
    ) -> Result<Mutation> {
        let ticket = Ticket::new(write, change.operation, change.payload)?;
        if let Some(reply) = ticket.replay(self)? {
            return Ok(reply);
        }
        let prepared = prepare()?;
        let loaded = change.target.load(self, change.slug)?;
        let expected = T::hash(&loaded).cloned();
        let page = transform(prepared, loaded)?;
        ticket.save(self, change.slug, page, expected.as_ref())
    }
}

pub(super) fn check_expected(
    actual: Option<&ContentHash>,
    expected: Option<&ContentHash>,
) -> Result<()> {
    if actual == expected {
        Ok(())
    } else {
        Err(Error::conflict())
    }
}
