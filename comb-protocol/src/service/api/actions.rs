use super::Operation;
use crate::{
    Error,
    page::Decision,
    request::{Consolidate, List},
    search::{Mode, Search},
    service::auth::Grant,
    values::{ContentHash, CredentialId, ImportId, RequestId, RunName, Slug},
};
use std::path::Path;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Class {
    Read,
    Write,
    Admin,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Capability {
    Read,
    Write,
    Capture,
    Review,
    Human,
}

pub enum Dispatch<'a> {
    Read(ReadAction<'a>),
    Mutation(MutationAction<'a>),
    Admin(AdminAction<'a>),
}

#[derive(Clone, Copy)]
pub enum ReadAction<'a> {
    Capabilities,
    Health,
    Get(&'a Slug),
    Search(&'a Search, Mode),
    List(&'a List),
    Links(&'a Slug, bool, Option<&'a str>, usize),
    Graph(&'a Slug, usize),
    Stats,
}

#[derive(Clone, Copy)]
pub struct WriteMeta<'a> {
    pub request_id: &'a RequestId,
    pub run: Option<&'a str>,
}
impl<'a> WriteMeta<'a> {
    pub fn new(request_id: &'a RequestId, run: Option<&'a RunName>) -> Self {
        Self {
            request_id,
            run: run.map(RunName::as_str),
        }
    }
}

pub enum MutationAction<'a> {
    Put(&'a Slug, &'a str, Option<&'a ContentHash>, WriteMeta<'a>),
    Append(
        &'a Slug,
        &'a str,
        &'a str,
        Option<&'a str>,
        Option<&'a ContentHash>,
        WriteMeta<'a>,
    ),
    Capture(&'a str, WriteMeta<'a>),
    Promote(&'a Slug, &'a ContentHash, WriteMeta<'a>),
    Consolidate(&'a Consolidate, WriteMeta<'a>),
    Decide(&'a Slug, &'a ContentHash, Decision, &'a str, WriteMeta<'a>),
    Remove(&'a Slug, &'a ContentHash, WriteMeta<'a>),
}

pub enum AdminAction<'a> {
    Doctor,
    Sync,
    Import(&'a Path, Option<&'a Slug>),
    ImportStatus,
    ImportAbandon(&'a ImportId),
    CredentialCreate(&'a Grant),
    CredentialList,
    CredentialRevoke(&'a CredentialId),
}

impl Dispatch<'_> {
    pub fn class(&self) -> Class {
        match self {
            Self::Read(_) => Class::Read,
            Self::Mutation(_) => Class::Write,
            Self::Admin(_) => Class::Admin,
        }
    }
    pub fn required(&self) -> Capability {
        match self {
            Self::Read(_) => Capability::Read,
            Self::Mutation(action) => action.required(),
            Self::Admin(action) => action.required(),
        }
    }
    pub fn is_mutation(&self) -> bool {
        match self {
            Self::Read(_) => false,
            Self::Mutation(_) => true,
            Self::Admin(action) => action.is_mutation(),
        }
    }
}
impl ReadAction<'_> {
    pub fn query(&self) -> Option<&str> {
        match self {
            Self::Search(request, mode) if *mode != Mode::Text => Some(&request.query),
            Self::Capabilities
            | Self::Health
            | Self::Get(_)
            | Self::Search(..)
            | Self::List(_)
            | Self::Links(..)
            | Self::Graph(..)
            | Self::Stats => None,
        }
    }
}
impl<'a> MutationAction<'a> {
    pub fn required(&self) -> Capability {
        match self {
            Self::Put(..) | Self::Append(..) | Self::Consolidate(..) => Capability::Write,
            Self::Capture(..) => Capability::Capture,
            Self::Promote(..) | Self::Decide(..) => Capability::Review,
            Self::Remove(..) => Capability::Human,
        }
    }
    pub fn metadata(&self) -> WriteMeta<'a> {
        match self {
            Self::Put(_, _, _, write)
            | Self::Append(_, _, _, _, _, write)
            | Self::Capture(_, write)
            | Self::Promote(_, _, write)
            | Self::Consolidate(_, write)
            | Self::Decide(_, _, _, _, write)
            | Self::Remove(_, _, write) => *write,
        }
    }
}
impl AdminAction<'_> {
    pub fn required(&self) -> Capability {
        match self {
            Self::Doctor => Capability::Read,
            Self::Sync
            | Self::Import(..)
            | Self::ImportStatus
            | Self::ImportAbandon(_)
            | Self::CredentialCreate(_)
            | Self::CredentialList
            | Self::CredentialRevoke(_) => Capability::Human,
        }
    }
    pub fn is_mutation(&self) -> bool {
        match self {
            Self::Doctor | Self::CredentialList | Self::ImportStatus => false,
            Self::Sync
            | Self::Import(..)
            | Self::ImportAbandon(_)
            | Self::CredentialCreate(_)
            | Self::CredentialRevoke(_) => true,
        }
    }
}

impl Operation {
    pub fn class(&self) -> Class {
        self.dispatch().class()
    }
    pub fn required(&self) -> Capability {
        self.dispatch().required()
    }
    pub fn is_mutation(&self) -> bool {
        self.dispatch().is_mutation()
    }
    pub fn request_id(&self) -> Option<&str> {
        // Return the operation's lifetime, not the temporary action borrow.
        match self.dispatch() {
            Dispatch::Mutation(action) => Some(action.metadata().request_id.as_str()),
            Dispatch::Read(_) | Dispatch::Admin(_) => None,
        }
    }
    pub fn lost_reply(&self) -> Error {
        let mutation = self.is_mutation();
        let mut error = if !mutation {
            Error::reply_timeout()
        } else if self.request_id().is_some() {
            Error::uncertain_retry()
        } else {
            Error::uncertain_inspect()
        };
        error.file_committed = mutation;
        error.request_id = self.request_id().map(str::to_owned);
        error
    }
}
