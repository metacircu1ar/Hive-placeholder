use super::*;

pub(super) fn cli(
    root: &Path,
    args: &[&str],
    input: Option<&str>,
    success: bool,
) -> TestResult<Value> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_comb"))
        .arg("--root")
        .arg(root)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    if let Some(input) = input {
        child
            .stdin
            .take()
            .ok_or("child stdin was not piped")?
            .write_all(input.as_bytes())?;
    }
    let output = child.wait_with_output()?;
    assert_eq!(
        output.status.success(),
        success,
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(serde_json::from_slice(&output.stdout)?)
}

pub(crate) fn optional_bytes(path: impl AsRef<std::path::Path>) -> Option<Vec<u8>> {
    match std::fs::read(path) {
        Ok(bytes) => Some(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => panic!("fixture snapshot read failed: {error}"),
    }
}
