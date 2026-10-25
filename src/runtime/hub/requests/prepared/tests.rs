use super::*;
use crate::runtime::hub::requests::{RequestPreparation, tests::configured};
use serde_json::json;

async fn prepared(
    runtime: &crate::runtime::test_support::RuntimeFixture,
    source: bool,
) -> Result<(Arc<PreparedRequest>, Facts)> {
    let request = Arc::new(PreparedRequest {
        caller: Caller::Human,
        id: "revalidation".into(),
        args: RequestEnvelope::new(if source {
            json!({"op":"spawn","from_agent":"agent","body":"branch"})
        } else {
            json!({"op":"resume","target":"agent"})
        }),
    });
    let capture = request.clone();
    let Captured::Input(input) = runtime
        .hub
        .with(move |core| capture.capture::<RequestPreparation>(core))
        .await?
    else {
        panic!("new request unexpectedly replayed")
    };
    let facts = RequestPreparation::prepare(&runtime.hub, &request.args, input).await?;
    if source {
        assert!(matches!(&facts.spawn(&request.args)?.source, Some(Ok(_))));
    }
    Ok((request, facts))
}

#[tokio::test]
async fn source_and_resume_revalidation_propagate_actual_sql_failure()
-> std::result::Result<(), Box<dyn std::error::Error>> {
    for source in [true, false] {
        let runtime = configured().await;
        let (request, facts) = prepared(&runtime, source).await?;
        let db = rusqlite::Connection::open(runtime.temp.path().join("runtime.sqlite"))?;
        // The agent load names this column; SQLite rewrites the view and keeps it
        // working, so only the load fails, with an actual SQL error.
        db.execute_batch(
            "ALTER TABLE agents RENAME COLUMN provider_spawn_after TO renamed_spawn_after",
        )?;
        let result = runtime
            .hub
            .with(move |core| request.dispatch::<RequestPreparation>(core, facts))
            .await;
        // Restore the schema before cleanup or an assertion, even on regression.
        db.execute_batch(
            "ALTER TABLE agents RENAME COLUMN renamed_spawn_after TO provider_spawn_after",
        )?;
        let Err(error) = result else {
            panic!("operational failure was lost: {result:?}")
        };
        assert_eq!(error.code(), "database_error", "source={source}");
        assert_eq!(
            crate::runtime::store::sqlite_code(&error),
            rusqlite::ffi::SQLITE_ERROR
        );
        runtime.finish();
    }
    Ok(())
}

#[tokio::test]
async fn source_and_resume_deleted_identity_still_retry()
-> std::result::Result<(), Box<dyn std::error::Error>> {
    for source in [true, false] {
        let runtime = configured().await;
        let (request, facts) = prepared(&runtime, source).await?;
        let db = rusqlite::Connection::open(runtime.temp.path().join("runtime.sqlite"))?;
        // Remove this fixture identity and its mailbox references as a concurrent
        // deletion would; the separate purge tests enforce deletion eligibility.
        db.execute("DELETE FROM messages WHERE recipient='agent'", [])?;
        db.execute("DELETE FROM agent_instructions WHERE agent='agent'", [])?;
        db.execute("DELETE FROM agents WHERE id='agent'", [])?;
        assert_eq!(
            runtime
                .hub
                .with(move |core| request.dispatch::<RequestPreparation>(core, facts))
                .await?,
            None
        );
        runtime.finish();
    }
    Ok(())
}
