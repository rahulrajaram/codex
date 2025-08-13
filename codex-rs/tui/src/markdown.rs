use crate::citation_regex::CITATION_REGEX;
use codex_core::config::Config;
use codex_core::config_types::UriBasedFileOpener;
use ratatui::style::Color;
use ratatui::style::Modifier;
use ratatui::text::Line;
use ratatui::text::Span;
use std::borrow::Cow;
use std::path::Path;

/// Convert leading unordered list markers ('-', '*', '+') to '• ' but only when
/// not inside fenced code blocks (``` ... ```). Also skip lines indented with
/// 4 or more spaces to avoid altering indented code blocks. Preserves nesting
/// indentation for list items.
fn replace_unordered_markers_outside_code_fences(src: &str) -> Cow<'_, str> {
    let mut in_fence = false;
    let mut out = String::new();
    let mut changed = false;

    for (i, line) in src.lines().enumerate() {
        let trimmed_start = line.trim_start();
        let leading_spaces = line.len() - trimmed_start.len();

        // Toggle fence state on lines starting with ``` (ignoring leading spaces).
        if trimmed_start.starts_with("```") {
            in_fence = !in_fence;
            if i > 0 {
                out.push('\n');
            }
            out.push_str(line);
            continue;
        }

        if !in_fence && leading_spaces < 4 {
            let tail = trimmed_start;
            if tail.starts_with("- ") || tail.starts_with("* ") || tail.starts_with("+ ") {
                if i > 0 {
                    out.push('\n');
                }
                out.push_str(&" ".repeat(leading_spaces));
                out.push('•');
                out.push(' ');
                out.push_str(&tail[2..]);
                changed = true;
                continue;
            }
        }

        if i > 0 {
            out.push('\n');
        }
        out.push_str(line);
    }

    if changed {
        Cow::Owned(out)
    } else {
        Cow::Borrowed(src)
    }
}

#[allow(dead_code)]
pub(crate) fn append_markdown(
    markdown_source: &str,
    lines: &mut Vec<Line<'static>>,
    config: &Config,
) {
    append_markdown_with_opener_and_cwd(markdown_source, lines, config.file_opener, &config.cwd);
}

#[allow(dead_code)]
fn append_markdown_with_opener_and_cwd(
    markdown_source: &str,
    lines: &mut Vec<Line<'static>>,
    file_opener: UriBasedFileOpener,
    cwd: &Path,
) {
    // Perform citation rewrite *before* feeding the string to the markdown
    // renderer. When `file_opener` is absent we bypass the transformation to
    // avoid unnecessary allocations.
    let processed_markdown = rewrite_file_citations(markdown_source, file_opener, cwd);
    // Swap hyphen-based list markers with bullets outside code fences for readability.
    let bullet_adjusted = replace_unordered_markers_outside_code_fences(&processed_markdown);

    let markdown = tui_markdown::from_str(&bullet_adjusted);

    // `tui_markdown` returns a `ratatui::text::Text` where every `Line` borrows
    // from the input `message` string. Since the `HistoryCell` stores its lines
    // with a `'static` lifetime we must create an **owned** copy of each line
    // so that it is no longer tied to `message`. We do this by cloning the
    // content of every `Span` into an owned `String`.

    for borrowed_line in markdown.lines {
        let mut owned_spans = Vec::with_capacity(borrowed_line.spans.len());

        // Heuristic: treat a line as a heading when all non-empty spans are bold.
        let is_heading_line = {
            let mut has_non_empty = false;
            let mut all_bold = true;
            for s in &borrowed_line.spans {
                if !s.content.trim().is_empty() {
                    has_non_empty = true;
                    // Access Modifier bits from the style to detect bold.
                    if !s.style.add_modifier.contains(Modifier::BOLD) {
                        all_bold = false;
                        break;
                    }
                }
            }
            has_non_empty && all_bold
        };

        for span in &borrowed_line.spans {
            // Create a new owned String for the span's content to break the lifetime link.
            let mut style = span.style;

            // Apply requested colors based on markdown semantics.
            if is_heading_line {
                style.fg = Some(Color::Yellow);
            } else {
                // Bold -> cyan
                if style.add_modifier.contains(Modifier::BOLD) {
                    style.fg = Some(Color::Cyan);
                }
                // Italic -> cream (#FFFDD0)
                if style.add_modifier.contains(Modifier::ITALIC) {
                    style.fg = Some(Color::Rgb(255, 253, 208));
                }
            }

            let owned_span = Span::styled(span.content.to_string(), style);
            owned_spans.push(owned_span);
        }

        // No post-processing needed here; list marker conversion happens pre-render.

        let owned_line: Line<'static> = Line::from(owned_spans).style(borrowed_line.style);
        // Preserve alignment if it was set on the source line.
        let owned_line = match borrowed_line.alignment {
            Some(alignment) => owned_line.alignment(alignment),
            None => owned_line,
        };

        lines.push(owned_line);
    }
}

/// Rewrites file citations in `src` into markdown hyperlinks using the
/// provided `scheme` (`vscode`, `cursor`, etc.). The resulting URI follows the
/// format expected by VS Code-compatible file openers:
///
/// ```text
/// <scheme>://file<ABS_PATH>:<LINE>
/// ```
#[allow(dead_code)]
fn rewrite_file_citations<'a>(
    src: &'a str,
    file_opener: UriBasedFileOpener,
    cwd: &Path,
) -> Cow<'a, str> {
    // Map enum values to the corresponding URI scheme strings.
    let scheme: &str = match file_opener.get_scheme() {
        Some(scheme) => scheme,
        None => return Cow::Borrowed(src),
    };

    CITATION_REGEX.replace_all(src, |caps: &regex_lite::Captures<'_>| {
        let file = &caps[1];
        let start_line = &caps[2];

        // Resolve the path against `cwd` when it is relative.
        let absolute_path = {
            let p = Path::new(file);
            let absolute_path = if p.is_absolute() {
                path_clean::clean(p)
            } else {
                path_clean::clean(cwd.join(p))
            };
            // VS Code expects forward slashes even on Windows because URIs use
            // `/` as the path separator.
            absolute_path.to_string_lossy().replace('\\', "/")
        };

        // Render as a normal markdown link so the downstream renderer emits
        // the hyperlink escape sequence (when supported by the terminal).
        //
        // In practice, sometimes multiple citations for the same file, but with a
        // different line number, are shown sequentially, so we:
        // - include the line number in the label to disambiguate them
        // - add a space after the link to make it easier to read
        format!("[{file}:{start_line}]({scheme}://file{absolute_path}:{start_line}) ")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn citation_is_rewritten_with_absolute_path() {
        let markdown = "See 【F:/src/main.rs†L42-L50】 for details.";
        let cwd = Path::new("/workspace");
        let result = rewrite_file_citations(markdown, UriBasedFileOpener::VsCode, cwd);

        assert_eq!(
            "See [/src/main.rs:42](vscode://file/src/main.rs:42)  for details.",
            result
        );
    }

    #[test]
    fn citation_is_rewritten_with_relative_path() {
        let markdown = "Refer to 【F:lib/mod.rs†L5】 here.";
        let cwd = Path::new("/home/user/project");
        let result = rewrite_file_citations(markdown, UriBasedFileOpener::Windsurf, cwd);

        assert_eq!(
            "Refer to [lib/mod.rs:5](windsurf://file/home/user/project/lib/mod.rs:5)  here.",
            result
        );
    }

    #[test]
    fn citation_followed_by_space_so_they_do_not_run_together() {
        let markdown = "References on lines 【F:src/foo.rs†L24】【F:src/foo.rs†L42】";
        let cwd = Path::new("/home/user/project");
        let result = rewrite_file_citations(markdown, UriBasedFileOpener::VsCode, cwd);

        assert_eq!(
            "References on lines [src/foo.rs:24](vscode://file/home/user/project/src/foo.rs:24) [src/foo.rs:42](vscode://file/home/user/project/src/foo.rs:42) ",
            result
        );
    }

    #[test]
    fn citation_unchanged_without_file_opener() {
        let markdown = "Look at 【F:file.rs†L1】.";
        let cwd = Path::new("/");
        let unchanged = rewrite_file_citations(markdown, UriBasedFileOpener::VsCode, cwd);
        // The helper itself always rewrites – this test validates behaviour of
        // append_markdown when `file_opener` is None.
        let mut out = Vec::new();
        append_markdown_with_opener_and_cwd(markdown, &mut out, UriBasedFileOpener::None, cwd);
        // Convert lines back to string for comparison.
        let rendered: String = out
            .iter()
            .flat_map(|l| l.spans.iter())
            .map(|s| s.content.clone())
            .collect::<Vec<_>>()
            .join("");
        assert_eq!(markdown, rendered);
        // Ensure helper rewrites.
        assert_ne!(markdown, unchanged);
    }

    #[test]
    fn bullets_replace_unordered_markers_outside_fences() {
        let src = "- one\n  - two\n    - three";
        let out = replace_unordered_markers_outside_code_fences(src);
        assert_eq!(out, "• one\n  • two\n    - three");
    }

    #[test]
    fn bullets_not_replaced_inside_fences() {
        let src = "```\n- not a list\n```\n- real list";
        let out = replace_unordered_markers_outside_code_fences(src);
        assert_eq!(out, "```\n- not a list\n```\n• real list");
    }

    #[test]
    fn bullets_not_replaced_in_indented_code_blocks() {
        let src = "    - literal hyphen in code\n- real item";
        let out = replace_unordered_markers_outside_code_fences(src);
        assert_eq!(out, "    - literal hyphen in code\n• real item");
    }
}
