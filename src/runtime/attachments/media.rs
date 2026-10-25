//! Bounded magic/header inspection; unknown or malformed image dimensions stay references.
pub(super) fn inspect(bytes: &[u8], truncated: bool) -> (&'static str, Option<[u32; 2]>) {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return (
            "image/png",
            bytes
                .get(16..24)
                .map(|b| [be(&b[..4]), be(&b[4..])])
                .filter(|d| valid(*d)),
        );
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return (
            "image/gif",
            bytes
                .get(6..10)
                .map(|b| [le16(&b[..2]), le16(&b[2..])])
                .filter(|d| valid(*d)),
        );
    }
    if bytes.starts_with(b"\xff\xd8\xff") {
        return ("image/jpeg", jpeg(bytes));
    }
    if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        return ("image/webp", webp(bytes));
    }
    if bytes.starts_with(b"%PDF-") {
        return ("application/pdf", None);
    }
    if !bytes.contains(&0)
        && match std::str::from_utf8(bytes) {
            Ok(_) => true,
            Err(e) => truncated && e.error_len().is_none(),
        }
    {
        ("text/plain", None)
    } else {
        ("application/octet-stream", None)
    }
}
fn valid(d: [u32; 2]) -> bool {
    d[0] != 0 && d[1] != 0
}
fn be(b: &[u8]) -> u32 {
    b.iter().fold(0, |v, b| (v << 8) | u32::from(*b))
}
fn le16(b: &[u8]) -> u32 {
    u32::from(b[0]) | u32::from(b[1]) << 8
}
fn le24(b: &[u8]) -> u32 {
    le16(b) | u32::from(b[2]) << 16
}
fn jpeg(b: &[u8]) -> Option<[u32; 2]> {
    let mut at = 2;
    while b.get(at) == Some(&0xff) {
        while b.get(at) == Some(&0xff) {
            at += 1;
        }
        let marker = *b.get(at)?;
        at += 1;
        if matches!(marker, 0xd8 | 0xd9 | 0x01 | 0xd0..=0xd7) {
            continue;
        }
        let length = be(b.get(at..at + 2)?) as usize;
        if length < 2 {
            return None;
        }
        if matches!(marker, 0xc0..=0xc3 | 0xc5..=0xc7 | 0xc9..=0xcb | 0xcd..=0xcf) {
            let d = [be(b.get(at + 5..at + 7)?), be(b.get(at + 3..at + 5)?)];
            return valid(d).then_some(d);
        }
        at += length;
    }
    None
}
fn webp(b: &[u8]) -> Option<[u32; 2]> {
    let d = match b.get(12..16)? {
        b"VP8X" => {
            let p = b.get(24..30)?;
            [le24(&p[..3]) + 1, le24(&p[3..]) + 1]
        }
        b"VP8L" if b.get(20) == Some(&0x2f) => {
            let p = b.get(21..25)?;
            [
                1 + (le16(&p[..2]) & 0x3fff),
                1 + ((u32::from(p[1]) >> 6 | u32::from(p[2]) << 2 | u32::from(p[3]) << 10)
                    & 0x3fff),
            ]
        }
        b"VP8 " if b.get(23..26) == Some(b"\x9d\x01\x2a") => {
            let p = b.get(26..30)?;
            [le16(&p[..2]) & 0x3fff, le16(&p[2..]) & 0x3fff]
        }
        _ => return None,
    };
    valid(d).then_some(d)
}
