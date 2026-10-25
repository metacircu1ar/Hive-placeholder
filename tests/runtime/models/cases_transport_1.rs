//! Transport regressions.
use super::*;

#[test]
fn claude_catalog_waits_for_clean_exit_and_rejects_failed_or_forced_exit() {
    for (delay, exit, expected_error) in [
        (3, 0, None),
        (0, 7, Some("provider_failed")),
        (30, 0, Some("provider_shutdown_forced")),
    ] {
        let f = super::super::claude::fixture();
        let executable = f.tmp.path().join("claude-discovery");
        let pid_file = f.tmp.path().join("discovery-pid");
        fs::write(
            &executable,
            format!(
                r#"#!{}
import json, os, pathlib, sys, time
if "--version" in sys.argv:
    print("2.1.285 (Claude Code)")
    sys.exit(0)
pathlib.Path({:?}).write_text(str(os.getpid()))
request = json.loads(sys.stdin.readline())
assert request['request']['subtype'] == 'initialize'
print(json.dumps({{'type':'control_response','response':{{'subtype':'success',
 'request_id':request['request_id'],'response':{{
 'account':{{'apiProvider':'firstParty','subscriptionType':'Team'}},
 'models':[{{'value':'sonnet','resolvedModel':'claude-sonnet-test','displayName':'Sonnet',
 'description':'mock','supportedEffortLevels':['low']}}]}}}}}}), flush=True)
assert not sys.stdin.read(), 'discovery must never submit input'
time.sleep({delay})
sys.exit({exit})
"#,
                f.tmp.path().join("python3").display(),
                pid_file.to_str().unwrap()
            ),
        )
        .unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        let output = support::command(env!("CARGO_BIN_EXE_hivectl"))
            .arg("--config")
            .arg(&f.config)
            .arg("--state-dir")
            .arg(&f.root)
            .arg("--claude-bin")
            .arg(&executable)
            .args(["models", "--auth", "claude"])
            .output()
            .unwrap();
        let result: Value = serde_json::from_slice(&output.stdout).unwrap();
        if let Some(error) = expected_error {
            assert!(!output.status.success());
            assert_eq!(result["error"]["code"], error, "{result}");
        } else {
            assert!(output.status.success(), "{result}");
            assert_eq!(result["models"]["data"][0]["model"], "sonnet");
        }
        let pid = fs::read_to_string(pid_file).unwrap();
        let probe = support::command("ps")
            .args(["-p", pid.trim(), "-o", "pid="])
            .output()
            .unwrap();
        assert!(
            probe.stdout.is_empty(),
            "discovery helper must be gone before returning"
        );
    }
}
