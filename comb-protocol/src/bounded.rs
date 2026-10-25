//! Bounded collection for already-open readers; callers own opening policy.
use crate::{Error, Result};
use std::io::{self, Read, Take};

/// Collect bytes, reading at most one byte beyond `max` to detect oversized input.
pub fn bytes(reader: impl Read, max: usize, oversized: impl FnOnce() -> Error) -> Result<Vec<u8>> {
    collect(reader, max, oversized, Read::read_to_end)
}

/// Decode text before checking its size, preserving UTF-8 error precedence.
pub fn text(reader: impl Read, max: usize, oversized: impl FnOnce() -> Error) -> Result<String> {
    collect(reader, max, oversized, Read::read_to_string)
}

fn collect<R: Read, T: Default>(
    reader: R,
    max: usize,
    oversized: impl FnOnce() -> Error,
    read: impl FnOnce(&mut Take<R>, &mut T) -> io::Result<usize>,
) -> Result<T> {
    let mut output = T::default();
    let count = read(
        &mut reader.take(crate::convert::usize_u64(max).saturating_add(1)),
        &mut output,
    )?;
    if count > max {
        return Err(oversized());
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ErrorCode, error::tests::io_cause};
    use std::{cell::Cell, error::Error as _, io::Cursor};

    #[test]
    fn exact_limits_and_one_byte_probe_preserve_the_remaining_reader() -> Result<()> {
        assert_eq!(bytes(&b"abc"[..], 3, too_large)?, b"abc");
        assert_eq!(text("éé".as_bytes(), 4, too_large)?, "éé");
        assert_eq!(bytes(&b""[..], 0, too_large)?, b"");
        assert_eq!(text(&b""[..], 0, too_large)?, "");

        let calls = Cell::new(0);
        let mut reader = Cursor::new(b"abcde");
        let Err(error) = bytes(&mut reader, 3, || {
            calls.set(calls.get() + 1);
            too_large()
        }) else {
            panic!("oversized bytes must fail");
        };
        assert_eq!(error.code, ErrorCode::SizeLimit);
        assert_eq!(calls.get(), 1);
        assert_eq!(reader.position(), 4);
        let mut remaining = Vec::new();
        reader.read_to_end(&mut remaining)?;
        assert_eq!(remaining, b"e");
        Ok(())
    }

    #[test]
    fn utf8_failure_precedes_size_but_never_reads_beyond_the_probe() {
        let Err(error) = text(&b"a\xffz"[..], 1, || {
            panic!("invalid UTF-8 must win over the size error")
        }) else {
            panic!("invalid UTF-8 must fail");
        };
        assert_eq!(error.code, ErrorCode::IoError);
        assert_eq!(io_cause(&error).kind(), io::ErrorKind::InvalidData);

        // The byte limit may split a valid multibyte character. Keep the old
        // read_to_string error instead of replacing it with a size error.
        let Err(error) = text("éé".as_bytes(), 2, too_large) else {
            panic!("a split UTF-8 character must fail");
        };
        assert_eq!(io_cause(&error).kind(), io::ErrorKind::InvalidData);

        let mut reader = Cursor::new(b"ab\xff");
        let Err(error) = text(&mut reader, 1, too_large) else {
            panic!("the valid probe exceeds the size limit");
        };
        assert_eq!(error.code, ErrorCode::SizeLimit);
        assert_eq!(reader.position(), 2);
    }

    #[test]
    fn read_failures_keep_their_cause_without_evaluating_the_size_error() {
        for error in [
            bytes(Fails, 10, || panic!("read failure must win")).err(),
            text(Fails, 10, || panic!("read failure must win")).err(),
        ] {
            let Some(error) = error else {
                panic!("reader failure must propagate");
            };
            assert_eq!(error.code, ErrorCode::IoError);
            assert_eq!(io_cause(&error).kind(), io::ErrorKind::PermissionDenied);
            assert_eq!(
                error.source().map(ToString::to_string),
                Some("private read failure".into())
            );
            assert!(!error.message.contains("private"));
        }
    }

    fn too_large() -> Error {
        Error::input_size_limit()
    }

    struct Fails;
    impl Read for Fails {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "private read failure",
            ))
        }
    }
}
