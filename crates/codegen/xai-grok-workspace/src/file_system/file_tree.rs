//! Generates the file-tree based on how we are using it during training
//! This gives the model an overview of the project and helps it navigate and understand
//! the repository better, cold-starting the exploration

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::time::Instant;

use serde_json::{Value, json};
use std::sync::Arc;
use xai_grok_tools::{
    computer::local::SimplicioRuntimeFs, types::resources::AsyncDirectoryListing,
};

use crate::file_system::FsError;

/// Configuration for limiting file tree traversal to prevent runaway I/O
/// on very large or deeply nested directories.
#[derive(Debug, Clone, Copy)]
pub struct ListContentsLimits {
    /// Maximum number of characters in the output
    pub max_characters: usize,
    /// Maximum depth to traverse (0 = root only)
    pub max_depth: usize,
    /// Maximum number of directories to visit during traversal
    pub max_dirs_visited: usize,
}

impl Default for ListContentsLimits {
    fn default() -> Self {
        Self {
            max_characters: 10_000,
            max_depth: 12,
            max_dirs_visited: 2000,
        }
    }
}

impl ListContentsLimits {
    pub fn new(max_characters: usize, max_depth: usize, max_dirs_visited: usize) -> Self {
        Self {
            max_characters,
            max_depth,
            max_dirs_visited,
        }
    }
}

fn get_top_exts(files: &[String]) -> Vec<(String, usize)> {
    let mut ext_counts: HashMap<String, usize> = HashMap::new();
    for item in files {
        let ext = if let Some(e) = Path::new(item).extension() {
            format!(".{}", e.to_str().unwrap_or("").to_lowercase())
        } else {
            String::new()
        };
        *ext_counts.entry(ext).or_insert(0) += 1;
    }
    let mut vec: Vec<_> = ext_counts.into_iter().collect();
    vec.sort_by_key(|&(_, count)| std::cmp::Reverse(count));
    vec
}

fn get_file_ext_str(files: &[String], k: usize) -> String {
    let top_exts = get_top_exts(files);
    let include_dots = top_exts.len() > k
        || (top_exts.len() == k && top_exts.iter().any(|(ext, _)| ext.is_empty()));
    let top_k_exts = &top_exts[0..std::cmp::min(k, top_exts.len())];
    if top_k_exts.is_empty() {
        return String::new();
    }
    if top_k_exts.len() == 1 && top_k_exts[0].0.is_empty() {
        return "(...)".to_string();
    }
    let filtered_top_k_exts: Vec<_> = top_k_exts
        .iter()
        .filter(|(ext, _)| !ext.is_empty())
        .collect();
    let top_counts = filtered_top_k_exts
        .iter()
        .map(|(ext, cnt)| format!("{} *{}", cnt, ext))
        .collect::<Vec<_>>()
        .join(", ");
    if include_dots {
        format!("({top_counts}, ...)")
    } else if top_counts.is_empty() {
        String::new()
    } else {
        format!("({top_counts})")
    }
}

/// Pre-collected directory contents from a single walk
struct DirContents {
    files: Vec<String>,
    dirs: Vec<String>,
}

/// Performs a single parallel walk and collects all directory contents into a map.
/// Returns a map from directory path -> (files, subdirs) in that directory.
fn collect_all_contents(
    payload: Value,
    root: &Path,
) -> Result<HashMap<PathBuf, DirContents>, FsError> {
    let payload = runtime_payload(payload)?;
    let nodes = payload
        .get("nodes")
        .or_else(|| payload.get("entries"))
        .and_then(Value::as_array)
        .ok_or_else(|| {
            FsError::Other("Runtime list response is missing the v1 `nodes` array".into())
        })?;
    let mut contents_map = HashMap::new();
    contents_map.insert(
        root.to_path_buf(),
        DirContents {
            files: Vec::new(),
            dirs: Vec::new(),
        },
    );

    for node in nodes {
        let relative = node
            .get("path")
            .and_then(Value::as_str)
            .or_else(|| node.get("name").and_then(Value::as_str))
            .ok_or_else(|| {
                FsError::Other("Runtime list entry is missing `path` or `name`".into())
            })?;
        let relative = relative.trim_end_matches('/');
        if relative.is_empty() {
            continue;
        }
        let path = root.join(relative);
        let parent = path.parent().unwrap_or(root).to_path_buf();
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| FsError::Other("Runtime list returned a non-UTF-8 path".into()))?;
        let is_dir = node
            .get("is_dir")
            .and_then(Value::as_bool)
            .or_else(|| {
                node.get("type")
                    .or_else(|| node.get("nodeType"))
                    .and_then(Value::as_str)
                    .map(|kind| kind.eq_ignore_ascii_case("directory"))
            })
            .unwrap_or_else(|| relative.ends_with('/'));
        let parent_contents = contents_map.entry(parent).or_insert_with(|| DirContents {
            files: vec![],
            dirs: vec![],
        });
        if is_dir {
            parent_contents.dirs.push(format!("{name}/"));
            contents_map.entry(path).or_insert_with(|| DirContents {
                files: vec![],
                dirs: vec![],
            });
        } else {
            parent_contents.files.push(name.to_owned());
        }
    }

    // Sort all entries for stable output
    {
        let _timer = (); // instrumentation_timer noop (dev infra)
        for contents in contents_map.values_mut() {
            contents.files.sort_by_cached_key(|n| n.to_lowercase());
            contents.dirs.sort_by_cached_key(|n| n.to_lowercase());
        }
    }

    Ok(contents_map)
}

fn runtime_payload(value: Value) -> Result<Value, FsError> {
    let Some(text) = value
        .get("content")
        .and_then(Value::as_array)
        .and_then(|items| items.iter().find_map(|item| item.get("text")))
        .and_then(Value::as_str)
    else {
        return Ok(value);
    };
    serde_json::from_str(text)
        .map_err(|error| FsError::Other(format!("invalid Runtime list JSON: {error}")))
}

struct DirectoryNode {
    depth: usize,
    path: PathBuf,
    files: Vec<String>,
    dirs: Vec<String>,
    summary_str: String,
    children: Option<HashMap<String, DirectoryNode>>,
    num_listed_files: usize,
}

impl DirectoryNode {
    fn new(path: PathBuf, depth: usize, contents: &HashMap<PathBuf, DirContents>) -> Self {
        let (files, dirs) = contents
            .get(&path)
            .map(|c| (c.files.clone(), c.dirs.clone()))
            .unwrap_or_default();

        let mut node = Self {
            depth,
            path,
            files,
            dirs,
            summary_str: String::new(),
            children: None,
            num_listed_files: 0,
        };
        node.summary_str = node.get_remaining_str(&[], false, 3);
        node
    }

    fn get_remaining_str(
        &self,
        excluded_files: &[String],
        exclude_all_dirs: bool,
        k: usize,
    ) -> String {
        let remaining_files: Vec<String> = self
            .files
            .iter()
            .filter(|f| !excluded_files.contains(*f))
            .cloned()
            .collect();
        let mut file_ext_str = get_file_ext_str(&remaining_files, k);
        if !file_ext_str.is_empty() {
            file_ext_str.push(' ');
        }
        let file_count = remaining_files.len();
        let dir_count = if exclude_all_dirs { 0 } else { self.dirs.len() };
        let indent = "  ".repeat(self.depth + 1);
        format!("{indent}- [+{file_count} files {file_ext_str}& {dir_count} dirs]")
    }

    fn is_expanded(&self) -> bool {
        self.children.is_some()
    }

    fn expand_children(&mut self, contents: &HashMap<PathBuf, DirContents>) {
        if self.is_expanded() {
            return;
        }
        let mut children: HashMap<String, DirectoryNode> = HashMap::new();
        for dir in &self.dirs {
            let child_path = self.path.join(dir.trim_end_matches('/'));
            let dir_node = DirectoryNode::new(child_path, self.depth + 1, contents);
            children.insert(dir.clone(), dir_node);
        }
        self.children = Some(children);
        self.num_listed_files = std::cmp::min(3, self.files.len());
    }

    fn unexpand_children(&mut self) {
        self.children = None;
        self.num_listed_files = 0;
    }

    fn subitem_str(&self, subitem: &str) -> String {
        let indent = "  ".repeat(self.depth + 1);
        format!("{indent}- {subitem}")
    }

    fn get_complete_str(&self) -> String {
        assert!(self.is_expanded());
        let children = self.children.as_ref().unwrap();
        let mut remaining_subitems = self.dirs.clone();
        remaining_subitems.extend(self.files[0..self.num_listed_files].iter().cloned());
        remaining_subitems.sort_by_key(|s| s.to_lowercase());
        let mut curr_str = String::new();
        for subitem in remaining_subitems {
            curr_str.push_str(&self.subitem_str(&subitem));
            curr_str.push('\n');
            if let Some(child) = children.get(&subitem) {
                if child.is_expanded() {
                    curr_str.push_str(&child.get_complete_str());
                    curr_str.push('\n');
                } else {
                    curr_str.push_str(&child.summary_str);
                    curr_str.push('\n');
                }
            }
        }
        if self.files.len() > self.num_listed_files {
            let remaining_str =
                self.get_remaining_str(&self.files[0..self.num_listed_files], true, 3);
            curr_str.push_str(&remaining_str);
        }
        curr_str.trim_end_matches('\n').to_string()
    }
}

/// Creates the project overview
pub async fn list_contents(
    path: impl Into<PathBuf>,
    limits: ListContentsLimits,
) -> Result<String, FsError> {
    let path = path.into();
    list_contents_with_runtime(
        path.clone(),
        limits,
        Arc::new(SimplicioRuntimeFs::new(path)),
    )
    .await
}

/// Runtime-injectable implementation used by focused boundary tests.
pub async fn list_contents_with_runtime(
    path: impl Into<PathBuf>,
    limits: ListContentsLimits,
    runtime: Arc<dyn AsyncDirectoryListing>,
) -> Result<String, FsError> {
    let _timer = (); // instrumentation_timer noop (dev infra)
    let t_total = Instant::now();
    let path: PathBuf = path.into();

    // Use this only for the printed header
    let path_head = {
        let s = path.to_string_lossy().replace('\\', "/");
        if s.ends_with('/') {
            s.to_owned()
        } else {
            format!("{s}/")
        }
    };

    let lim_characters = limits.max_characters + path_head.len();

    // Runtime is the sole traversal authority; it applies the Agent gate
    // before returning the typed recursive listing.
    let payload = runtime
        .list_directory(
            &path,
            json!({
                "depth": limits.max_depth,
                "limit": limits.max_dirs_visited,
                "include_hidden": false,
                "follow_symlinks": false,
                "respect_git_ignore": true,
            }),
        )
        .await
        .map_err(|error| FsError::Other(format!("Simplicio Runtime list denied: {error}")))?;
    let contents = collect_all_contents(payload, &path)?;

    let t_walk = t_total.elapsed();

    let (
        mut root_node,
        mut remaining_chars,
        to_fit_files,
        dirs_visited,
        max_depth_reached,
        depth_limit_hit,
        dirs_limit_hit,
    ) = {
        let _timer = (); // instrumentation_timer noop (dev infra)

        let mut root_node = DirectoryNode::new(path, 0, &contents);

        let min_chars = path_head.len() + root_node.summary_str.len();
        if min_chars > lim_characters {
            return Err(FsError::Other(format!(
                "Minimum possible string is too long for character limit, {} > {}",
                min_chars, lim_characters
            )));
        }

        let mut remaining_chars = lim_characters - min_chars;
        let mut to_fit_files = true;
        let mut dirs_visited: usize = 1; // Count root as visited
        let mut max_depth_reached: usize = 0;
        let mut depth_limit_hit = false;
        let mut dirs_limit_hit = false;
        let mut q: VecDeque<&mut DirectoryNode> = VecDeque::new();
        q.push_back(&mut root_node);
        while let Some(node) = q.pop_front() {
            // Check if we've hit the max directories limit
            if dirs_visited >= limits.max_dirs_visited {
                dirs_limit_hit = true;
                to_fit_files = false;
                break;
            }

            remaining_chars += node.summary_str.len();
            node.expand_children(&contents);
            dirs_visited += node.dirs.len();
            max_depth_reached = max_depth_reached.max(node.depth);

            let test_str = node.get_complete_str();
            let new_additional_len = test_str.replace('\n', "").len();
            if new_additional_len > remaining_chars {
                node.unexpand_children();
                to_fit_files = false;
                break;
            }
            remaining_chars -= new_additional_len;
            if let Some(children_map) = node.children.as_mut() {
                let mut child_values: Vec<&mut DirectoryNode> = children_map.values_mut().collect();
                child_values.sort_by_key(|node| node.path.to_string_lossy().to_lowercase());

                // Filter out children that exceed max_depth
                for child in child_values {
                    if child.depth > limits.max_depth {
                        depth_limit_hit = true;
                        continue;
                    }
                    q.push_back(child);
                }
            }
        }

        (
            root_node,
            remaining_chars,
            to_fit_files,
            dirs_visited,
            max_depth_reached,
            depth_limit_hit,
            dirs_limit_hit,
        )
    };

    {
        let _timer = (); // instrumentation_timer noop (dev infra)
        let mut q: VecDeque<&mut DirectoryNode> = VecDeque::new();
        if to_fit_files {
            q.push_back(&mut root_node);
        }
        let mut file_done = false;
        while let Some(node) = q.pop_front() {
            if !node.is_expanded() {
                continue;
            }
            let num_file_limit = node.files.len();
            for i in node.num_listed_files..num_file_limit {
                let new_additional_len = node.subitem_str(&node.files[i]).len();
                if new_additional_len <= remaining_chars {
                    node.num_listed_files = i + 1;
                    remaining_chars -= new_additional_len;
                } else {
                    file_done = true;
                    break;
                }
            }
            if file_done {
                break;
            }
            if let Some(children_map) = node.children.as_mut() {
                let mut child_values: Vec<&mut DirectoryNode> = children_map.values_mut().collect();
                child_values.sort_by_key(|node| node.path.to_string_lossy().to_lowercase());
                q.extend(child_values);
            }
        }
    }

    let output = {
        let _timer = (); // instrumentation_timer noop (dev infra)
        let mut output = format!("{path_head}\n");
        if root_node.is_expanded() {
            output.push_str(&root_node.get_complete_str());
        } else {
            output.push_str(&root_node.summary_str);
        }
        output
    };

    // Log warnings when limits are hit
    if depth_limit_hit {
        tracing::warn!(
            path = %path_head,
            max_depth = limits.max_depth,
            "list_contents: max_depth limit hit, some directories were not traversed"
        );
    }
    if dirs_limit_hit {
        tracing::warn!(
            path = %path_head,
            max_dirs_visited = limits.max_dirs_visited,
            dirs_visited = dirs_visited,
            "list_contents: max_dirs_visited limit hit, traversal stopped early"
        );
    }

    tracing::debug!(
        path = %path_head,
        max_characters = limits.max_characters,
        max_depth = limits.max_depth,
        max_dirs_visited = limits.max_dirs_visited,
        dirs_visited = dirs_visited,
        max_depth_reached = max_depth_reached,
        depth_limit_hit = depth_limit_hit,
        dirs_limit_hit = dirs_limit_hit,
        output_len = output.len(),
        walk_ms = t_walk.as_millis() as u64,
        elapsed_ms = t_total.elapsed().as_millis() as u64,
        "list_contents complete"
    );
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use xai_grok_tools::computer::types::ComputerError;

    struct FakeRuntime {
        calls: Mutex<Vec<(PathBuf, Value)>>,
        payload: Value,
    }

    #[async_trait::async_trait]
    impl AsyncDirectoryListing for FakeRuntime {
        async fn list_directory(
            &self,
            path: &Path,
            options: Value,
        ) -> Result<Value, ComputerError> {
            self.calls.lock().unwrap().push((path.to_owned(), options));
            Ok(self.payload.clone())
        }
    }

    #[tokio::test]
    async fn tree_uses_runtime_recursive_listing() {
        let runtime = Arc::new(FakeRuntime {
            calls: Mutex::new(vec![]),
            payload: json!({"nodes": [
                {"path": "src", "type": "directory"},
                {"path": "src/lib.rs", "type": "file"},
                {"path": "README.md", "type": "file"}
            ]}),
        });
        let output = list_contents_with_runtime(
            "/repo",
            ListContentsLimits::new(10_000, 4, 20),
            runtime.clone(),
        )
        .await
        .unwrap();

        assert!(output.contains("src/"));
        assert!(output.contains("lib.rs"));
        let calls = runtime.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, Path::new("/repo"));
        assert_eq!(calls[0].1["depth"], 4);
        assert_eq!(calls[0].1["limit"], 20);
    }
}
