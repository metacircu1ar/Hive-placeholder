//! Isolated CLI fixture that runs the ordinary command path.
use super::*;
use std::sync::OnceLock;

pub(crate) fn binary() -> &'static Path {
    static FIXTURE: OnceLock<(tempfile::TempDir, PathBuf)> = OnceLock::new();
    &FIXTURE
        .get_or_init(|| {
            let temp = tempfile::tempdir().unwrap();
            let binary = temp.path().join("hive-cli-fixture");
            let executable = std::env::current_exe().unwrap();
            // Reserve the original stdout before libtest prints its prelude. The
            // selected fixture restores it before entering the real CLI, including PTYs.
            let wrapper = format!(
                r"{}import json, os, sys
os.environ['HIVE_CLI_FIXTURE_ARGS'] = json.dumps(['hivectl'] + sys.argv[1:])
os.dup2(1, 198, inheritable=True)
with open(os.devnull, 'wb') as sink:
    os.dup2(sink.fileno(), 1)
os.execv({}, [{}, '--exact', 'auth::cli_tests::child::fixture', '--ignored', '--nocapture'])
",
                support::python_shebang(temp.path()),
                json!(executable),
                json!(executable)
            );
            fs::write(&binary, wrapper).unwrap();
            fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).unwrap();
            (temp, binary)
        })
        .1
}

#[test]
#[ignore = "isolated CLI child"]
fn fixture() {
    let args: Vec<String> =
        serde_json::from_str(&std::env::var("HIVE_CLI_FIXTURE_ARGS").unwrap()).unwrap();
    // SAFETY: the fixture wrapper inherited descriptor 198 as an owned duplicate
    // of stdout. This isolated child restores it once before any CLI work.
    assert_ne!(unsafe { libc::dup2(198, libc::STDOUT_FILENO) }, -1);
    // SAFETY: dup2 above created stdout's independent reference; release the extra.
    assert_eq!(unsafe { libc::close(198) }, 0);
    crate::cli::run_fixture(&args.into_iter().map(Into::into).collect::<Vec<_>>());
    std::process::exit(0);
}
