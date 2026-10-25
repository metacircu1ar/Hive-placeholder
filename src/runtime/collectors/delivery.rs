use super::{Delivery, Destination};
use crate::{
    error::{Error, Result},
    runtime::hub::Hub,
};
use comb_protocol::service::{
    Operation,
    auth::{Claim, Connection},
    client,
};
use comb_protocol::values::{RequestId, RunName};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};

const DEADLINE: Duration = Duration::from_secs(10);
const BATCH_ITEMS: usize = 32;
const BATCH_TIME: Duration = Duration::from_secs(1);
static DELIVERED: AtomicU64 = AtomicU64::new(0);
static FAILED: AtomicU64 = AtomicU64::new(0);

fn delivery_failed(phase: &'static str, error: &(dyn std::error::Error + 'static)) {
    if let Some(count) = crate::observability::sample(&FAILED) {
        tracing::warn!(name: "collector.delivery_failed",
             phase = phase,
             count = count,
             error = error,
             decision = "retain_outbox");
    }
}

pub(in crate::runtime) async fn run(hub: Hub, shutdown: Arc<AtomicBool>) -> Result<()> {
    let mut interval = tokio::time::interval(COLLECTION_INTERVAL);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut jobs = tokio::task::JoinSet::new();
    let mut active = std::collections::HashSet::new();
    let mut cursor = String::new();
    loop {
        if shutdown.load(Ordering::Relaxed) {
            jobs.abort_all();
            while let Some(result) = jobs.join_next().await {
                match result {
                    Ok((_, Err(error))) => delivery_failed("shutdown", &error),
                    Err(error) => {
                        let task_id = error.id().to_string();
                        tracing::debug!(name: "collector.task_stopped",
                             task_id = task_id,
                             code = if error.is_panic() { "panicked" } else { "cancelled" },
                             error = &error as &(dyn std::error::Error + 'static),
                             decision = "retain_outbox");
                    }
                    Ok((_, Ok(()))) => (),
                }
            }
            return Ok(());
        }
        tokio::select! {
            _ = interval.tick() => {
                if jobs.len() >= 4 { continue; }
                let Some(reader) = hub.reader.get() else { continue; };
                let mut destinations = match reader.collectors().await {
                    Err(error) if shutdown.load(Ordering::Relaxed) => {
                        crate::observability::observation_failed("collector_shutdown_read", &error);
                        continue;
                    }
                    result => result?,
                };
                let pivot = destinations.partition_point(|destination| destination.name <= cursor);
                destinations.rotate_left(pivot);
                for destination in destinations {
                    if jobs.len() >= 4 { break; }
                    if active.insert(destination.name.clone()) {
                        cursor.clone_from(&destination.name);
                        let hub = hub.clone();
                        jobs.spawn(async move {
                            let result = deliver(&hub, &destination).await;
                            (destination.name, result)
                        });
                    }
                }
            }
            result = jobs.join_next(), if !jobs.is_empty() => {
                let (name, result) = result.ok_or_else(|| Error::internal_error("Collector \
                    task disappeared."))?
                    .map_err(|cause| {
                        Error::internal_error("Collector task failed.").with_source(cause)
                    })?;
                active.remove(&name);
                result?;
                interval.reset_immediately();
            }
        }
    }
}
async fn deliver(hub: &Hub, destination: &Destination) -> Result<()> {
    let path = destination.credential_file.clone();
    let connection = tokio::task::spawn_blocking(move || Connection::read(&path))
        .await
        .map_err(|cause| {
            Error::internal_error("Collector credential worker failed.").with_source(cause)
        })?;
    let connection = match connection {
        Ok(connection) => connection,
        Err(error) => {
            delivery_failed("credential_read", &error);
            return destination_error(hub, destination, error.code.as_str().to_owned()).await;
        }
    };
    if let Err(error) = client::call(
        &connection,
        &Operation::Capabilities,
        Some(Claim::Collector),
        DEADLINE,
    )
    .await
    {
        delivery_failed("capabilities", &error);
        return destination_error(hub, destination, error.code.as_str().to_owned()).await;
    }
    let comb = connection.credential.comb_id.to_string();
    let credential = connection.credential.id.as_str().to_owned();
    let run = RunName::try_from("hive-collector".to_owned()).map_err(|cause| {
        Error::internal_error("Invalid collector run attribution.").with_source(cause)
    })?;
    let started = std::time::Instant::now();
    for _ in 0..BATCH_ITEMS {
        // Bound admission, not an already submitted capture's timeout/receipt.
        if started.elapsed() >= BATCH_TIME {
            break;
        }
        let claim = destination.clone();
        let comb = comb.clone();
        let credential = credential.clone();
        let item = hub
            .with(move |core| core.store().claim_collection(&claim, &comb, &credential))
            .await?;
        let Some(item) = item else {
            break;
        };
        let operation = Operation::Capture {
            text: item.body.clone(),
            request_id: request_id(&item)?,
            run: Some(run.clone()),
        };
        let error = client::call(&connection, &operation, Some(Claim::Collector), DEADLINE)
            .await
            .inspect_err(|error| delivery_failed("capture", error))
            .err()
            .map(|error| error.code.as_str().to_owned());
        let failure = error.clone();
        hub.with(move |core| core.store().finish_collection(&item, error.as_deref()))
            .await?;
        if let Some(code) = failure {
            // Do not pin another item to possibly revoked credentials.
            return destination_error(hub, destination, code).await;
        }
        if let Some(count) = crate::observability::sample(&DELIVERED) {
            tracing::info!(name: "collector.delivered", count = count);
        }
    }
    Ok(())
}
fn request_id(item: &Delivery) -> Result<RequestId> {
    format!("{}-{}", item.id, item.generation)
        .try_into()
        .map_err(|cause| {
            Error::internal_error("Invalid collector request identity.").with_source(cause)
        })
}
async fn destination_error(hub: &Hub, destination: &Destination, code: String) -> Result<()> {
    let destination = destination.clone();
    hub.with(move |core| {
        core.store()
            .collection_destination_error(&destination, &code)
    })
    .await
}

const COLLECTION_INTERVAL: std::time::Duration = Duration::from_millis(250);
