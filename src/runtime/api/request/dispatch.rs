//! Command vocabulary and caller routing policy.
use super::{
    AgentsArgs, ApprovalArgs, ApprovalsArgs, AskArgs, AttachArgs, AuthArgs, BatchArgs,
    CompleteArgs, Deserialize, GcArgs, Input, KeySourceArgs, LiveArgs, MessageResultArgs,
    MessagesArgs, Op, OptionsCatalogArgs, OutputArgs, PurgeArgs, ReadArgs, RespondArgs, Result,
    SendArgs, Serialize, SetAuthArgs, SetNameArgs, SetOptionsArgs, SpawnArgs, StatusArgs,
    TargetArgs, TreeArgs, UsageArgs,
};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    Ping,
    #[cfg(feature = "comb")]
    Evidence(crate::runtime::collectors::Evidence),
    #[cfg(feature = "comb")]
    Collectors,
    #[cfg(feature = "comb")]
    CollectorAdmin(crate::runtime::collectors::Admin),
    Agents(AgentsArgs),
    Status(StatusArgs),
    Read(ReadArgs),
    Output(OutputArgs),
    Messages(MessagesArgs),
    MessageResult(MessageResultArgs),
    Approvals(ApprovalsArgs),
    Approval(ApprovalArgs),
    Ask(AskArgs),
    Respond(RespondArgs),
    Spawn(Box<SpawnArgs>),
    Batch(BatchArgs),
    Send(SendArgs),
    SetOptions(SetOptionsArgs),
    OptionsCatalog(OptionsCatalogArgs),
    SetAuth(SetAuthArgs),
    /// Change or clear an agent's display name at once.
    SetName(SetNameArgs),
    LoginRestored(AuthArgs),
    AuthGroups,
    Roles,
    AuthAvailability(AuthArgs),
    /// The quota usage each login last reported, or one login's.
    Usage(UsageArgs),
    KeySource(KeySourceArgs),
    Complete(CompleteArgs),
    Terminate(TargetArgs),
    Purge(PurgeArgs),
    Gc(GcArgs),
    Stop(TargetArgs),
    Resume(TargetArgs),
    Interrupt(TargetArgs),
    Live(LiveArgs),
    Tree(TreeArgs),
    Attach(AttachArgs),
    Shutdown,
    #[serde(skip)]
    Invalid,
    #[serde(other)]
    Unsupported,
}

impl Request {
    pub(in crate::runtime) fn caller_policy(
        &self,
        caller: &crate::runtime::hub::Caller,
    ) -> Result<()> {
        use crate::error::Error;
        let denied = match self {
            Self::LoginRestored(_) if caller.is_human() || caller.is_system() => None,
            _ if caller.is_system() => {
                Some("Operation is not implemented in this runtime increment.")
            }
            Self::Approvals(_) | Self::Approval(_) | Self::Respond(_) if !caller.is_human() => {
                Some("Approval list, show and respond are for humans only.")
            }
            Self::MessageResult(_) if !caller.is_human() => {
                Some("Message results are for external callers.")
            }
            Self::KeySource(_) if !caller.is_human() => {
                Some("API key source status is for humans only.")
            }
            Self::Purge(_) if !caller.is_human() => {
                Some("Agent purge is for external callers only.")
            }
            #[cfg(feature = "comb")]
            Self::Collectors | Self::CollectorAdmin(_) if !caller.is_human() => {
                Some("Collector administration is human-only.")
            }
            Self::Gc(_) if !caller.is_human() => Some("Agent GC is for external callers only."),
            Self::Live(_) | Self::Tree(_) | Self::Attach(_) if !caller.is_human() => {
                Some("Live viewing is for human viewers only.")
            }
            Self::Roles if !caller.is_human() => Some("Spawn choices are for human viewers only."),
            Self::Usage(_) if !caller.is_human() => {
                Some("Usage reports are for human viewers only.")
            }
            Self::Shutdown | Self::LoginRestored(_) if !caller.is_human() => {
                Some("Operation is not implemented in this runtime increment.")
            }
            _ => None,
        };
        if let Some(message) = denied {
            return Err(Error::unsupported_operation(message));
        }
        Ok(())
    }

    pub(crate) fn is_preview(&self) -> bool {
        match self {
            Self::Purge(args) => args.dry_run.copied() == Some(true),
            Self::Gc(args) => args.dry_run.copied() == Some(true),
            _ => false,
        }
    }

    pub(crate) fn is_read_only(&self) -> bool {
        #[cfg(feature = "comb")]
        if matches!(self, Self::Collectors) {
            return true;
        }
        matches!(
            self,
            Self::Ping
                | Self::OptionsCatalog(_)
                | Self::Agents(_)
                | Self::Status(_)
                | Self::Read(_)
                | Self::Messages(_)
                | Self::Output(_)
                | Self::MessageResult(_)
                | Self::Approvals(_)
                | Self::Approval(_)
                | Self::AuthGroups
                | Self::Roles
                | Self::AuthAvailability(_)
                | Self::Tree(_)
                | Self::Attach(_)
        )
    }
}

impl Request {
    pub(crate) fn attachment_context(&self) -> Op {
        match self {
            Self::Spawn(_) => Op::Spawn,
            Self::Send(_) => Op::Send,
            _ => unreachable!("only spawn/send expose attachment paths"),
        }
    }
    pub(crate) fn attachment_paths(&self) -> Option<&Input<Vec<std::path::PathBuf>>> {
        match self {
            Self::Spawn(args) => Some(&args.attachments),
            Self::Send(args) => Some(&args.attachments),
            _ => None,
        }
    }
}
