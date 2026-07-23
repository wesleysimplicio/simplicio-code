use std::{
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{RecvError, RecvTimeoutError, SyncSender, sync_channel},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use nucleo::{
    Match, Matcher, Nucleo, Snapshot, Utf32String,
    pattern::{CaseMatching, MultiPattern, Normalization, Pattern},
};
use serde_json::{Value, json};
use xai_grok_tools::{
    computer::local::SimplicioRuntimeFs, types::resources::AsyncDirectoryListing,
};

const NUM_NUCLEO_THREADS: usize = 2;

#[derive(Debug, Clone, Default)]
pub struct FuzzyMatchResult {
    // Path of the matched entry.
    pub path: Utf32String,
    /// Matcher score, higher is better.
    pub score: u32,
    /// Matched indices of characters.
    pub indices: Vec<u32>,
    /// Is it a directory.
    pub is_dir: bool,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct FuzzyMatcherStatus {
    pub changed: bool,
    pub done: bool,
}

#[derive(Debug, Clone)]
struct MatchEntry {
    pub is_dir: bool,
}

/// A very fast fuzzy matcher that does ignore-walking. Both happen in background threads.
pub struct FuzzyFileMatcher {
    root: PathBuf,
    query: String,
    nucleo: Nucleo<MatchEntry>,
    matcher: Matcher,
    walk_handle: Option<JoinHandle<()>>,
    cancel: Arc<AtomicBool>,
    dirs: bool,
    runtime: Arc<dyn AsyncDirectoryListing>,
}

impl FuzzyFileMatcher {
    /// Create a new matcher with default config focused on matching paths.
    pub fn new(root: &Path) -> Self {
        Self::with_runtime(root, Arc::new(SimplicioRuntimeFs::new(root.to_owned())))
    }

    /// Creates a matcher with an injectable Runtime listing backend.
    pub fn with_runtime(root: &Path, runtime: Arc<dyn AsyncDirectoryListing>) -> Self {
        let matcher_config = nucleo::Config::DEFAULT.match_paths();
        // matcher_config.prefer_prefix = true; // yes or no? nucleo docs lean towards no

        let mut nucleo = Nucleo::new(
            matcher_config.clone(),
            Arc::new(move || ()),
            Some(NUM_NUCLEO_THREADS),
            1,
        );
        nucleo.pattern = MultiPattern::new(1);

        Self {
            root: root.to_owned(),
            nucleo,
            matcher: Matcher::new(matcher_config),
            walk_handle: None,
            cancel: Arc::new(AtomicBool::new(false)),
            query: String::new(),
            dirs: false,
            runtime,
        }
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    /// Start a new walk and restart nucleo matcher.
    fn restart_walk_with_hidden(&mut self, hidden: bool) {
        // first, wait for previous walker to finish if it's up
        self.cancel.store(true, Ordering::Relaxed);
        if let Some(walk_handle) = self.walk_handle.take() {
            walk_handle.join().unwrap();
        }

        // disconnect all injectors and clear snapshots and streams
        self.nucleo.restart(true);

        // we're back in business
        self.cancel.store(false, Ordering::Relaxed);

        let injector = self.nucleo.injector();
        let root = self.root.clone();
        let cancel = self.cancel.clone();
        let runtime = self.runtime.clone();

        let walk_handle = thread::spawn(move || {
            let Ok(tokio_runtime) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            else {
                return;
            };
            let request = runtime.list_directory(
                &root,
                json!({
                    "depth": usize::MAX,
                    "include_hidden": hidden,
                    "follow_symlinks": false,
                    "respect_git_ignore": !hidden,
                }),
            );
            let payload = tokio_runtime.block_on(async {
                tokio::pin!(request);
                loop {
                    tokio::select! {
                        result = &mut request => break result.ok(),
                        _ = tokio::time::sleep(Duration::from_millis(10)) => {
                            if cancel.load(Ordering::Relaxed) { break None; }
                        }
                    }
                }
            });
            let Some(payload) = payload.and_then(|value| runtime_payload(value).ok()) else {
                return;
            };
            let Some(nodes) = payload
                .get("nodes")
                .or_else(|| payload.get("entries"))
                .and_then(Value::as_array)
            else {
                return;
            };
            for node in nodes {
                if cancel.load(Ordering::Relaxed) {
                    return;
                }
                let Some(path) = node
                    .get("path")
                    .and_then(Value::as_str)
                    .or_else(|| node.get("name").and_then(Value::as_str))
                else {
                    continue;
                };
                let path = path.trim_end_matches('/');
                if path.is_empty() {
                    continue;
                }
                let is_dir = node
                    .get("is_dir")
                    .and_then(Value::as_bool)
                    .or_else(|| {
                        node.get("type")
                            .or_else(|| node.get("nodeType"))
                            .and_then(Value::as_str)
                            .map(|kind| kind.eq_ignore_ascii_case("directory"))
                    })
                    .unwrap_or(false);
                injector.push(MatchEntry { is_dir }, |_entry, columns| {
                    columns[0] = path.into()
                });
            }
        });
        self.walk_handle = Some(walk_handle);

        self.nucleo.tick(0);
    }

    /// Restart the walk with default walker parameters.
    pub fn restart_walk(&mut self) {
        self.restart_walk_with_hidden(false);
    }

    /// Set the query to a given string and trigger reparse.
    ///
    /// It will be faster if the current query is a strict prefix of the new query.
    pub fn set_query(&mut self, mut query: &str, dirs: bool) {
        self.dirs = dirs;
        if dirs && query.ends_with('/') {
            query = &query[..query.len() - 1];
        }
        if query == self.query {
            return;
        }
        // see this re: backslash etc: https://github.com/helix-editor/nucleo/pull/87
        let append = query.as_bytes().starts_with(self.query.as_bytes())
            && !query.ends_with('\\')
            && !query
                .as_bytes()
                .last()
                .is_some_and(|ch| ch.is_ascii_whitespace());
        self.nucleo
            .pattern
            .reparse(0, query, CaseMatching::Smart, Normalization::Smart, append);
        self.nucleo.tick(0);
        self.query = query.to_owned();
    }

    /// Sends a tick to nucleo matcher. Can be safely called at any frequency.
    pub fn tick(&mut self, tick_timeout_ms: u64) -> FuzzyMatcherStatus {
        let status = self.nucleo.tick(tick_timeout_ms);
        let done = self.nucleo.active_injectors() == 0 && !status.running;
        FuzzyMatcherStatus {
            done,
            changed: status.changed,
        }
    }

    /// Total number of currently matched items in the snapshot.
    pub fn num_items(&self) -> usize {
        if self.query.is_empty() {
            let snapshot = self.nucleo.snapshot();
            snapshot
                .matches()
                .iter()
                .filter(|matched| {
                    let item = unsafe { snapshot.get_item_unchecked(matched.idx) };
                    !item.matcher_columns[0].slice(..).to_string().contains('/')
                })
                .count()
        } else {
            self.nucleo.snapshot().item_count() as _
        }
    }

    /// Get top `k` items from the snapshot and sort them by score, path length and path.
    pub fn get_top_k(&mut self, k: usize) -> Vec<FuzzyMatchResult> {
        // note: &mut only because we access self.matcher which has internal allocations

        // rust is a bit dumb at times, we'll need this for sorting without cloning
        fn sort_by_key_hrtb<T, F, K, Q>(slice: &mut [T], f: F)
        where
            F: for<'a> Fn(&'a T) -> (Q, &'a K),
            K: Ord,
            Q: Ord,
        {
            slice.sort_by(|a, b| f(a).cmp(&f(b)))
        }

        // special case: if query is empty, return top items only
        if self.query.is_empty() {
            let snapshot = self.nucleo.snapshot();
            let mut entries = snapshot
                .matches()
                .iter()
                .filter_map(|matched| {
                    let item = unsafe { snapshot.get_item_unchecked(matched.idx) };
                    let path = item.matcher_columns[0].clone();
                    (!path.slice(..).to_string().contains('/') && (!self.dirs || item.data.is_dir))
                        .then_some(FuzzyMatchResult {
                            path,
                            score: 0,
                            indices: vec![],
                            is_dir: item.data.is_dir,
                        })
                })
                .collect::<Vec<_>>();
            entries.sort_by(|a, b| a.path.cmp(&b.path));
            entries.truncate(k);
            return entries;
        }

        // https://github.com/helix-editor/helix/blob/d79cce4e4bfc24dd204f1b294c899ed73f7e9453/helix-term/src/ui/completion.rs#L369
        // suggested min score = 7 * len + 14
        let len = self.query.chars().count() as u32;
        let min_score = 7 + len * 14;

        let mut items = Vec::with_capacity(k);
        let pattern = self.nucleo.pattern.column_pattern(0);
        let snapshot = self.nucleo.snapshot();
        let mut iter = snapshot.matches().iter().peekable();

        while items.len() < k
            && let Some(m) = iter.next()
            // for empty queries, return everything; otherwise, apply heuristic min-score limit
            && (self.query.is_empty() ||  m.score >= min_score)
        {
            fn extract_match(
                m: &Match,
                snapshot: &Snapshot<MatchEntry>,
                pattern: &Pattern,
                matcher: &mut Matcher,
                dirs_only: bool,
            ) -> Option<FuzzyMatchResult> {
                let item = unsafe { snapshot.get_item_unchecked(m.idx) };
                // dirs_only=true means only directories; dirs_only=false means both files and directories
                if dirs_only && !item.data.is_dir {
                    return None;
                }
                let path = item.matcher_columns[0].clone();
                let mut indices = Vec::new();
                if !pattern.atoms.is_empty() {
                    pattern.indices(path.slice(..), matcher, &mut indices);
                }
                Some(FuzzyMatchResult {
                    path,
                    score: m.score,
                    indices,
                    is_dir: item.data.is_dir,
                })
            }

            if !pattern.atoms.is_empty() {
                let start = items.len();
                items.extend(extract_match(
                    m,
                    snapshot,
                    pattern,
                    &mut self.matcher,
                    self.dirs,
                ));
                while iter.peek().is_some_and(|p| p.score == m.score) {
                    let m = iter.next().unwrap();
                    items.extend(extract_match(
                        m,
                        snapshot,
                        pattern,
                        &mut self.matcher,
                        self.dirs,
                    ));
                }
                sort_by_key_hrtb(&mut items[start..], |m| (m.path.len(), &m.path));
            } else {
                items.extend(extract_match(
                    m,
                    snapshot,
                    pattern,
                    &mut self.matcher,
                    self.dirs,
                ));
            }
        }

        if items.len() > k {
            items.truncate(k);
        }

        if pattern.atoms.is_empty() {
            sort_by_key_hrtb(&mut items, |m| (true, &m.path));
        }

        items
    }
}

fn runtime_payload(value: Value) -> Result<Value, serde_json::Error> {
    let Some(text) = value
        .get("content")
        .and_then(Value::as_array)
        .and_then(|items| items.iter().find_map(|item| item.get("text")))
        .and_then(Value::as_str)
    else {
        return Ok(value);
    };
    serde_json::from_str(text)
}

impl Drop for FuzzyFileMatcher {
    fn drop(&mut self) {
        // note: walker threads *may* get detached for a little while but hopefully not for too long
        self.cancel.store(true, Ordering::Relaxed);
    }
}

#[derive(Debug, Clone, Default)]
pub struct FuzzyMatcherDaemonResults {
    pub topk: Arc<[FuzzyMatchResult]>,
    pub num_items: usize,
    pub status: FuzzyMatcherStatus,
    pub generation: usize,
}

impl AsRef<[FuzzyMatchResult]> for FuzzyMatcherDaemonResults {
    fn as_ref(&self) -> &[FuzzyMatchResult] {
        self.topk.as_ref()
    }
}

#[derive(Debug, Clone)]
enum FuzzyMatcherDaemonMessage {
    RestartWalk { hidden: bool },
    SetQuery { query: String, dirs: bool },
    Stop,
}

pub struct FuzzyFileMatcherDaemon {
    results: Arc<Mutex<FuzzyMatcherDaemonResults>>,
    tx: SyncSender<FuzzyMatcherDaemonMessage>,
    _handle: JoinHandle<()>,
}

impl FuzzyFileMatcherDaemon {
    pub fn new(mut matcher: FuzzyFileMatcher, topk: usize) -> Self {
        let results = Arc::new(Mutex::new(FuzzyMatcherDaemonResults::default()));
        let (tx, rx) = sync_channel(1024);

        let res = results.clone();
        let handle = thread::spawn(move || {
            let results = res;
            let mut done = false;
            let mut generation = 0;
            loop {
                let msg = if !done {
                    rx.recv_timeout(Duration::from_micros(250))
                } else {
                    rx.recv().map_err(|e| match e {
                        RecvError => RecvTimeoutError::Disconnected,
                    })
                };
                match msg {
                    Ok(FuzzyMatcherDaemonMessage::RestartWalk { hidden }) => {
                        if !hidden {
                            tracing::trace!("restarting normal walk");
                            matcher.restart_walk();
                        } else {
                            tracing::trace!("restarting hidden walk");
                            matcher.restart_walk_with_hidden(true);
                        }
                        generation += 1;
                        *results.lock().unwrap() = FuzzyMatcherDaemonResults::default();
                        done = false;
                    }
                    Ok(FuzzyMatcherDaemonMessage::SetQuery { query, dirs }) => {
                        matcher.set_query(&query, dirs);
                        generation += 1;
                        done = false;
                    }
                    Ok(FuzzyMatcherDaemonMessage::Stop) | Err(RecvTimeoutError::Disconnected) => {
                        break;
                    }
                    Err(RecvTimeoutError::Timeout) => {
                        if !done {
                            let status = matcher.tick(10);
                            done = status.done;
                            let num_items = matcher.num_items();
                            let topk: Arc<[_]> = matcher.get_top_k(topk).into();
                            *results.lock().unwrap() = FuzzyMatcherDaemonResults {
                                topk,
                                num_items,
                                status,
                                generation,
                            };
                            generation += 1;
                        }
                    }
                }
            }
        });

        Self {
            results,
            tx,
            _handle: handle,
        }
    }

    pub fn get(&self) -> FuzzyMatcherDaemonResults {
        self.results.lock().unwrap().clone()
    }

    pub fn set_query(&self, query: impl AsRef<str>, dirs: bool) {
        let query = query.as_ref().to_owned();
        _ = self
            .tx
            .send(FuzzyMatcherDaemonMessage::SetQuery { query, dirs })
            .ok();
    }

    pub fn restart_walk(&self, hidden: bool) {
        _ = self
            .tx
            .send(FuzzyMatcherDaemonMessage::RestartWalk { hidden })
            .ok();
    }
}

impl Drop for FuzzyFileMatcherDaemon {
    fn drop(&mut self) {
        _ = self.tx.send(FuzzyMatcherDaemonMessage::Stop).ok();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use xai_grok_tools::computer::types::ComputerError;

    struct FakeRuntime;

    #[async_trait::async_trait]
    impl AsyncDirectoryListing for FakeRuntime {
        async fn list_directory(
            &self,
            _path: &Path,
            _options: Value,
        ) -> Result<Value, ComputerError> {
            Ok(json!({"nodes": [
                {"path": "src", "type": "directory"},
                {"path": "src/runtime.rs", "type": "file"},
                {"path": "README.md", "type": "file"}
            ]}))
        }
    }

    #[test]
    fn matcher_indexes_runtime_listing_on_background_thread() {
        let mut matcher = FuzzyFileMatcher::with_runtime(Path::new("/repo"), Arc::new(FakeRuntime));
        matcher.restart_walk();
        matcher.set_query("runtime", false);

        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while !matcher.tick(10).done && std::time::Instant::now() < deadline {}
        let matches = matcher.get_top_k(10);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].path.to_string(), "src/runtime.rs");
    }
}
