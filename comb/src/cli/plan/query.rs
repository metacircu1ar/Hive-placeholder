use super::{AfterReply, Plan};
use crate::cli::args;
use comb::{Result, search::Search, service::Operation, store::List, values::Timestamp};

impl args::List {
    pub(super) fn operation(self) -> Operation {
        Operation::List {
            request: List {
                kind: self.filters.kind,
                tag: self.filters.tag,
                prefix: self.filters.prefix,
                unverified: self.unverified,
                after: self.after,
                limit: self.limit,
            },
        }
    }
}

impl args::Search {
    pub(super) fn plan(self) -> crate::cli::input::Result<Plan> {
        Ok(Plan {
            operation: Operation::Search {
                request: Search {
                    query: self.query,
                    limit: self.limit,
                    kind: self.filters.kind,
                    tag: self.filters.tag,
                    prefix: self.filters.prefix,
                    after: self.after.map(Timestamp::search_filter).transpose()?,
                    before: self.before.map(Timestamp::search_filter).transpose()?,
                },
                mode: self.mode,
            },
            embedding_config: self
                .embedding_config
                .as_deref()
                .map(|path| {
                    crate::cli::input::Context::new("search", "--embedding-config").embedding(path)
                })
                .transpose()?,
            after: AfterReply::Print,
        })
    }
}

impl args::LinkArgs {
    pub(super) fn operation(self, incoming: bool) -> Result<Operation> {
        Ok(Operation::Links {
            slug: self.slug.try_into()?,
            incoming,
            after: self.after,
            limit: self.limit,
        })
    }
}
