use super::super::{Context, args, input};
use comb::{
    Error, Result,
    service::Operation,
    values::{ContentHash, RequestId, RunName},
};

impl args::RequestIdArg {
    fn resolve(self) -> Result<RequestId> {
        Ok(self
            .request_id
            .map(|value| value.parse())
            .transpose()?
            .unwrap_or_else(RequestId::generate))
    }
}

impl Context {
    fn run_name(&self) -> Result<Option<RunName>> {
        self.run.clone().map(TryInto::try_into).transpose()
    }
}

impl args::Put {
    pub(super) fn operation(self, context: &Context) -> input::Result<Operation> {
        Ok(Operation::Put {
            markdown: input::Context::new(
                "put",
                if self.file.is_some() {
                    "--file"
                } else {
                    "stdin"
                },
            )
            .optional_file(self.file.as_deref())
            .markdown()?,
            slug: self.slug.try_into()?,
            expected_hash: self
                .expected_hash
                .map(ContentHash::caller_input)
                .transpose()?,
            request_id: self.request.resolve()?,
            run: context.run_name()?,
        })
    }
}
impl args::Append {
    pub(super) fn operation(self, context: &Context) -> input::Result<Operation> {
        Ok(Operation::Append {
            summary: self
                .summary_file
                .as_deref()
                .map(|p| {
                    input::Context::new("append", "--summary-file")
                        .file(p)
                        .summary(false)
                })
                .transpose()?,
            slug: self.slug.try_into()?,
            text: self.text,
            source: self.source,
            expected_hash: self
                .expected_hash
                .map(ContentHash::caller_input)
                .transpose()?,
            request_id: self.request.resolve()?,
            run: context.run_name()?,
        })
    }
}
impl args::Capture {
    pub(super) fn operation(self, context: &Context) -> input::Result<Operation> {
        Ok(Operation::Capture {
            text: self.text,
            request_id: self.request.resolve()?,
            run: context.run_name()?,
        })
    }
}
impl args::Promote {
    pub(super) fn operation(self, context: &Context) -> input::Result<Operation> {
        Ok(Operation::Promote {
            slug: self.slug.try_into()?,
            expected_hash: ContentHash::caller_input(self.expected_hash)?,
            request_id: self.request.resolve()?,
            run: context.run_name()?,
        })
    }
}
impl args::Rm {
    pub(super) fn operation(self) -> Result<Operation> {
        Ok(Operation::Remove {
            slug: self.slug.try_into()?,
            expected_hash: ContentHash::caller_input(self.expected_hash)?,
            request_id: self.request.resolve()?,
        })
    }
}
impl args::Consolidate {
    pub(super) fn operation(self, context: &Context) -> input::Result<Operation> {
        Ok(Operation::Consolidate {
            request: comb::store::Consolidate {
                summary: input::Context::new("consolidate", "--summary-file")
                    .file(&self.summary_file)
                    .summary(true)?,
                note: self.note.try_into()?,
                target: self.target.try_into()?,
                note_hash: ContentHash::caller_input(self.note_hash)?,
                target_hash: ContentHash::caller_input(self.target_hash)?,
                recopy: self.recopy,
            },
            request_id: self.request.resolve()?,
            run: context
                .run
                .clone()
                .ok_or_else(Error::consolidation_run_required)?
                .try_into()?,
        })
    }
}
impl args::Decide {
    pub(super) fn operation(self, context: &Context) -> input::Result<Operation> {
        Ok(Operation::Decide {
            slug: self.slug.try_into()?,
            decision: self.decision,
            reason: self.reason,
            expected_hash: ContentHash::caller_input(self.expected_hash)?,
            request_id: self.request.resolve()?,
            run: context.run_name()?,
        })
    }
}
