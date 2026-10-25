//! Garbage collection selection, confirmation and paginated purge requests.
use super::{CommandOutcome, Error, api, runtime};
use serde::Serialize;
use std::{path::Path, time::Instant};

#[derive(Serialize)]
struct Report {
    before: u64,
    dry_run: bool,
    complete: bool,
    acknowledged_pages: u64,
    stats: api::GcStats,
    selected_preview: Vec<String>,
    blockers: Vec<api::PurgeBlocker>,
    elapsed_seconds: f64,
    max_page_seconds: f64,
    native_history: &'static str,
}

#[derive(Serialize)]
struct FailedPage {
    request_id: String,
    args: api::Request,
    may_have_committed: bool,
}
#[derive(Serialize)]
struct FailedProgress {
    #[serde(flatten)]
    error: serde_json::Value,
    gc_progress: Report,
    failed_page: FailedPage,
}

pub(super) fn run(root: &Path, age: u64, dry_run: bool) -> CommandOutcome {
    run_before(root, crate::state::now().saturating_sub(age), dry_run)
}

pub(super) fn run_before(root: &Path, before: u64, dry_run: bool) -> CommandOutcome {
    let start = Instant::now();
    let mut report = Report::new(before, dry_run);
    let mut after = None;
    loop {
        let id = uuid::Uuid::new_v4().to_string();
        let request = api::Request::Gc(api::GcArgs {
            before: before.into(),
            after: after.clone().map_or(api::Input::Missing, api::Input::from),
            dry_run: dry_run.into(),
        });
        let page_start = Instant::now();
        let result = receive_page(root, &id, &request, after.as_ref(), &report);
        report.max_page_seconds = report
            .max_page_seconds
            .max(page_start.elapsed().as_secs_f64());
        report.elapsed_seconds = start.elapsed().as_secs_f64();
        let page = match result {
            Ok(page) => page,
            Err(value) => {
                return CommandOutcome::failed(api::value(FailedProgress {
                    error: value,
                    gc_progress: report,
                    failed_page: FailedPage {
                        request_id: id,
                        args: request,
                        may_have_committed: !dry_run,
                    },
                }));
            }
        };
        after = report.accept(page);
        if after.is_none() {
            break;
        }
    }
    report.complete = true;
    CommandOutcome::ok(api::value(report))
}

fn receive_page(
    root: &Path,
    id: &str,
    request: &api::Request,
    after: Option<&api::GcCursor>,
    report: &Report,
) -> std::result::Result<api::GcPage, serde_json::Value> {
    loop {
        // A transport failure is ambiguous. The same ID/body protects accepted work.
        let value = runtime::client::Client::new(root)
            .request_retrying_transport(id, request)
            .map_err(|error| runtime::response(Err(error)))?;
        if !value.is_object() {
            return Err(runtime::response(Err(Error::daemon_protocol_error(
                "Invalid GC response envelope.",
            ))));
        }
        if api::DaemonReply::new(&value)
            .map_err(|error| runtime::response(Err(error)))?
            .has_error()
        {
            return Err(value);
        }
        let page: api::GcPage = serde_json::from_value(value).map_err(|cause| {
            runtime::response(Err(Error::daemon_protocol_error(
                "Invalid GC page response.",
            )
            .with_source(cause)))
        })?;
        if invalid_page(&page, after, report) {
            return Err(runtime::response(Err(Error::daemon_protocol_error(
                "Invalid GC page response.",
            ))));
        }
        if !page.cleanup_pending {
            return Ok(page);
        }
        // Only cleanup changes on this replay; aggregate the accepted page once.
    }
}

impl Report {
    fn new(before: u64, dry_run: bool) -> Self {
        Self {
            before,
            dry_run,
            complete: false,
            acknowledged_pages: 0,
            stats: api::GcStats::default(),
            selected_preview: vec![],
            blockers: vec![],
            elapsed_seconds: 0.0,
            max_page_seconds: 0.0,
            native_history: "retained",
        }
    }

    fn accept(&mut self, page: api::GcPage) -> Option<api::GcCursor> {
        self.stats.add(&page.stats);
        self.acknowledged_pages += 1;
        self.selected_preview.extend(
            page.selected_preview
                .into_iter()
                .take(api::GC_PREVIEW.saturating_sub(self.selected_preview.len())),
        );
        self.blockers.extend(
            page.blockers
                .into_iter()
                .take(api::GC_PREVIEW.saturating_sub(self.blockers.len())),
        );
        page.next_after
    }
}

fn invalid_page(page: &api::GcPage, after: Option<&api::GcCursor>, report: &Report) -> bool {
    page.before != report.before
        || page.dry_run != report.dry_run
        || page.next_after.as_ref().is_some_and(|next| {
            next.ended_at >= report.before
                || next.id.is_empty()
                || after.is_some_and(|prev| next <= prev)
        })
        || page.stats.examined_agents
            != page
                .stats
                .eligible_agents
                .saturating_add(page.stats.blocked_agents)
        || page.stats.examined_agents > crate::runtime::limits::GC_CANDIDATES as u64
        || page.stats.deleted_agents
            != if report.dry_run {
                0
            } else {
                page.stats.eligible_agents
            }
        || page.selected_preview.len() > api::GC_PREVIEW
        || page.blockers.len() > api::GC_PREVIEW
        || (report.dry_run && page.cleanup_pending)
}
