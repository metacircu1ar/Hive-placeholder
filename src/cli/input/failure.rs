//! Selected local paths never enter Error metadata, sources, JSON or wire serializers.
use crate::error::Error;
use std::{fmt, path::PathBuf};

pub(in crate::cli) struct Failure {
    pub error: Error,
    pub path: Option<PathBuf>,
    pub exit_code: i32,
}
impl fmt::Debug for Failure {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.error, output)
    }
}
impl From<Error> for Failure {
    fn from(error: Error) -> Self {
        Self {
            error,
            path: None,
            exit_code: super::super::EXIT_ERROR,
        }
    }
}
impl Failure {
    pub(in crate::cli) fn with_source(
        mut self,
        cause: impl std::error::Error + Send + Sync + 'static,
    ) -> Self {
        self.error = self.error.with_source(cause);
        self
    }

    pub(in crate::cli) fn path_line(&self) -> Option<String> {
        self.path.as_deref().map(Self::render_path)
    }

    #[expect(
        clippy::unnecessary_debug_formatting,
        reason = "Debug escapes terminal controls and non-UTF8 bytes; Display would emit raw \
            controls."
    )]
    pub(in crate::cli) fn render_path(selected: &std::path::Path) -> String {
        // OsStr Debug escapes controls, backslashes and non-UTF8 bytes. Bound the
        // local diagnostic on a UTF-8 boundary; never print raw path bytes.
        const LIMIT: usize = 200;
        let mut path = format!("{:?}", selected.as_os_str());
        if path.len() > LIMIT {
            let mut end = LIMIT - 3;
            while !path.is_char_boundary(end) {
                end -= 1;
            }
            path.truncate(end);
            path.push_str("...");
        }
        format!("Path: {path}\n")
    }
}
