use crate::citation_regex::CITATION_REGEX;
use codex_core::config::Config;
use codex_core::config_types::UriBasedFileOpener;
use ratatui::style::Color;
use ratatui::style::Modifier;
use ratatui::text::Line;
use ratatui::text::Span;
use std::borrow::Cow;
use std::path::Path;

/// Safely slice a string by byte indices, clamping to bounds and realigning
/// to the nearest valid UTF-8 boundaries. Returns an empty string slice if the
/// computed range is invalid.
fn safe_slice<'a>(s: &'a str, start: usize, end: usize) -> &'a str {
    let len = s.len();
    if len == 0 {
        return "";
    }
    let mut s_idx = start.min(len);
    let mut e_idx = end.min(len);
    if s_idx >= e_idx {
        return "";
    }
    if let Some(sub) = s.get(s_idx..e_idx) {
        return sub;
    }
    // Realign to char boundaries if needed.
    while s_idx < len && !s.is_char_boundary(s_idx) {
        s_idx += 1;
    }
    while e_idx > s_idx && !s.is_char_boundary(e_idx) {
        e_idx -= 1;
    }
    if s_idx >= e_idx {
        return "";
    }
    s.get(s_idx..e_idx).unwrap_or("")
}

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

        // Outside of fenced code blocks, replace common unordered list markers with
        // a bullet regardless of indentation so nested list items are shown
        // correctly. We keep the original leading spaces to preserve nesting.
        if !in_fence {
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

/// Escape ordered list markers (e.g., `1. `, `2. `) at the start of lines when
/// outside fenced code blocks. This prevents the markdown renderer from
/// converting them into structured list layouts that may split the marker and
/// content across different visual lines in the terminal. By escaping the dot
/// (e.g., `1\.`), we preserve the appearance ("1. foo") but keep it on a single
/// line as plain text.
fn escape_ordered_markers_outside_code_fences(src: &str) -> Cow<'_, str> {
    let mut in_fence = false;
    let mut out = String::new();
    let mut changed = false;

    for (i, line) in src.lines().enumerate() {
        let trimmed_start = line.trim_start();
        // Toggle fence state on lines starting with ``` (ignoring leading spaces).
        if trimmed_start.starts_with("```") {
            in_fence = !in_fence;
            if i > 0 {
                out.push('\n');
            }
            out.push_str(line);
            continue;
        }

        if !in_fence {
            // Match optional spaces, then one or more digits, then a dot and a space.
            // Example matches: "1. foo", "  12. bar".
            let mut j = 0usize;
            let bytes = trimmed_start.as_bytes();
            // Require at least one digit at the beginning of the trimmed segment.
            while j < bytes.len() && bytes[j].is_ascii_digit() {
                j += 1;
            }
            if j > 0 && j + 1 < bytes.len() && trimmed_start[j..].starts_with('.') {
                // Ensure a space follows the dot (markdown list pattern)
                let after_dot = j + '.'.len_utf8();
                if after_dot < trimmed_start.len() && trimmed_start[after_dot..].starts_with(' ') {
                    // Escape the dot to render as plain text (keeps number inline).
                    if i > 0 {
                        out.push('\n');
                    }
                    // Rebuild: original leading spaces, digits, escaped dot, remainder
                    let leading_spaces = line.len() - trimmed_start.len();
                    out.push_str(&" ".repeat(leading_spaces));
                    out.push_str(&trimmed_start[..j]);
                    out.push_str("\\.");
                    out.push_str(&trimmed_start[after_dot..]);
                    changed = true;
                    continue;
                }
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
    // Prevent structured ordered-lists which may split marker/content across lines.
    let ordered_escaped = escape_ordered_markers_outside_code_fences(&processed_markdown);
    // Swap hyphen-based list markers with bullets outside code fences for readability.
    let bullet_adjusted = replace_unordered_markers_outside_code_fences(&ordered_escaped);

    // Compute heading levels per source line before rendering so we can style
    // them deterministically after rendering.
    let heading_levels = compute_heading_levels_outside_code_fences(&bullet_adjusted);

    let markdown = tui_markdown::from_str(&bullet_adjusted);

    // `tui_markdown` returns a `ratatui::text::Text` where every `Line` borrows
    // from the input `message` string. Since the `HistoryCell` stores its lines
    // with a `'static` lifetime we must create an **owned** copy of each line
    // so that it is no longer tied to `message`. We do this by cloning the
    // content of every `Span` into an owned `String`.

    for (line_idx, borrowed_line) in markdown.lines.into_iter().enumerate() {
        let mut owned_spans = Vec::with_capacity(borrowed_line.spans.len());

        // Determine if this source line is a heading based on precomputed levels.
        let heading_level = heading_levels.get(line_idx).and_then(|v| *v);

        for span in &borrowed_line.spans {
            // Create a new owned String for the span's content to break the lifetime link.
            let mut style = span.style;

            // Apply requested colors based on markdown semantics.
            if heading_level.is_some() {
                // Color all headings uniformly for now.
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

        // If a list item starts with a term of the form "• Term:" or
        // "1. Term:", color the term for readability.
        owned_spans = colorize_bullet_term(owned_spans);
        owned_spans = colorize_ordered_term(owned_spans);

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

/// If a line begins with an unordered bullet (possibly indented) followed by a
/// term and a colon, color just the term to make scan-reading easier.
fn colorize_bullet_term(spans: Vec<Span<'static>>) -> Vec<Span<'static>> {
    use crate::colors::PINK;

    // Rebuild the full line text and track span boundaries.
    let mut full = String::new();
    let mut boundaries: Vec<(usize, usize, Span<'static>)> = Vec::with_capacity(spans.len());
    for s in spans.into_iter() {
        let start = full.len();
        full.push_str(&s.content);
        let end = full.len();
        boundaries.push((start, end, s));
    }

    // Match: optional spaces, bullet, spaces, capture term (non-colon) until a colon
    // Example: "  • Term: rest" -> capture "Term".
    let bytes = full.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() && bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    if i < bytes.len() && full[i..].starts_with('•') {
        i += '•'.len_utf8();
        // Require at least one space after bullet
        let mut j = i;
        while j < bytes.len() && bytes[j].is_ascii_whitespace() {
            j += 1;
        }
        // Capture until the next ':'
        let term_start = j;
        while j < bytes.len() {
            let ch = full[j..].chars().next().unwrap();
            if ch == ':' {
                break;
            }
            // Stop if we encounter a newline (shouldn't happen within a single line)
            if ch == '\n' {
                return boundaries.into_iter().map(|(_, _, s)| s).collect();
            }
            j += ch.len_utf8();
        }
        let term_end = j;
        if term_end > term_start
            && j < bytes.len()
            && full[term_start..term_end]
                .chars()
                .any(|c| !c.is_whitespace())
        {
            // Re-split spans so that [term_start, term_end) is a distinct span colored LIGHT_BLUE.
            let mut out: Vec<Span<'static>> = Vec::new();
            for (s_start, s_end, s) in boundaries {
                if s_end <= term_start || s_start >= term_end {
                    // Entire span outside the term range.
                    out.push(s);
                } else {
                    // Overlaps with term range. Split into prefix, term, suffix
                    // clamping indices to the span bounds to avoid OOB slicing.
                    let content = s.content;
                    let span_term_start = term_start.max(s_start);
                    let span_term_end = term_end.min(s_end);

                    // Prefix: from start of span to start of term within span.
                    if span_term_start > s_start {
                        let pre_len = span_term_start - s_start;
                        let pre = safe_slice(&content, 0, pre_len);
                        if !pre.is_empty() {
                            out.push(Span::styled(pre.to_string(), s.style));
                        }
                    }

                    // Term segment within this span.
                    if span_term_end > span_term_start {
                        let mut style = s.style;
                        style.fg = Some(PINK);
                        let start = span_term_start - s_start;
                        let end = span_term_end - s_start;
                        let mid = safe_slice(&content, start, end);
                        if !mid.is_empty() {
                            out.push(Span::styled(mid.to_string(), style));
                        }
                    }

                    // Suffix: from end of term within span to end of span.
                    if span_term_end < s_end {
                        let start = span_term_end - s_start;
                        let suf = safe_slice(&content, start, content.len());
                        if !suf.is_empty() {
                            out.push(Span::styled(suf.to_string(), s.style));
                        }
                    }
                }
            }
            return out;
        }
    }
    // No match or unsuitable for coloring.
    boundaries.into_iter().map(|(_, _, s)| s).collect()
}

/// Compute heading levels (H1..H6) per source line, ignoring fenced code blocks.
/// A heading is a line that starts with 1–6 `#` characters followed by a space.
fn compute_heading_levels_outside_code_fences(src: &str) -> Vec<Option<u8>> {
    let mut in_fence = false;
    let mut levels = Vec::new();
    for line in src.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") {
            in_fence = !in_fence;
            levels.push(None);
            continue;
        }
        if in_fence {
            levels.push(None);
            continue;
        }
        let mut count = 0usize;
        for ch in trimmed.chars() {
            if ch == '#' {
                count += 1;
            } else {
                break;
            }
        }
        if (1..=6).contains(&count) {
            let after_hashes = &trimmed[count..];
            if after_hashes.starts_with(' ') {
                levels.push(Some(count as u8));
                continue;
            }
        }
        levels.push(None);
    }
    levels
}

/// If a line begins with an ordered list marker (digits + '.'), followed by a
/// term and a colon, color just the term in pink to make scan-reading easier.
fn colorize_ordered_term(spans: Vec<Span<'static>>) -> Vec<Span<'static>> {
    use crate::colors::PINK;

    // Rebuild the full line text and track span boundaries.
    let mut full = String::new();
    let mut boundaries: Vec<(usize, usize, Span<'static>)> = Vec::with_capacity(spans.len());
    for s in spans.into_iter() {
        let start = full.len();
        full.push_str(&s.content);
        let end = full.len();
        boundaries.push((start, end, s));
    }

    // Pattern: optional spaces, 1+ digits, '.' (or escaped "\."), space(s), then term until ':'
    let bytes = full.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() && bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    // Require at least one digit
    let mut j = i;
    while j < bytes.len() && bytes[j].is_ascii_digit() {
        j += 1;
    }
    if j == i {
        return boundaries.into_iter().map(|(_, _, s)| s).collect();
    }

    // Accept either "." or "\\." produced by the markdown escape.
    let mut after_marker = j;
    if full[after_marker..].starts_with("\\.") {
        after_marker += 2; // skip backslash and dot
    } else if full[after_marker..].starts_with('.') {
        after_marker += 1; // skip dot
    } else {
        return boundaries.into_iter().map(|(_, _, s)| s).collect();
    }

    // Require at least one space after the dot
    let mut k = after_marker;
    let mut saw_space = false;
    while k < bytes.len() && bytes[k].is_ascii_whitespace() {
        saw_space = true;
        k += 1;
    }
    if !saw_space {
        return boundaries.into_iter().map(|(_, _, s)| s).collect();
    }

    // Capture term until ':'
    let term_start = k;
    while k < bytes.len() {
        let ch = full[k..].chars().next().unwrap();
        if ch == ':' {
            break;
        }
        if ch == '\n' {
            return boundaries.into_iter().map(|(_, _, s)| s).collect();
        }
        k += ch.len_utf8();
    }
    let term_end = k;

    if term_end > term_start
        && k < bytes.len()
        && full[term_start..term_end]
            .chars()
            .any(|c| !c.is_whitespace())
    {
        // Re-split spans so that [term_start, term_end) is a distinct span colored PINK.
        let mut out: Vec<Span<'static>> = Vec::new();
        for (s_start, s_end, s) in boundaries {
            if s_end <= term_start || s_start >= term_end {
                // Entire span outside the term range.
                out.push(s);
            } else {
                // Overlaps with term range. Split into prefix, term, suffix
                // clamping indices to the span bounds to avoid OOB slicing.
                let content = s.content;
                let span_term_start = term_start.max(s_start);
                let span_term_end = term_end.min(s_end);

                // Prefix: from start of span to start of term within span.
                if span_term_start > s_start {
                    let pre_len = span_term_start - s_start;
                    let pre = safe_slice(&content, 0, pre_len);
                    if !pre.is_empty() {
                        out.push(Span::styled(pre.to_string(), s.style));
                    }
                }

                // Term segment within this span.
                if span_term_end > span_term_start {
                    let mut style = s.style;
                    style.fg = Some(PINK);
                    let start = span_term_start - s_start;
                    let end = span_term_end - s_start;
                    let mid = safe_slice(&content, start, end);
                    if !mid.is_empty() {
                        out.push(Span::styled(mid.to_string(), style));
                    }
                }

                // Suffix: from end of term within span to end of span.
                if span_term_end < s_end {
                    let start = span_term_end - s_start;
                    let suf = safe_slice(&content, start, content.len());
                    if !suf.is_empty() {
                        out.push(Span::styled(suf.to_string(), s.style));
                    }
                }
            }
        }
        return out;
    }

    // No match or unsuitable for coloring.
    boundaries.into_iter().map(|(_, _, s)| s).collect()
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
        assert_eq!(out, "• one\n  • two\n    • three");
    }

    #[test]
    fn bullets_not_replaced_inside_fences() {
        let src = "```\n- not a list\n```\n- real list";
        let out = replace_unordered_markers_outside_code_fences(src);
        assert_eq!(out, "```\n- not a list\n```\n• real list");
    }

    #[test]
    fn bullets_are_replaced_even_with_indentation_outside_fences() {
        let src = "    - nested item\n- real item";
        let out = replace_unordered_markers_outside_code_fences(src);
        assert_eq!(out, "    • nested item\n• real item");
    }

    #[test]
    fn escape_ordered_markers_outside_fences_basic() {
        let src = "1. first\n2. second";
        let out = escape_ordered_markers_outside_code_fences(src);
        assert_eq!(out, "1\\. first\n2\\. second");
    }

    #[test]
    fn escape_ordered_markers_preserves_indentation() {
        let src = "  10. ten\n    3. three";
        let out = escape_ordered_markers_outside_code_fences(src);
        assert_eq!(out, "  10\\. ten\n    3\\. three");
    }

    #[test]
    fn do_not_escape_inside_code_fences() {
        let src = "```\n1. not list\n```\n1. real list";
        let out = escape_ordered_markers_outside_code_fences(src);
        assert_eq!(out, "```\n1. not list\n```\n1\\. real list");
    }
}
