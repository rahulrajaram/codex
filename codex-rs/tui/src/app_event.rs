use codex_core::protocol::Event;
use codex_file_search::FileMatch;
use crossterm::event::KeyEvent;
use ratatui::text::Line;

use crate::app::ChatWidgetArgs;
use crate::slash_command::SlashCommand;

/// Mode to use for file searching.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FileSearchMode {
    /// Default mode: fuzzy matching across the workspace.
    #[allow(dead_code)]
    Fuzzy,
    /// Force a single-shot glob search (used on initial Tab stroke).
    ForceGlob,
}

#[allow(clippy::large_enum_variant)]
pub(crate) enum AppEvent {
    CodexEvent(Event),

    /// Request a redraw which will be debounced by the [`App`].
    RequestRedraw,

    /// Actually draw the next frame.
    Redraw,

    KeyEvent(KeyEvent),

    /// Text pasted from the terminal clipboard.
    Paste(String),

    /// Request to exit the application gracefully.
    ExitRequest,

    /// Forward an `Op` to the Agent. Using an `AppEvent` for this avoids
    /// bubbling channels through layers of widgets.
    CodexOp(codex_core::protocol::Op),

    /// Latest formatted log line emitted by `tracing`.
    LatestLog(String),

    /// Dispatch a recognized slash command from the UI (composer) to the app
    /// layer so it can be handled centrally.
    DispatchCommand(SlashCommand),

    /// Kick off an asynchronous file search for the given query (text after
    /// the `@`). Previous searches may be cancelled by the app layer so there
    /// is at most one in-flight search.
    StartFileSearch(String),

    /// Same as `StartFileSearch` but with an explicit mode to override
    /// default behavior (e.g., force a glob search when the user hits Tab).
    StartFileSearchWithMode {
        query: String,
        mode: FileSearchMode,
    },

    /// Result of a completed asynchronous file search. The `query` echoes the
    /// original search term so the UI can decide whether the results are
    /// still relevant.
    FileSearchResult {
        query: String,
        matches: Vec<FileMatch>,
    },

    InsertHistory(Vec<Line<'static>>),

    /// Onboarding: result of login_with_chatgpt.
    OnboardingAuthComplete(Result<(), String>),
    OnboardingComplete(ChatWidgetArgs),
}
