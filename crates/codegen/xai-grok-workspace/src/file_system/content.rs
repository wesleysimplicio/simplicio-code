//! Runtime-backed workspace content search.
//!
//! Search is intentionally collected by Runtime and then projected into the
//! workspace's historical streaming response shape.  Code never starts `rg`
//! or walks project files itself.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use xai_grok_tools::computer::local::SimplicioRuntimeFs;
use xai_grok_tools::computer::types::AsyncSearch;

pub use xai_grok_workspace_types::rpc::search::{
    ContentMatch, ContentMatchFile, ContentSearchData,
};

#[derive(Debug, Clone, Default)]
pub struct ContentSearchParams {
    pub pattern: String,
    pub case_insensitive: bool,
    pub literal: bool,
    pub globs: Vec<String>,
    pub max_files: Option<usize>,
    pub max_matches: Option<usize>,
    pub respect_gitignore: bool,
}

#[derive(Debug, Clone, Default)]
pub struct ContentSearchBatch {
    pub files: Vec<ContentMatchFile>,
    pub total_matches: usize,
    pub total_files: usize,
    pub done: bool,
    pub truncated: bool,
}

const DEFAULT_MAX_FILES: usize = 100;
const DEFAULT_MAX_MATCHES: usize = 1000;

pub async fn content_search_streaming<F>(
    root: &Path,
    params: &ContentSearchParams,
    cancel: Arc<AtomicBool>,
    on_status: F,
) -> anyhow::Result<ContentSearchData>
where
    F: Fn(ContentSearchBatch) + Send + 'static,
{
    if cancel.load(Ordering::Acquire) {
        return Ok(ContentSearchData::default());
    }
    let max_files = params.max_files.unwrap_or(DEFAULT_MAX_FILES);
    let max_matches = params.max_matches.unwrap_or(DEFAULT_MAX_MATCHES);
    let outcome = SimplicioRuntimeFs::new(root)
        .search(
            &params.pattern,
            None,
            &params.globs,
            params.case_insensitive,
            params.literal,
            max_files,
            max_matches,
        )
        .await
        .map_err(|error| anyhow::anyhow!("Simplicio Runtime search denied: {error}"))?;

    // Cancellation after the Runtime response suppresses publication. Runtime
    // owns in-flight cancellation/reconciliation; Code never retries an
    // ambiguous operation or falls back to a local search.
    if cancel.load(Ordering::Acquire) {
        return Ok(ContentSearchData::default());
    }
    let mut grouped: BTreeMap<String, Vec<ContentMatch>> = BTreeMap::new();
    for item in outcome.matches.into_iter().take(max_matches) {
        grouped.entry(item.path).or_default().push(ContentMatch {
            line: usize::try_from(item.line).unwrap_or(usize::MAX),
            content: item.text,
            match_start: None,
            match_end: None,
        });
    }
    let files: Vec<_> = grouped
        .into_iter()
        .take(max_files)
        .map(|(path, matches)| {
            let mut file = ContentMatchFile::new(path);
            file.matches = matches;
            file
        })
        .collect();
    let total_matches = files.iter().map(|file| file.matches.len()).sum();
    let total_files = files.len();
    let truncated = outcome.truncated;
    on_status(ContentSearchBatch {
        files: files.clone(),
        total_matches,
        total_files,
        done: true,
        truncated,
    });
    Ok(ContentSearchData {
        files,
        total_matches,
        total_files,
        truncated,
    })
}
