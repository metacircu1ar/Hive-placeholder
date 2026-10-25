//! Validate list selectors without losing retained lineage or retired login bindings.
use super::{AGENT_PAGE_ROWS, Op, Queries, Result, Value, api};
use api::pagination::{number, rows};
use hive_input::{Code, validate};

pub(super) fn agents(
    queries: &Queries,
    args: &api::AgentsArgs,
    config: &crate::config::Config,
) -> Result<Value> {
    let page_error = |error: api::pagination::InvalidPage| {
        Op::Agents.error(error.code, error.field, error.expected)
    };
    let after = number(&args.after, 0, "/after").map_err(page_error)?;
    let limit = rows(number(&args.limit, AGENT_PAGE_ROWS, "/limit").map_err(page_error)?)
        .map_err(page_error)?;
    // Decode both optional types before adding semantic checks; retain error order.
    let parent = args
        .parent
        .optional(Op::Agents, "/parent", "a parent agent ID string")?;
    let auth = args
        .auth
        .optional(Op::Agents, "/auth", "a login profile or group string")?;
    if let Some(parent) = parent {
        validate::text_bytes(parent, crate::limits::CONTROL_INPUT_BYTES)
            .map_err(|rule| Op::Agents.error(rule.code(), "/parent", rule.expected()))?;
    }
    if let Some(auth) = auth {
        validate::config_name(auth)
            .map_err(|rule| Op::Agents.error(rule.code(), "/auth", rule.expected()))?;
    }
    if let Some(parent) = parent
        && !queries.parent_filter_exists(parent)?
    {
        return Err(Op::Agents.error(
            Code::InvalidValue,
            "/parent",
            "an existing agent ID or a retained purged-parent ID",
        ));
    }
    if let Some(auth) = auth
        && !config.auth_profiles.contains_key(auth)
        && !config.auth_groups.contains_key(auth)
        && !queries.auth_filter_exists(auth)?
    {
        return Err(Op::Agents.error(
            Code::InvalidValue,
            "/auth",
            "a configured login profile/group or a binding retained by an agent",
        ));
    }
    queries.agent_page(
        after,
        limit,
        parent.map(String::as_str),
        auth.map(String::as_str),
    )
}
