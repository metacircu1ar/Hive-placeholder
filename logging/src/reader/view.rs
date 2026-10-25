//! Human-only streaming shares the same bounded reader and pinned descriptors.
use super::{Filter, Reader};
use std::{
    io::{self, Write},
    time::Duration,
};
const POLL: Duration = Duration::from_millis(500);

pub struct View {
    pub tail: usize,
    pub filter: Filter,
    pub follow: bool,
}
impl Reader {
    pub fn view(
        &self,
        options: &View,
        product: &str,
        out: &mut impl Write,
        notices: &mut impl Write,
    ) -> io::Result<()> {
        let mut cursor = None;
        let mut first = true;
        let mut partial = false;
        loop {
            let result = if first {
                self.tail(options.tail, &options.filter)
            } else {
                self.after(cursor.as_ref(), &options.filter)
            };
            let page = match result {
                Ok(page) => page,
                Err(error) if options.follow && error.kind() == io::ErrorKind::WouldBlock => {
                    std::thread::sleep(POLL);
                    continue;
                }
                Err(error) => return Err(error),
            };
            for record in &page.records {
                writeln!(out, "{record}")?;
            }
            out.flush()?;
            report(&page.warnings, partial, product, notices)?;
            partial = page.warnings.incomplete;
            cursor = page.cursor;
            first = false;
            if !options.follow {
                return Ok(());
            }
            if !page.more {
                std::thread::sleep(POLL);
            }
        }
    }
}

fn report(
    page: &super::Warnings,
    partial: bool,
    product: &str,
    notices: &mut impl Write,
) -> io::Result<()> {
    if page.pruned {
        writeln!(
            notices,
            "{product} logs: the previous position was pruned or truncated; some \
                diagnostics may be missing."
        )?;
    }
    if page.limited {
        writeln!(
            notices,
            "{product} logs: tail inspection reached its 8 MiB limit; older matches may be \
                omitted."
        )?;
    }
    if page.incomplete && !partial {
        writeln!(
            notices,
            "{product} logs: an incomplete trailing record is withheld."
        )?;
    }
    if page.corrupt > 0 {
        writeln!(
            notices,
            "{product} logs: skipped {} corrupt or oversized record fragments.",
            page.corrupt
        )?;
    }
    notices.flush()
}
