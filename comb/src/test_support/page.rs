use crate::{Page, Result};

pub(crate) struct PageBuilder<'a> {
    title: &'a str,
    body: &'a str,
    kind: &'a str,
    tags: &'a [&'a str],
    aliases: &'a [&'a str],
}
impl<'a> PageBuilder<'a> {
    pub(crate) fn new(title: &'a str, body: &'a str) -> Self {
        Self {
            title,
            body,
            kind: "note",
            tags: &[],
            aliases: &[],
        }
    }
    pub(crate) fn kind(mut self, kind: &'a str) -> Self {
        self.kind = kind;
        self
    }
    pub(crate) fn tags(mut self, tags: &'a [&'a str]) -> Self {
        self.tags = tags;
        self
    }
    pub(crate) fn aliases(mut self, aliases: &'a [&'a str]) -> Self {
        self.aliases = aliases;
        self
    }
    pub(crate) fn build(self) -> Result<Page> {
        Page::parse(&format!(
            "---\ntitle: {}\ntype: {}\ntags: {}\naliases: {}\n---\n{}",
            crate::error::json_text(self.title)?,
            crate::error::json_text(self.kind)?,
            crate::error::json_text(self.tags)?,
            crate::error::json_text(self.aliases)?,
            self.body,
        ))
    }
}
