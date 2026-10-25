//! Raw operating-system randomness for private keys and credential secrets.
use crate::Result;

pub(crate) fn bytes() -> Result<[u8; 32]> {
    fill_with(getrandom::fill)
}

pub(crate) fn secret() -> Result<String> {
    Ok(hex(&bytes()?))
}

fn fill_with(
    fill: impl FnOnce(&mut [u8]) -> std::result::Result<(), getrandom::Error>,
) -> Result<[u8; 32]> {
    let mut bytes = [0; 32];
    fill(&mut bytes).map_err(std::io::Error::other)?;
    Ok(bytes)
}

fn hex(bytes: &[u8; 32]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut text = String::with_capacity(64);
    for byte in bytes {
        text.push(char::from(DIGITS[usize::from(byte >> 4)]));
        text.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ErrorCode;
    use std::io;

    #[test]
    fn fixed_bytes_keep_leading_zeroes_and_all_former_uuid_bits() -> Result<()> {
        let expected = [
            0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0xff, 0x07, 0xff, 0x09, 0x0a, 0x0b, 0x0c, 0x0d,
            0x0e, 0x0f, 0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x00, 0x17, 0x00, 0x19, 0x1a, 0x1b,
            0x1c, 0x1d, 0x1e, 0x1f,
        ];
        let filled = fill_with(|output| {
            output.copy_from_slice(&expected);
            Ok(())
        })?;
        assert_eq!(filled, expected);
        assert_eq!(
            hex(&filled),
            "000102030405ff07ff090a0b0c0d0e0f101112131415001700191a1b1c1d1e1f"
        );
        Ok(())
    }

    #[test]
    fn failed_fill_returns_safe_io_error_with_the_original_resource_free_cause() {
        let Err(error) = fill_with(|output| {
            output[0] = 0xff;
            Err(getrandom::Error::UNSUPPORTED)
        }) else {
            panic!("partial random output must never be returned after failure");
        };
        assert_eq!(error.code, ErrorCode::IoError);
        assert_eq!(error.message, "I/O operation failed (Other).");
        assert!(!error.file_committed);
        let io_error = crate::error::tests::io_cause(&error);
        assert_eq!(io_error.kind(), io::ErrorKind::Other);
        assert_eq!(
            io_error
                .get_ref()
                .and_then(|cause| cause.downcast_ref::<getrandom::Error>())
                .copied(),
            Some(getrandom::Error::UNSUPPORTED)
        );
    }

    #[test]
    fn generated_secrets_have_the_existing_wire_shape() -> Result<()> {
        let generated = secret()?;
        assert_eq!(generated.len(), 64);
        assert!(
            generated
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        );
        Ok(())
    }
}
