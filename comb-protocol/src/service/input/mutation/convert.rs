use super::Write;
use crate::{
    Error, Result,
    request::Consolidate,
    service::{
        Operation,
        input::{RawConsolidate, RawOperation},
    },
    values::{ContentHash, RequestId, RunName, Slug},
};
use hive_input::Code;

pub(crate) fn operation(raw: RawOperation, tool: bool) -> Result<Operation> {
    use RawOperation as Raw;
    match raw {
        Raw::Put {
            slug,
            markdown,
            expected_hash,
            request_id,
            run,
        } => {
            let c = Context(Write::Put, tool);
            Ok(Operation::Put {
                slug: c.slug("slug", slug)?,
                markdown,
                expected_hash: c.optional_hash(expected_hash)?,
                request_id: c.id(request_id)?,
                run: c.run(run)?,
            })
        }
        Raw::Append {
            slug,
            text,
            source,
            summary,
            expected_hash,
            request_id,
            run,
        } => {
            let c = Context(Write::Append, tool);
            Ok(Operation::Append {
                slug: c.slug("slug", slug)?,
                text,
                source,
                summary,
                expected_hash: c.optional_hash(expected_hash)?,
                request_id: c.id(request_id)?,
                run: c.run(run)?,
            })
        }
        Raw::Capture {
            text,
            request_id,
            run,
        } => {
            let c = Context(Write::Capture, tool);
            Ok(Operation::Capture {
                text,
                request_id: c.id(request_id)?,
                run: c.run(run)?,
            })
        }
        Raw::Consolidate {
            request,
            request_id,
            run,
        } => {
            let c = Context(Write::Consolidate, tool);
            Ok(Operation::Consolidate {
                request: c.consolidate(request)?,
                request_id: c.id(request_id)?,
                run: c.value("run", run.try_into())?,
            })
        }
        other => review_or_remove(other, tool),
    }
}
fn review_or_remove(raw: RawOperation, tool: bool) -> Result<Operation> {
    use RawOperation as Raw;
    match raw {
        Raw::Promote {
            slug,
            expected_hash,
            request_id,
            run,
        } => {
            let c = Context(Write::Promote, tool);
            Ok(Operation::Promote {
                slug: c.slug("slug", slug)?,
                expected_hash: c.hash("expected_hash", expected_hash)?,
                request_id: c.id(request_id)?,
                run: c.run(run)?,
            })
        }
        Raw::Decide {
            slug,
            expected_hash,
            decision,
            reason,
            request_id,
            run,
        } => {
            let c = Context(Write::Decide, tool);
            Ok(Operation::Decide {
                slug: c.slug("slug", slug)?,
                expected_hash: c.hash("expected_hash", expected_hash)?,
                decision,
                reason,
                request_id: c.id(request_id)?,
                run: c.run(run)?,
            })
        }
        Raw::Remove {
            slug,
            expected_hash,
            request_id,
        } => {
            let c = Context(Write::Remove, tool);
            Ok(Operation::Remove {
                slug: c.slug("slug", slug)?,
                expected_hash: c.hash("expected_hash", expected_hash)?,
                request_id: c.id(request_id)?,
            })
        }
        _ => Err(Error::internal_serialization()),
    }
}
struct Context(Write, bool);
impl Context {
    fn value<T>(&self, field: &str, result: Result<T>) -> Result<T> {
        result.map_err(|cause| {
            self.0
                .error(Code::InvalidValue, field, self.1)
                .caused_by(cause)
        })
    }
    fn slug(&self, field: &str, text: String) -> Result<Slug> {
        self.value(field, text.try_into())
    }
    fn hash(&self, field: &str, text: String) -> Result<ContentHash> {
        self.value(field, ContentHash::caller_input(text))
    }
    fn optional_hash(&self, text: Option<String>) -> Result<Option<ContentHash>> {
        text.map(|text| self.hash("expected_hash", text))
            .transpose()
    }
    fn id(&self, text: String) -> Result<RequestId> {
        self.value("request_id", text.try_into())
    }
    fn run(&self, text: Option<String>) -> Result<Option<RunName>> {
        text.map(|text| self.value("run", text.try_into()))
            .transpose()
    }
    fn consolidate(&self, raw: RawConsolidate) -> Result<Consolidate> {
        Ok(Consolidate {
            note: self.slug("note", raw.note)?,
            target: self.slug("target", raw.target)?,
            note_hash: self.hash("note_hash", raw.note_hash)?,
            target_hash: self.hash("target_hash", raw.target_hash)?,
            summary: raw.summary,
            recopy: raw.recopy,
        })
    }
}
