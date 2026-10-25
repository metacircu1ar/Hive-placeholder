use super::{
    Operation,
    actions::{AdminAction, Dispatch, MutationAction, ReadAction, WriteMeta},
};

impl Operation {
    #[expect(
        clippy::too_many_lines,
        reason = "exhaustive operation projection keeps variant and field mappings together"
    )]
    pub fn dispatch(&self) -> Dispatch<'_> {
        use Dispatch::{Admin, Mutation, Read};
        match self {
            Self::Capabilities => Read(ReadAction::Capabilities),
            Self::Health => Read(ReadAction::Health),
            Self::Get { slug } => Read(ReadAction::Get(slug)),
            Self::Search { request, mode } => Read(ReadAction::Search(request, *mode)),
            Self::List { request } => Read(ReadAction::List(request)),
            Self::Links {
                slug,
                incoming,
                after,
                limit,
            } => Read(ReadAction::Links(slug, *incoming, after.as_deref(), *limit)),
            Self::Graph { slug, depth } => Read(ReadAction::Graph(slug, *depth)),
            Self::Stats => Read(ReadAction::Stats),
            Self::Doctor => Admin(AdminAction::Doctor),
            Self::Put {
                slug,
                markdown,
                expected_hash: hash,
                request_id: id,
                run,
            } => Mutation(MutationAction::Put(
                slug,
                markdown,
                hash.as_ref(),
                WriteMeta::new(id, run.as_ref()),
            )),
            Self::Append {
                slug,
                text,
                source,
                summary,
                expected_hash: hash,
                request_id: id,
                run,
            } => Mutation(MutationAction::Append(
                slug,
                text,
                source,
                summary.as_deref(),
                hash.as_ref(),
                WriteMeta::new(id, run.as_ref()),
            )),
            Self::Capture {
                text,
                request_id: id,
                run,
            } => Mutation(MutationAction::Capture(
                text,
                WriteMeta::new(id, run.as_ref()),
            )),
            Self::Promote {
                slug,
                expected_hash: hash,
                request_id: id,
                run,
            } => Mutation(MutationAction::Promote(
                slug,
                hash,
                WriteMeta::new(id, run.as_ref()),
            )),
            Self::Consolidate {
                request,
                request_id: id,
                run,
            } => Mutation(MutationAction::Consolidate(
                request,
                WriteMeta::new(id, Some(run)),
            )),
            Self::Decide {
                slug,
                expected_hash: hash,
                decision,
                reason,
                request_id: id,
                run,
            } => Mutation(MutationAction::Decide(
                slug,
                hash,
                *decision,
                reason,
                WriteMeta::new(id, run.as_ref()),
            )),
            Self::Remove {
                slug,
                expected_hash: hash,
                request_id: id,
            } => Mutation(MutationAction::Remove(slug, hash, WriteMeta::new(id, None))),
            Self::Sync => Admin(AdminAction::Sync),
            Self::Import { directory, prefix } => {
                Admin(AdminAction::Import(directory, prefix.as_ref()))
            }
            Self::ImportStatus => Admin(AdminAction::ImportStatus),
            Self::ImportAbandon { batch_id } => Admin(AdminAction::ImportAbandon(batch_id)),
            Self::CredentialCreate { grant } => Admin(AdminAction::CredentialCreate(grant)),
            Self::CredentialList => Admin(AdminAction::CredentialList),
            Self::CredentialRevoke { id } => Admin(AdminAction::CredentialRevoke(id)),
        }
    }
}
