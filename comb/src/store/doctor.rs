use super::limits::{CITATION_CHARS, FINDING_DETAIL_CHARS, IGNORE_BYTES, PAGE_NEAR_LIMIT_BYTES};
use super::{
    BTreeSet, Comb, Doctor, Filter, Finding, FindingKind, Findings, Page, Prefix, Result, Slug,
    Stats,
};
mod tracking;

impl Comb {
    pub fn stats(&self) -> Result<Stats> {
        self.ready()?;
        let mut stats = self.index.stats()?;
        stats.embedding_runtime.clone_from(&self.embedding_runtime);
        Ok(stats)
    }
    pub fn doctor(&self) -> Result<Doctor> {
        self.ready()?;
        let mut findings = Findings::default();
        findings.extend(self.index.embedding_findings()?);
        for item in self.disk.pages() {
            let (slug, raw) = item?;
            match raw {
                Ok(raw) => self.page_findings(slug, raw, &mut findings)?,
                Err(error) => findings.push(Finding {
                    slug: slug.to_string(),
                    kind: FindingKind::InvalidPage,
                    detail: error.message.chars().take(FINDING_DETAIL_CHARS).collect(),
                }),
            }
        }
        self.private_state_findings(&mut findings)?;
        let (findings, findings_truncated) = findings.into_parts();
        tracing::info!(name: "comb.doctor_finished",
             count = findings.len(),
             partial = findings_truncated);
        Ok(Doctor {
            embedding_worker_error: self.embedding_worker_error.clone(),
            embeddings: self.index.embedding_coverage(Filter::default())?,
            stats: self.stats()?,
            findings_truncated,
            findings,
            citation_check: "Structural references only; evidence support requires review.".into(),
        })
    }
    fn page_findings(&self, slug: Slug, raw: String, findings: &mut Findings) -> Result<()> {
        let page = match Page::parse(&raw) {
            Ok(page) => page,
            Err(error) => {
                findings.push(Finding {
                    slug: slug.to_string(),
                    kind: FindingKind::InvalidPage,
                    detail: error.message.chars().take(FINDING_DETAIL_CHARS).collect(),
                });
                return Ok(());
            }
        };
        let loaded = self.disk.load_parsed(&slug, raw, page)?;
        self.loaded_findings(&slug, loaded, findings)
    }
    fn loaded_findings(
        &self,
        slug: &Slug,
        loaded: crate::disk::Loaded,
        findings: &mut Findings,
    ) -> Result<()> {
        let raw = loaded.raw;
        let page = loaded.page;
        let mut finding = |kind: FindingKind, detail: String| {
            findings.push(Finding {
                slug: slug.to_string(),
                kind,
                detail: detail.chars().take(FINDING_DETAIL_CHARS).collect(),
            });
        };
        if raw.len() >= PAGE_NEAR_LIMIT_BYTES {
            finding(
                FindingKind::PageNearLimit,
                "Page is at least 75% of the 1 MiB limit; put further evidence in a linked page."
                    .into(),
            );
        }
        if self.index.content_hash(slug)?.as_ref() != Some(&loaded.hash) {
            finding(
                FindingKind::IndexStale,
                "File differs from its search index; run sync.".into(),
            );
        }
        if page.metadata.owned().external() {
            finding(
                FindingKind::ExternalEdit,
                "Unexplained file edit; owned trust and receipts were discarded.".into(),
            );
        }
        if Prefix::recognize(slug).is_none() {
            finding(
                FindingKind::UnknownPrefix,
                "Page is outside the standard filing prefixes.".into(),
            );
        }
        if self.index.links(slug, true)?.is_empty() && self.index.links(slug, false)?.is_empty() {
            finding(
                FindingKind::Orphan,
                "Page has no incoming or outgoing links.".into(),
            );
        }
        // Query missing targets directly: the public links API is paginated.
        self.index.visit_broken_links(slug, |target| {
            finding(FindingKind::BrokenLink, target);
        })?;
        citation_findings(&page, finding);
        Ok(())
    }
    fn private_state_findings(&self, findings: &mut Findings) -> Result<()> {
        let ignore = self.disk.admin.join(".gitignore");
        if !crate::disk::exists(&ignore)? || crate::disk::read(&ignore, IGNORE_BYTES)? != b"*\n" {
            findings.push(Finding {
                slug: ".comb".into(),
                kind: FindingKind::PrivateStateUnprotected,
                detail: "Restore .comb/.gitignore with a single * rule; administrative state \
                    must be backed up privately, never committed."
                    .into(),
            });
        }
        if let Some(finding) = tracking::finding(tracking::check(&self.disk.root)) {
            findings.push(finding);
        }
        Ok(())
    }
}

fn citation_findings(page: &Page, mut finding: impl FnMut(FindingKind, String)) {
    let ids = page
        .timeline
        .iter()
        .map(|e| e.metadata.authored.id.as_str())
        .collect::<BTreeSet<_>>();
    for paragraph in page
        .compiled
        .split("\n\n")
        .filter(|p| !p.trim().is_empty() && !p.trim().starts_with('#'))
    {
        let mut cited = false;
        for tail in paragraph.split("[evidence:").skip(1) {
            if let Some((id, _)) = tail.split_once(']') {
                cited = true;
                if !ids.contains(id) {
                    finding(FindingKind::DanglingCitation, id.into());
                }
            }
        }
        if !cited {
            finding(
                FindingKind::UncitedParagraph,
                paragraph.chars().take(CITATION_CHARS).collect(),
            );
        }
    }
}
