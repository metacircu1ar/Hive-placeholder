//! The one-line `/usage` summary of a login's reported quota usage.
use crate::cli::views::{self, Credits, ExtraUsage, UsageReport, Window};
use serde_json::Value;

const MINUTE: u64 = 60;
const HOUR: u64 = 60 * MINUTE;
const DAY: u64 = 24 * HOUR;

/// `auth`'s report from a `usage` reply, as one line: the agent's credential
/// generation's, else the login's newest. `now` is in Unix seconds.
pub(super) fn summary(auth: &str, generation: Option<&str>, reports: &Value, now: u64) -> String {
    let rows: Vec<UsageReport<'_>> = views::list::<UsageReport<'_>>(reports)
        .into_iter()
        .filter(|report| report.auth == Some(auth))
        .collect();
    let report = rows
        .iter()
        .find(|report| generation.is_some_and(|generation| report.generation == Some(generation)))
        .or(rows.first());
    let Some(report) = report else {
        return format!("{auth}: no usage reported yet; it is reported during this login's turns");
    };
    let mut parts: Vec<String> = report
        .windows
        .iter()
        .map(|window| window_text(window, now))
        .collect();
    if let Some(credits) = report.credits.as_ref() {
        parts.push(credits_text(credits));
    }
    if let Some(extra) = report.extra_usage.as_ref() {
        parts.push(extra_text(extra));
    }
    let observed = report.observed_at.unwrap_or(now);
    parts.push(format!("reported {}", ago(now.saturating_sub(observed))));
    format!("{auth}: {}", parts.join(" · "))
}

fn window_text(window: &Window<'_>, now: u64) -> String {
    let label = label(window.name.unwrap_or("window"), window.minutes);
    let used = window.used_percent;
    match window.resets_at {
        Some(at) if at <= now => format!("{label} reset since this report"),
        Some(at) => format!("{label} {used:.0}% used, resets in {}", span(at - now)),
        None => format!("{label} {used:.0}% used"),
    }
}

/// `five_hour` → `5h`, `seven_day` → `weekly`, `seven_day_opus` → `weekly
/// Opus`; a Codex slot shows its length.
fn label(name: &str, minutes: Option<u64>) -> String {
    match (name, minutes) {
        ("five_hour", _) => String::from("5h"),
        ("seven_day", _) => String::from("weekly"),
        ("primary" | "secondary", Some(minutes)) if minutes % (DAY / MINUTE) == 0 => {
            format!("{}d", minutes / (DAY / MINUTE))
        }
        ("primary" | "secondary", Some(minutes)) if minutes % (HOUR / MINUTE) == 0 => {
            format!("{}h", minutes / (HOUR / MINUTE))
        }
        ("primary" | "secondary", Some(minutes)) => format!("{minutes}m"),
        _ => match name.strip_prefix("seven_day_") {
            Some(scope) => format!("weekly {}", capitalized(&scope.replace('_', " "))),
            None => name.replace('_', " "),
        },
    }
}

fn capitalized(text: &str) -> String {
    let mut characters = text.chars();
    characters
        .next()
        .map(|first| first.to_uppercase().chain(characters).collect())
        .unwrap_or_default()
}

fn credits_text(credits: &Credits<'_>) -> String {
    if credits.unlimited {
        String::from("credits unlimited")
    } else if let Some(balance) = credits.balance {
        // Codex reports the balance with ten decimals; two are enough to read.
        match balance.parse::<f64>() {
            Ok(amount) if amount.is_finite() => format!("credits {amount:.2}"),
            _ => format!("credits {balance}"),
        }
    } else if credits.available {
        String::from("credits available")
    } else {
        String::from("no credits")
    }
}

fn extra_text(extra: &ExtraUsage<'_>) -> String {
    if extra.in_use {
        String::from("extra usage in use")
    } else if extra.status == Some("rejected") {
        String::from("extra usage unavailable")
    } else {
        String::from("extra usage available")
    }
}

/// A time ahead: `2h 38m`, `5d 18h`, `12m`.
fn span(seconds: u64) -> String {
    match seconds {
        0..MINUTE => String::from("under a minute"),
        MINUTE..HOUR => format!("{}m", seconds / MINUTE),
        HOUR..DAY => format!("{}h {}m", seconds / HOUR, seconds % HOUR / MINUTE),
        _ => format!("{}d {}h", seconds / DAY, seconds % DAY / HOUR),
    }
}

/// A time past: `just now`, `3 min ago`, `2h ago`, `4d ago`.
fn ago(seconds: u64) -> String {
    match seconds {
        0..MINUTE => String::from("just now"),
        MINUTE..HOUR => format!("{} min ago", seconds / MINUTE),
        HOUR..DAY => format!("{}h ago", seconds / HOUR),
        _ => format!("{}d ago", seconds / DAY),
    }
}

#[cfg(test)]
mod tests;
