use super::*;
use crate::launch::{
    image::{Image, build_image, sweep_images},
    tests::private_root,
};
use std::{cell::Cell, io::Write, os::unix::fs::PermissionsExt};

fn fixture(root: &Path) -> Image {
    fs::write(root.join("source"), b"launcher").unwrap();
    let image = build_image(root, &root.join("source"), |_| {}).unwrap();
    state::private_file(&image.directory.join(".hive-tmp-0123456789abcdef"), true)
        .unwrap()
        .write_all(b"partial")
        .unwrap();
    image
}
fn sweep(root: &Path) {
    let _cache = state::Lock::index(&root.join("cache.lock")).unwrap();
    sweep_images(root, None);
}
fn abandon(mut image: Image) -> Retirement {
    image.retained = true;
    image.retirement.clone()
}
fn retire(root: &Path) -> Retirement {
    let image = fixture(root);
    let retirement = abandon(image);
    fs::rename(&retirement.original, &retirement.aside).unwrap();
    retirement
}

mod constructor;
mod faults;
mod lifetimes;
mod refusal;
mod sync_retry;
