#![forbid(unsafe_code)]

pub mod cli;
pub mod core;
mod hybrid;
mod managed;
mod mcp;
mod repo;
mod root;
mod service;
mod setup;
mod syntax;

#[cfg(feature = "fuzzing")]
#[doc(hidden)]
pub mod fuzz_support {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum PathDisposition {
        Invalid,
        Denied,
        Pruned,
        Allowed,
    }

    #[must_use]
    pub fn redact_bounded(text: &str, max_output_bytes: usize) -> (String, bool) {
        crate::repo::fuzz_redact_bounded(text, max_output_bytes)
    }

    #[must_use]
    pub fn path_disposition(path: &str) -> PathDisposition {
        match crate::repo::fuzz_path_disposition(path) {
            0 => PathDisposition::Invalid,
            1 => PathDisposition::Denied,
            2 => PathDisposition::Pruned,
            _ => PathDisposition::Allowed,
        }
    }

    #[must_use]
    pub fn parse_supported_source(path: &str, text: &str, max_symbols: usize) -> usize {
        crate::syntax::extract_ast_symbols_bounded(path, text, max_symbols, None, None)
            .map_or(0, |symbols| symbols.len())
    }
}
