//! Synthetic encodings exercise real publication without shipping historical
//! readers. A released adapter must provide its frozen parser and seal encoding.
use super::*;
use crate::{
    Actor, Page, golden,
    values::{ContentHash, Slug},
};
use hive_setup_support::{files, migration::Step};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::{
    io,
    path::PathBuf,
    sync::atomic::{AtomicBool, Ordering},
};

struct Encoding<'a> {
    disk: &'a Disk,
    slugs: Vec<Slug>,
    receipt: PathBuf,
    from: u32,
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Head {
    slug: Slug,
    hash: ContentHash,
}

impl Encoding<'_> {
    fn tag(&self, number: u32, slug: &Slug, page: &Page) -> String {
        let inner = (&self.disk.identity.id, slug, page.unsigned());
        let input = match number {
            1 => serde_json::to_vec(&inner).unwrap(),
            2 => serde_json::to_vec(&("synthetic-seal-two", inner)).unwrap(),
            3 => serde_json::to_vec(
                &serde_json::json!({"encoding":"synthetic-three","payload":inner}),
            )
            .unwrap(),
            _ => panic!("unregistered synthetic reader"),
        };
        let mut mac = Hmac::<Sha256>::new_from_slice(&self.disk.identity.key).unwrap();
        mac.update(&input);
        format!("{:x}", mac.finalize().into_bytes())
    }
    fn head(&self, slug: &Slug) -> PathBuf {
        self.disk
            .head_path(slug)
            .strip_prefix(&self.disk.root)
            .unwrap()
            .into()
    }
    fn inspect(&self, root: &Path, number: u32, require_trust: bool) -> io::Result<()> {
        for slug in &self.slugs {
            let raw = fs::read_to_string(root.join(format!("{slug}.md")))?;
            let page = Page::parse(&raw).map_err(io::Error::other)?;
            let head: Head = serde_json::from_slice(&fs::read(root.join(self.head(slug)))?)?;
            if head.slug != *slug
                || (require_trust
                    && (head.hash != ContentHash::digest(&raw)
                        || page.metadata.owned().seal()
                            != Some(self.tag(number, slug, &page).as_str())))
            {
                return Err(io::Error::other("Synthetic seal/head mismatch."));
            }
        }
        Ok(())
    }
}
impl Step for Encoding<'_> {
    fn from(&self) -> u32 {
        self.from
    }
    fn to(&self) -> u32 {
        self.from + 1
    }
    fn files(&self, _: &Path) -> io::Result<Vec<PathBuf>> {
        let mut paths = vec![
            ".comb/format".into(),
            ".comb/identity.json".into(),
            self.receipt.clone(),
        ];
        for slug in &self.slugs {
            paths.extend([format!("{slug}.md").into(), self.head(slug)]);
        }
        Ok(paths)
    }
    fn validate_before(&self, root: &Path) -> io::Result<()> {
        self.inspect(root, self.from(), false)
    }
    fn validate_after(&self, root: &Path) -> io::Result<()> {
        self.inspect(root, self.to(), true)
    }
    fn transform(&self, root: &Path) -> io::Result<()> {
        for slug in &self.slugs {
            let path = root.join(format!("{slug}.md"));
            let raw = fs::read_to_string(&path)?;
            let mut page = Page::parse(&raw).map_err(io::Error::other)?;
            let head: Head = serde_json::from_slice(&fs::read(root.join(self.head(slug)))?)?;
            // Authenticate the exact old bytes before rendering or externalizing.
            // No promotion/touch operation belongs in a format migration.
            if head.hash != ContentHash::digest(&raw)
                || page.metadata.owned().seal() != Some(self.tag(self.from(), slug, &page).as_str())
            {
                page.externalize();
            }
            page.install_seal(self.tag(self.to(), slug, &page));
            let next = page.render().map_err(io::Error::other)?;
            files::write(&path, next.as_bytes(), true)?;
            files::write(
                &root.join(self.head(slug)),
                &serde_json::to_vec(&Head {
                    slug: slug.clone(),
                    hash: ContentHash::digest(next),
                })?,
                true,
            )?;
        }
        files::write(
            &root.join(".comb/format"),
            &serde_json::to_vec(&serde_json::json!({"format":self.to()}))?,
            true,
        )
    }
}

struct Failure {
    path: PathBuf,
    original: Vec<u8>,
    fired: AtomicBool,
}
impl atomic_fs::Barriers for Failure {
    fn observe(&self, step: atomic_fs::Step) -> io::Result<()> {
        if step == atomic_fs::Step::AfterPublish
            && fs::read(&self.path)? != self.original
            && !self.fired.swap(true, Ordering::SeqCst)
        {
            return Err(io::Error::from_raw_os_error(libc::EIO));
        }
        Ok(())
    }
}

fn fixture() -> (tempfile::TempDir, Vec<(String, Page)>) {
    let pages: Vec<_> = golden::actors()
        .into_iter()
        .enumerate()
        .map(|(n, actor)| (format!("concepts/actor-{n}"), golden::page(actor)))
        .collect();
    let mut borrowed: Vec<_> = pages
        .iter()
        .map(|(slug, page)| (slug.as_str(), page.clone()))
        .collect();
    borrowed.push(("concepts/untrusted", golden::page(Actor::Human)));
    let (root, comb) = crate::disk::golden::seeded_comb(&borrowed);
    let forged: Slug = "concepts/untrusted".parse().unwrap();
    let original = comb.disk.content(&forged).unwrap().unwrap();
    let mut page = Page::parse(&original).unwrap();
    page.install_seal("00".repeat(32));
    comb.disk
        .write(
            &forged,
            Some(&page),
            Some(&ContentHash::digest(original)),
            None,
        )
        .unwrap();
    comb.disk.finish().unwrap();
    let receipt = crate::page::Receipt {
        principal: Actor::Human.principal().unwrap(),
        request_id: "retained".parse().unwrap(),
        fingerprint: "ab".repeat(32),
        operation: "put".into(),
        slug: pages[0].0.parse().unwrap(),
    };
    comb.disk.set_receipt(&receipt.into()).unwrap();
    drop(comb);
    (root, pages)
}

#[test]
fn synthetic_seal_chain_preserves_review_authorship_witnesses_and_receipts_after_interruption() {
    let (root, pages) = fixture();
    let owner = Owner::acquire(root.path()).unwrap();
    let disk = &owner.disk;
    let receipt_path = disk
        .receipt_path(Actor::Human.principal().unwrap().as_str(), "retained")
        .unwrap();
    let receipt_bytes = fs::read(&receipt_path).unwrap();
    let identity_bytes = fs::read(disk.admin.join("identity.json")).unwrap();
    let lock_inode = fs::metadata(disk.admin.join("lock")).unwrap().ino();
    let check = || owner.check().map_err(io::Error::other);
    let engine = Engine::new(&disk.root, &disk.admin, Path::new(".comb/format"), &check).unwrap();
    let mut step = Encoding {
        disk,
        slugs: pages
            .iter()
            .map(|(slug, _)| slug.parse().unwrap())
            .chain(["concepts/untrusted".parse().unwrap()])
            .collect(),
        receipt: receipt_path.strip_prefix(&disk.root).unwrap().into(),
        from: 1,
    };
    let path = disk.root.join("concepts/actor-0.md");
    let original = fs::read(&path).unwrap();
    let failure = Failure {
        path: path.clone(),
        original: original.clone(),
        fired: AtomicBool::new(false),
    };
    assert!(engine.apply(&step, &failure).is_err());
    assert!(failure.fired.load(Ordering::SeqCst));
    let pending = engine.pending().unwrap().unwrap();
    assert_eq!(
        fs::read(
            disk.admin
                .join("backups")
                .join(&pending.backup)
                .join("concepts/actor-0.md")
        )
        .unwrap(),
        original
    );
    engine.recover(&step, &atomic_fs::DefaultBarriers).unwrap();
    step.from = 2;
    engine.apply(&step, &atomic_fs::DefaultBarriers).unwrap();
    for (slug, expected) in pages {
        let page = Page::parse(&fs::read_to_string(disk.root.join(format!("{slug}.md"))).unwrap())
            .unwrap();
        assert_eq!(
            serde_json::to_value(page.unsigned()).unwrap(),
            serde_json::to_value(expected.unsigned()).unwrap()
        );
        assert_eq!(page.verified(), expected.verified());
        assert_eq!(page.reviewed_start(), expected.reviewed_start());
        assert_eq!(
            crate::page::ContentWitness::new(&page).unwrap(),
            crate::page::ContentWitness::new(&expected).unwrap()
        );
    }
    let external =
        Page::parse(&fs::read_to_string(disk.root.join("concepts/untrusted.md")).unwrap()).unwrap();
    assert!(!external.verified());
    assert_eq!(external.reviewed_start(), None);
    assert!(external.metadata.owned().promotion().is_none());
    assert_eq!(fs::read(receipt_path).unwrap(), receipt_bytes);
    assert_eq!(
        fs::read(disk.admin.join("identity.json")).unwrap(),
        identity_bytes
    );
    assert_eq!(
        fs::metadata(disk.admin.join("lock")).unwrap().ino(),
        lock_inode
    );
    assert_eq!(engine.history().unwrap().len(), 2);
}
