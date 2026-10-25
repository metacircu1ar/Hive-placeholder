use super::*;

#[tokio::test]
async fn validated_launch_stays_pinned_when_installation_symlink_changes() {
    use std::os::unix::fs::symlink;
    let runtime = crate::runtime::test_support::RuntimeFixture::new();
    let first = runtime.temp.path().join("provider-first");
    let second = runtime.temp.path().join("provider-second");
    for (path, text) in [(&first, "first"), (&second, "second")] {
        crate::runtime::test_support::codex_binary(path, &format!("sys.stdout.write('{text}')"));
    }
    let alias = runtime.temp.path().join("provider");
    symlink(&first, &alias).unwrap();
    let leases = Leases::default();
    for expected in ["first", "second"] {
        let candidate = alias.clone();
        let input = runtime
            .hub
            .with(move |core| {
                let mut agent = core.store().agent("agent")?;
                agent.cwd = core.config_dir().clone();
                agent.conversation_clean = true;
                Ok(WorkerInput {
                    launcher: core.launcher().clone(),
                    state: core.state().clone(),
                    profile: Ok(agent.profile.clone()),
                    binary: Ok(candidate),
                    agent,
                })
            })
            .await
            .unwrap();
        let launch = input.resolve(&leases).checked.unwrap();
        if expected == "first" {
            std::fs::remove_file(&alias).unwrap();
            symlink(&second, &alias).unwrap();
        }
        let output = crate::os::output(&mut std::process::Command::new(&launch.binary)).unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout, expected.as_bytes());
    }
    runtime.finish();
}

#[tokio::test]
async fn cancelled_request_keeps_running_filesystem_work_owned_until_join() {
    let Started { service, owner } = Service::start(
        2,
        Arc::default(),
        Arc::default(),
        Arc::default(),
        Arc::default(),
    );
    let (entered, running) = oneshot::channel();
    let (release, blocked) = std::sync::mpsc::channel();
    let copy = service.clone();
    let request = tokio::spawn(async move { copy.hold(entered, blocked).await });
    running.await.unwrap();
    request.abort();
    assert!(request.await.unwrap_err().is_cancelled());
    let (finished, mut done) = oneshot::channel();
    let joining = std::thread::spawn(move || {
        let _ = finished.send(owner.finish());
    });
    let premature = tokio::time::timeout(std::time::Duration::from_millis(20), &mut done).await;
    release.send(()).unwrap();
    assert!(premature.is_err());
    tokio::time::timeout(std::time::Duration::from_secs(2), done)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    joining.join().unwrap();
    assert!(service.0.closed.load(Ordering::Acquire));
    assert!(service.0.capacity.is_closed());
}

#[tokio::test]
async fn cached_leases_are_weak_and_each_checkout_rechecks_files() {
    let runtime = crate::runtime::test_support::RuntimeFixture::new();
    let input = runtime
        .hub
        .with(|core| {
            let mut agent = core.store().agent("agent")?;
            agent.cwd = core.config_dir().clone();
            agent.conversation_clean = true;
            Ok(WorkerInput {
                launcher: core.launcher().clone(),
                state: core.state().clone(),
                profile: Ok(agent.profile.clone()),
                binary: Ok("/unused-test-provider".into()),
                agent,
            })
        })
        .await
        .unwrap();
    let state = input.state.clone();
    let binding = state
        .generation(
            &input.agent.auth,
            &input.agent.profile,
            input.agent.auth_generation,
        )
        .unwrap();
    let leases = Leases::default();
    let first = leases.checkout(&state, &binding).unwrap();
    let second = leases.checkout(&state, &binding).unwrap();
    assert!(Arc::ptr_eq(&first, &second));
    assert_eq!(leases.live_count(), 1);
    let identity = binding.directory.join("identity.json");
    let saved = std::fs::read(&identity).unwrap();
    std::fs::remove_file(&identity).unwrap();
    assert!(leases.checkout(&state, &binding).is_err());
    crate::state::write_json(
        &identity,
        &serde_json::from_slice::<serde_json::Value>(&saved).unwrap(),
    )
    .unwrap();
    drop(first);
    drop(second);
    assert_eq!(leases.live_count(), 0);
    runtime.finish();
}
