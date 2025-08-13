//! Helper that owns the debounce/cancellation logic for `@` file searches.
//!
//! `ChatComposer` publishes *every* change of the `@token` as
//! `AppEvent::StartFileSearch(query)`.
//! This struct receives those events and decides when to actually spawn the
//! expensive search (handled in the main `App` thread). It tries to ensure:
//!
//! - Even when the user types long text quickly, they will start seeing results
//!   after a short delay using an early version of what they typed.
//! - At most one search is in-flight at any time.
//!
//! It works as follows:
//!
//! 1. First query starts a debounce timer.
//! 2. While the timer is pending, the latest query from the user is stored.
//! 3. When the timer fires, it is cleared, and a search is done for the most
//!    recent query.
//! 4. If there is a in-flight search that is not a prefix of the latest thing
//!    the user typed, it is cancelled.

use codex_file_search as file_search;
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::thread;
use std::time::Duration;

use crate::app_event::AppEvent;
use crate::app_event_sender::AppEventSender;
use regex_lite::Regex;
use std::fs;
use std::path::Path;

#[allow(clippy::unwrap_used)]
const MAX_FILE_SEARCH_RESULTS: NonZeroUsize = NonZeroUsize::new(8).unwrap();

#[allow(clippy::unwrap_used)]
const NUM_FILE_SEARCH_THREADS: NonZeroUsize = NonZeroUsize::new(2).unwrap();

/// How long to wait after a keystroke before firing the first search when none
/// is currently running. Keeps early queries more meaningful.
const FILE_SEARCH_DEBOUNCE: Duration = Duration::from_millis(100);

const ACTIVE_SEARCH_COMPLETE_POLL_INTERVAL: Duration = Duration::from_millis(20);

/// State machine for file-search orchestration.
pub(crate) struct FileSearchManager {
    /// Unified state guarded by one mutex.
    state: Arc<Mutex<SearchState>>,

    search_dir: PathBuf,
    app_tx: AppEventSender,
}

struct SearchState {
    /// Latest query typed by user (updated every keystroke).
    latest_query: String,
    /// Whether the latest scheduled search should force glob matching.
    latest_force_glob: bool,

    /// true if a search is currently scheduled.
    is_search_scheduled: bool,

    /// If there is an active search, this will be the query being searched.
    active_search: Option<ActiveSearch>,
}

struct ActiveSearch {
    query: String,
    cancellation_token: Arc<AtomicBool>,
}

impl FileSearchManager {
    pub fn new(search_dir: PathBuf, tx: AppEventSender) -> Self {
        Self {
            state: Arc::new(Mutex::new(SearchState {
                latest_query: String::new(),
                latest_force_glob: false,
                is_search_scheduled: false,
                active_search: None,
            })),
            search_dir,
            app_tx: tx,
        }
    }

    /// Call whenever the user edits the `@` token.
    pub fn on_user_query(&self, query: String) {
        self.on_user_query_with_mode(query, false)
    }

    /// Call to start a search with an explicit mode; when `force_glob` is true
    /// the search will use glob semantics even if the query lacks glob chars.
    pub fn on_user_query_with_mode(&self, query: String, force_glob: bool) {
        {
            #[allow(clippy::unwrap_used)]
            let mut st = self.state.lock().unwrap();
            // Decide whether anything relevant changed: either the query text
            // or the requested mode (e.g. ForceGlob on Tab).
            let query_changed = query != st.latest_query;
            let mode_changed = force_glob != st.latest_force_glob;

            if !(query_changed || mode_changed) {
                // Nothing changed – avoid re-scheduling needlessly.
                return;
            }

            // Update latest query and mode.
            st.latest_query.clear();
            st.latest_query.push_str(&query);
            st.latest_force_glob = force_glob;

            // If there is an in-flight search that is definitely obsolete,
            // cancel it now. We keep an in-flight search if the new query is a
            // prefix of the active query so the results may still be useful; a
            // newly scheduled search will run once the current one completes.
            if let Some(active_search) = &st.active_search {
                if !query.starts_with(&active_search.query) {
                    active_search
                        .cancellation_token
                        .store(true, Ordering::Relaxed);
                    st.active_search = None;
                }
            }

            // Schedule a search to run after debounce if one isn't already queued.
            if !st.is_search_scheduled {
                st.is_search_scheduled = true;
            } else {
                return;
            }
        }

        // If we are here, we set `st.is_search_scheduled = true` before
        // dropping the lock. This means we are the only thread that can spawn a
        // debounce timer.
        let state = self.state.clone();
        let search_dir = self.search_dir.clone();
        let tx_clone = self.app_tx.clone();
        thread::spawn(move || {
            // Always do a minimum debounce, but then poll until the
            // `active_search` is cleared.
            thread::sleep(FILE_SEARCH_DEBOUNCE);
            loop {
                #[allow(clippy::unwrap_used)]
                if state.lock().unwrap().active_search.is_none() {
                    break;
                }
                thread::sleep(ACTIVE_SEARCH_COMPLETE_POLL_INTERVAL);
            }

            // The debounce timer has expired, so start a search using the
            // latest query.
            let cancellation_token = Arc::new(AtomicBool::new(false));
            let token = cancellation_token.clone();
            let (query, force_glob) = {
                #[allow(clippy::unwrap_used)]
                let mut st = state.lock().unwrap();
                let query = st.latest_query.clone();
                let force_glob = st.latest_force_glob;
                st.is_search_scheduled = false;
                st.active_search = Some(ActiveSearch {
                    query: query.clone(),
                    cancellation_token: token,
                });
                (query, force_glob)
            };

            FileSearchManager::spawn_file_search(
                query,
                search_dir,
                tx_clone,
                cancellation_token,
                state,
                force_glob,
            );
        });
    }

    fn spawn_file_search(
        query: String,
        search_dir: PathBuf,
        tx: AppEventSender,
        cancellation_token: Arc<AtomicBool>,
        search_state: Arc<Mutex<SearchState>>,
        force_glob: bool,
    ) {
        let compute_indices = true;
        std::thread::spawn(move || {
            // If the query contains glob characters, perform a glob-style match
            // over the repository tree; otherwise fall back to fuzzy search.
            let is_glob = force_glob || contains_glob_chars(&query);

            let effective_query = if is_glob && !contains_glob_chars(&query) {
                format!("{query}*")
            } else {
                query.clone()
            };

            let matches = if is_glob {
                run_glob_search(&effective_query, &search_dir, &cancellation_token)
            } else {
                file_search::run(
                    &effective_query,
                    MAX_FILE_SEARCH_RESULTS,
                    &search_dir,
                    Vec::new(),
                    NUM_FILE_SEARCH_THREADS,
                    cancellation_token.clone(),
                    compute_indices,
                )
                .map(|res| res.matches)
                .unwrap_or_default()
            };

            let is_cancelled = cancellation_token.load(Ordering::Relaxed);
            if !is_cancelled {
                tx.send(AppEvent::FileSearchResult { query, matches });
            }

            // Reset the active search state. Do a pointer comparison to verify
            // that we are clearing the ActiveSearch that corresponds to the
            // cancellation token we were given.
            {
                #[allow(clippy::unwrap_used)]
                let mut st = search_state.lock().unwrap();
                if let Some(active_search) = &st.active_search {
                    if Arc::ptr_eq(&active_search.cancellation_token, &cancellation_token) {
                        st.active_search = None;
                    }
                }
            }
        });
    }
}

/// Return true if `s` contains shell-style glob characters.
fn contains_glob_chars(s: &str) -> bool {
    s.chars().any(|c| matches!(c, '*' | '?' | '['))
}

/// Very small glob implementation suitable for interactive UI:
/// - `*` matches any sequence of characters except '/'
/// - `**` matches any sequence including '/'
/// - `?` matches any single character except '/'
/// Character classes like `[a-z]` are supported with a simple translation to regex.
/// The match is anchored to the full relative path.
fn glob_to_regex(pattern: &str) -> Option<Regex> {
    let mut regex = String::from("^");
    let mut chars = pattern.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            // Escape regex meta
            '.' | '+' | '(' | ')' | '|' | '^' | '$' | '{' | '}' | '\\' => {
                regex.push('\\');
                regex.push(ch);
            }
            // Character class: copy through until closing ']'
            '[' => {
                regex.push('[');
                for c2 in chars.by_ref() {
                    regex.push(c2);
                    if c2 == ']' {
                        break;
                    }
                }
            }
            '?' => {
                regex.push_str("[^/]");
            }
            '*' => {
                if matches!(chars.peek(), Some('*')) {
                    // Consume the second '*'
                    let _ = chars.next();
                    regex.push_str(".*"); // match across '/'
                } else {
                    regex.push_str("[^/]*"); // single-segment wildcard
                }
            }
            _ => regex.push(ch),
        }
    }
    regex.push('$');
    Regex::new(&regex).ok()
}

/// Walk the tree rooted at `search_dir` and return up to MAX_FILE_SEARCH_RESULTS
/// file matches whose relative path matches the provided glob pattern.
fn run_glob_search(
    pattern: &str,
    search_dir: &PathBuf,
    cancel: &Arc<AtomicBool>,
) -> Vec<file_search::FileMatch> {
    let Some(regex) = glob_to_regex(pattern) else {
        return Vec::new();
    };

    let mut results: Vec<file_search::FileMatch> = Vec::new();

    // Simple DFS traversal without .gitignore handling; fast-exit after N hits.
    fn visit_dir(
        dir: &Path,
        root: &Path,
        regex: &Regex,
        results: &mut Vec<file_search::FileMatch>,
        limit: usize,
        cancel: &AtomicBool,
    ) {
        if cancel.load(Ordering::Relaxed) || results.len() >= limit {
            return;
        }
        let Ok(read) = fs::read_dir(dir) else {
            return;
        };
        for entry_res in read {
            if cancel.load(Ordering::Relaxed) || results.len() >= limit {
                return;
            }
            let Ok(entry) = entry_res else {
                continue;
            };
            let Ok(ft) = entry.file_type() else {
                continue;
            };
            let path = entry.path();
            if ft.is_dir() {
                visit_dir(&path, root, regex, results, limit, cancel);
            } else if ft.is_file() {
                let rel = match path.strip_prefix(root) {
                    Ok(p) => match p.to_str() {
                        Some(s) => s,
                        None => continue,
                    },
                    Err(_) => continue,
                };
                if regex.is_match(rel) {
                    results.push(file_search::FileMatch {
                        score: 0,
                        path: rel.to_string(),
                        indices: None,
                    });
                    if results.len() >= limit {
                        return;
                    }
                }
            }
        }
    }

    visit_dir(
        search_dir,
        search_dir,
        &regex,
        &mut results,
        MAX_FILE_SEARCH_RESULTS.get(),
        cancel,
    );
    results
}
