use super::{Platform, Spec, path_text};
use crate::{Result, invalid};

fn text(value: &str) -> Result<&str> {
    if value.chars().any(char::is_control) {
        return Err(invalid(
            "Service arguments cannot contain control characters.",
        ));
    }
    Ok(value)
}
fn xml(value: &str) -> Result<String> {
    Ok(text(value)?
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;"))
}
fn systemd(value: &str) -> Result<String> {
    Ok(format!(
        "\"{}\"",
        text(value)?
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('%', "%%")
            .replace('$', "$$")
    ))
}
pub(super) fn unit(spec: &Spec) -> Result<Vec<u8>> {
    let label = spec.scope.label()?;
    let mut args = vec![path_text(&spec.program)?];
    args.extend(spec.arguments.clone());
    args.extend(["--managed-service".into(), path_text(&spec.descriptor)?]);
    let log = path_text(&spec.log)?;
    let unit = match spec.platform {
        Platform::Launchd => {
            let arguments = args
                .iter()
                .map(|a| Ok(format!("<string>{}</string>", xml(a)?)))
                .collect::<Result<Vec<_>>>()?
                .join("");
            let environment = spec
                .environment
                .iter()
                .map(|(k, v)| {
                    Ok(format!(
                        "<key>{}</key><string>{}</string>",
                        xml(k)?,
                        xml(v)?
                    ))
                })
                .collect::<Result<Vec<_>>>()?
                .join("");
            format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \
                \"-//Apple//DTD PLIST 1.0//EN\" \"http:\
                //www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist \
                version=\"1.0\"><dict>\n<key>Label</key><string>{label}\
                </string>\n<key>ProgramArguments</key><array>{arguments}\
                </array>\n<key>EnvironmentVariables</key><dict>{environment}\
                </dict>\n<key>RunAtLoad</key><true/>\n<key>KeepAlive</key><dict><key>Successfu\
                lExit</key><false/></dict>\n<key>ThrottleInterval</key><integer>10</integer>\n\
                <key>ExitTimeOut</key><integer>30</integer>\n<key>Umask</key><integer>63</inte\
                ger>\n<key>StandardOutPath</key><string>{}\
                </string>\n<key>StandardErrorPath</key><string>{}</string>\n</dict></plist>\n",
                xml(&log)?,
                xml(&log)?
            )
        }
        Platform::Systemd => {
            let arguments = args
                .iter()
                .map(|a| systemd(a))
                .collect::<Result<Vec<_>>>()?
                .join(" ");
            // Environment= and file paths use specifier escaping, but not ExecStart's $ expansion.
            let quote = |s: &str| -> Result<String> { Ok(systemd(s)?.replace("$$", "$")) };
            let environment = spec
                .environment
                .iter()
                .map(|(k, v)| Ok(format!("Environment={}\n", quote(&format!("{k}={v}"))?)))
                .collect::<Result<String>>()?;
            let log = text(&log)?.replace('%', "%%");
            format!(
                "[Unit]\nDescription=Hive {} user \
                service\nStartLimitIntervalSec=60\nStartLimitBurst=5\n\n[Service]\
                \nType=exec\nExecStart={arguments}\n{environment}\
                Restart=on-failure\nRestartSec=10\nTimeoutStopSec=30\nKillMode=control-group\n\
                UMask=0077\nStandardOutput=append:{log}\nStandardError=append:{log}\
                \n\n[Install]\nWantedBy=default.target\n",
                spec.scope.product
            )
        }
    };
    Ok(unit.into_bytes())
}
