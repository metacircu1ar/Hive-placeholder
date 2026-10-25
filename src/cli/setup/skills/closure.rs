//! Traverse the rendered local links of the shipped skills, not a second package list.
use super::{Result, invalid, io};
use pulldown_cmark::{Event, Options, Parser, Tag};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Component, Path, PathBuf},
};

pub(super) fn bundle(product: &Path) -> Result<BTreeMap<PathBuf, Vec<u8>>> {
    let product = product.canonicalize().map_err(io)?;
    let mut todo = vec![
        PathBuf::from("skills/hive/SKILL.md"),
        PathBuf::from("skills/comb-curator/SKILL.md"),
    ];
    let mut visited = BTreeSet::new();
    let mut files = BTreeMap::new();
    let mut total = 0;
    while let Some(relative) = todo.pop() {
        if !visited.insert(relative.clone()) {
            continue;
        }
        if visited.len() > 2048 || relative.components().count() > 64 {
            return Err(invalid(
                "Documentation closure exceeds its path/count limit.",
            ));
        }
        let path = product.join(&relative);
        if path.canonicalize().map_err(io)? != path {
            return Err(invalid("Bundled documentation must not contain symlinks."));
        }
        let meta = path.symlink_metadata().map_err(io)?;
        if meta.is_dir() {
            for item in std::fs::read_dir(&path).map_err(io)? {
                todo.push(relative.join(item.map_err(io)?.file_name()));
            }
            continue;
        }
        if !meta.is_file() || meta.len() > 2 * 1024 * 1024 {
            return Err(invalid("Unsupported or oversized bundled document."));
        }
        let bytes = std::fs::read(&path).map_err(io)?;
        total += bytes.len();
        if total > 32 * 1024 * 1024 {
            return Err(invalid("Documentation closure exceeds 32 MiB."));
        }
        if path.extension().is_some_and(|s| s == "md") {
            let text = std::str::from_utf8(&bytes)
                .map_err(|cause| invalid("Documentation is not UTF-8.").with_source(cause))?;
            for link in links(text)? {
                if let Some(target) = target(&relative, &link)? {
                    todo.push(target);
                }
            }
        }
        files.insert(relative, bytes);
    }
    Ok(files)
}
fn links(text: &str) -> Result<Vec<String>> {
    let html = regex::Regex::new(r#"(?:src|href)\s*=\s*["']([^"']+)["']"#)
        .map_err(|cause| invalid("Invalid bundled skill link expression.").with_source(cause))?;
    let mut links = vec![];
    for event in Parser::new_ext(text, Options::all()) {
        match event {
            Event::Start(Tag::Link { dest_url, .. } | Tag::Image { dest_url, .. }) => {
                links.push(dest_url.into_string());
            }
            Event::Html(text) | Event::InlineHtml(text) => {
                links.extend(
                    html.captures_iter(&text)
                        .filter_map(|c| c.get(1).map(|m| m.as_str().to_owned())),
                );
            }
            _ => (),
        }
    }
    Ok(links)
}
fn target(source: &Path, link: &str) -> Result<Option<PathBuf>> {
    let path = link.split(['#', '?']).next().unwrap_or_default();
    if path.is_empty() || path.starts_with("//") || path.contains(':') {
        return Ok(None);
    }
    let path = percent_encoding::percent_decode_str(path)
        .decode_utf8()
        .map_err(|cause| invalid("Invalid document link encoding.").with_source(cause))?;
    let mut result = source.parent().unwrap_or(Path::new("")).to_owned();
    for part in Path::new(path.as_ref()).components() {
        match part {
            Component::Normal(s) => result.push(s),
            Component::CurDir => (),
            Component::ParentDir if result.pop() => (),
            _ => return Err(invalid("Document link escapes the shipped product root.")),
        }
    }
    Ok(Some(result))
}
/// Move only link destinations; code examples and ordinary prose stay unchanged.
pub(super) fn relocate(source: &Path, text: &str) -> Result<Vec<u8>> {
    let mut replacements = BTreeMap::new();
    for link in links(text)? {
        if let Some(target) = target(source, &link)? {
            let suffix = link.find(['#', '?']).map_or("", |n| &link[n..]);
            let replacement = format!("bundle/{}{suffix}", target.to_string_lossy());
            replacements.insert(link, replacement);
        }
    }
    // Offset events cover inline syntax; reference definitions are not events.
    // Match destinations in markup only, then consult the parser-derived set.
    let pattern = regex::Regex::new(
        r#"(?m)(\]\(\s*<?|(?:src|href)\s*=\s*["']|^\s*\[[^\]]+\]:\s*<?)([^\s)>"']+)"#,
    )
    .map_err(|cause| invalid("Invalid bundled skill destination expression.").with_source(cause))?;
    let mut code = vec![];
    let mut start = None;
    for (event, range) in Parser::new_ext(text, Options::all()).into_offset_iter() {
        match event {
            Event::Start(Tag::CodeBlock(_)) => start = Some(range.start),
            Event::End(pulldown_cmark::TagEnd::CodeBlock) => {
                if let Some(start) = start.take() {
                    code.push(start..range.end);
                }
            }
            Event::Code(_) => code.push(range),
            _ => (),
        }
    }
    Ok(pattern
        .replace_all(text, |c: &regex::Captures<'_>| {
            if let Some(matched) = c.get(0)
                && code.iter().any(|r| r.contains(&matched.start()))
            {
                return matched.as_str().to_owned();
            }
            let prefix = c.get(1).map_or("", |m| m.as_str());
            let link = c.get(2).map_or("", |m| m.as_str());
            format!(
                "{prefix}{}",
                replacements.get(link).map_or(link, String::as_str)
            )
        })
        .into_owned()
        .into_bytes())
}
