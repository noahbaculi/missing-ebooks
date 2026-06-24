//! All Maud markup: the page shell, the per-root section, the per-node rows, and
//! the SVG/JS constants those use. Kept separate from the router so the markup is
//! the test surface and `web.rs` stays handlers and glue.

use maud::{Markup, PreEscaped, html};

use crate::config::SearchLink;
use crate::query::clean_query;
use crate::service::{FlaggedView, RootSection};
use crate::tree::Node;
use crate::tree::{RootState, ViewMode};

/// The rotating folder caret used on collapsible rows.
fn chevron() -> Markup {
    html! { (PreEscaped(include_str!("../../assets/svg/chevron.svg"))) }
}

/// The folder glyph used on every node row (every node is a folder).
fn folder_icon() -> Markup {
    html! { (PreEscaped(include_str!("../../assets/svg/folder.svg"))) }
}

/// The root sections in order. Shared by the full page and the htmx rescan
/// response, which swaps just this list into `#roots`.
pub(crate) fn roots(view: &FlaggedView, links: &[SearchLink], mode: ViewMode) -> Markup {
    html! {
        @for (root, section) in view.iter().enumerate() {
            (render_section(section, root, None, links, mode))
        }
    }
}

/// Wrap one rendered section in the OOB-swap fragment the SSE stream uses, so
/// HTMX routes the fragment to its `<section id="root-N-section">` target on
/// the open page (see ADR-0024). Shared by the page-level snapshot helper and
/// the autosync per-root push so the bytes a tab receives via SSE equal the
/// bytes a direct render would produce.
pub fn single_oob_section(
    section: &RootSection,
    root: usize,
    links: &[SearchLink],
    mode: ViewMode,
) -> Markup {
    html! {
        div hx-swap-oob=(format!("outerHTML:#root-{root}-section")) {
            (render_section(section, root, None, links, mode))
        }
    }
}

/// Render every section of `view` as a sequence of OOB swap fragments, suitable
/// for an SSE snapshot payload. Walks the view and delegates each section to
/// `single_oob_section` so the per-section bytes are identical to what the
/// autosync loop pushes one root at a time.
pub fn oob_sections(view: &FlaggedView, links: &[SearchLink], mode: ViewMode) -> Markup {
    html! {
        @for (root, section) in view.iter().enumerate() {
            (single_oob_section(section, root, links, mode))
        }
    }
}

/// The page entry point: assembles the body content (gap summary + roots
/// block) and hands it to `page::page` for shell-wrapping. The single
/// production caller is `web::index` in `src/web.rs`.
pub(crate) fn render_view(view: &FlaggedView, links: &[SearchLink], mode: ViewMode) -> Markup {
    let body = html! {
        (gap_summary(view))
        div.roots-wrap {
            main id="roots" {
                (roots(view, links, mode))
            }
            (super::page::scan_bar())
            (super::page::search_empty())
        }
    };
    super::page::page(mode, body)
}

/// Count the gaps (`needs_ebook()` nodes) anywhere in a forest. Drives the root
/// summary badge so a collapsed root still tells you how much is unresolved.
fn count_gaps(nodes: &[Node]) -> usize {
    nodes
        .iter()
        .map(|n| usize::from(n.needs_ebook()) + count_gaps(&n.children))
        .sum()
}

/// Total gaps across all roots: the sum of `count_gaps` over each forest. `Clean`
/// and `Error` roots contribute nothing. Feeds the summary hero and the session
/// bar's load-time baseline.
fn total_gaps(view: &FlaggedView) -> usize {
    view.iter()
        .map(|section| match &section.state {
            RootState::Forest(nodes) => count_gaps(nodes),
            RootState::Clean | RootState::Error(_) => 0,
        })
        .sum()
}

/// A root's short label: the last non-empty path segment, for the per-root chips.
fn root_label(path: &str) -> &str {
    path.rsplit(['/', '\\'])
        .find(|seg| !seg.is_empty())
        .unwrap_or(path)
}

/// "gap" / "gaps".
fn gap_word(n: usize) -> &'static str {
    if n == 1 { "gap" } else { "gaps" }
}

/// The gap summary strip between the navbar and the roots. Holds the hero gap
/// total on the left, a library coverage readout with its bar on the right,
/// and optional per-root chips for a multi-root setup. `app.js` keeps every
/// number current as marks land, rescans complete, and autosync section pushes
/// arrive. This render is the first paint.
fn gap_summary(view: &FlaggedView) -> Markup {
    let total_gaps = total_gaps(view);
    let total_audiobooks: usize = view.iter().map(|s| s.total_audiobooks).sum();
    let covered = total_audiobooks.saturating_sub(total_gaps);
    // Floor so 199 of 200 reads "99%" rather than rounding up to a false "100%"
    // beside a hero that still says "1 gap to fill". The all-clear branch
    // emits the literal "100%" itself, so the only path that prints `pct`
    // alongside a non-zero gap total is the head, where the floor is honest.
    let pct = if total_audiobooks > 0 {
        ((covered as f64 / total_audiobooks as f64) * 100.0).floor() as usize
    } else {
        0
    };
    let clear_tail_visible = total_audiobooks > 0 && total_gaps == 0;
    html! {
        section.gap-summary id="gap-summary" {
            // Both end-states render; `app.js` toggles `hidden` as the live
            // total crosses zero so the strip converges on what a reload would
            // show, and so an undo back from the last mark can bring the head
            // back. The trailing coverage span is the audiobooks-present
            // variant; a truly empty library keeps the line bare. The two
            // numeric spans inside are the only thing `app.js` rewrites on
            // recompute, so the surrounding "· 100% covered (… of … audiobooks)"
            // wording lives in one place (this template).
            p.gap-summary-clear id="gap-summary-clear" hidden[total_gaps != 0] {
                (PreEscaped(include_str!("../../assets/svg/check.svg")))
                span { "All clear. No gaps in your library." }
                span.coverage-clear id="coverage-clear" hidden[!clear_tail_visible] {
                    " · 100% covered ("
                    span id="coverage-clear-covered" { (total_audiobooks) }
                    " of "
                    span id="coverage-clear-total" { (total_audiobooks) }
                    " audiobooks)"
                }
            }
            div.gap-summary-head id="gap-summary-head" hidden[total_gaps == 0] {
                div.gap-hero {
                    span.gap-hero-num id="gap-total" { (total_gaps) }
                    span.gap-hero-label { (gap_word(total_gaps)) " to fill" }
                }
                (coverage_bar(covered, total_audiobooks, pct))
            }
            @if view.len() > 1 {
                div.gap-chips id="gap-chips" {
                    @for (root, section) in view.iter().enumerate() {
                        (root_chip(root, section))
                    }
                }
            }
        }
    }
}

/// One per-root chip: the root's short label and its own gap count, shown only in
/// a multi-root setup. A covered root reads zero, an error root reads "scan error".
/// The `data-root` hook lets the client update each chip independently.
fn root_chip(root: usize, section: &RootSection) -> Markup {
    html! {
        @match &section.state {
            RootState::Forest(nodes) => {
                @let n = count_gaps(nodes);
                span.gap-chip data-root=(root) {
                    span.gap-chip-name { (root_label(&section.path)) }
                    span.gap-chip-num { (n) }
                }
            }
            RootState::Clean => {
                span.gap-chip.gap-chip-clean data-root=(root) {
                    span.gap-chip-name { (root_label(&section.path)) }
                    span.gap-chip-num { "0" }
                }
            }
            RootState::Error(_) => {
                span.gap-chip.gap-chip-error data-root=(root) {
                    span.gap-chip-name { (root_label(&section.path)) }
                    span.gap-chip-num { "scan error" }
                }
            }
        }
    }
}

/// The library coverage block beside the hero: a readout of covered over total
/// audiobooks (`{pct}% covered · {covered} of {total} audiobooks`) with a
/// `progressbar` so the value is announced. The block always renders, even at
/// a clean load where its head is hidden, because a rescan re-renders only
/// `#roots` and never this strip, so `app.js` reaches in to fill the bar and
/// rewrite the readout when a rescan turns up new gaps.
fn coverage_bar(covered: usize, total: usize, pct: usize) -> Markup {
    html! {
        div.gap-coverage {
            p.gap-coverage-readout {
                span.coverage-num id="coverage-pct" { (pct) } "%"
                " covered · "
                span.coverage-num id="coverage-covered" { (covered) }
                " of "
                span.coverage-num id="coverage-total" { (total) }
                " audiobooks"
            }
            // Floor the max at 1 on a zero-total render so the attribute parses
            // even though the head is hidden in that case. Matches the pattern
            // the old session bar used.
            div.gap-bar role="progressbar"
                aria-label="Library coverage"
                aria-valuemin="0" aria-valuemax=(total.max(1)) aria-valuenow=(covered) {
                span.gap-bar-fill id="coverage-bar-fill"
                    style=(format!("width: {pct}%")) {}
            }
        }
    }
}

/// The badge shown on a root's summary: the gap count, a clean check, or a scan
/// error. In show-all the forest also holds covered nodes, but only gaps are
/// counted.
fn root_badge(state: &RootState) -> Markup {
    html! {
        @match state {
            RootState::Forest(nodes) => {
                @let n = count_gaps(nodes);
                @if n == 0 {
                    span.root-badge.root-badge-clean { "\u{2713} no gaps" }
                } @else if n == 1 {
                    span.root-badge.root-badge-gaps { "1 gap" }
                } @else {
                    span.root-badge.root-badge-gaps { (n) " gaps" }
                }
            }
            RootState::Clean => {
                span.root-badge.root-badge-clean { "\u{2713} no gaps" }
            }
            RootState::Error(_) => {
                span.root-badge.root-badge-error { "scan error" }
            }
        }
    }
}

/// Render one root's section with an optional inline alert. Public so SSE
/// integration tests under `tests/` can compare the byte output against an
/// OOB-wrapped snapshot fragment.
pub fn render_section(
    section: &RootSection,
    root: usize,
    error: Option<&str>,
    links: &[SearchLink],
    mode: ViewMode,
) -> Markup {
    let counter = std::cell::Cell::new(0usize);
    let section_id = format!("root-{root}-section");
    html! {
        section.card.root id=(section_id) data-root=(root)
            data-total-audiobooks=(section.total_audiobooks) {
            details.root-fold open {
                summary.root-head {
                    (chevron())
                    h2 { (section.path) }
                    span.spring {}
                    (root_badge(&section.state))
                }
                @if let Some(message) = error {
                    div.alert.alert-error { (PreEscaped(include_str!("../../assets/svg/error.svg"))) span { (message) } }
                }
                @match &section.state {
                    RootState::Forest(nodes) => {
                        @if nodes.is_empty() {
                            // The normal empty-root path renders as Clean
                            // (the arm below). This arm is reached only for the
                            // loose-root edge case where the walk emits one
                            // entry with rel_path = "", which tree::build
                            // skips because insert_all has no components to descend.
                            div.empty { span { "Nothing here" } }
                        } @else {
                            ul.menu {
                                @for node in nodes { (render_node(node, root, links, mode, &counter, 0)) }
                            }
                        }
                    }
                    RootState::Clean => {
                        div.empty { (PreEscaped(include_str!("../../assets/svg/check.svg"))) span { "No missing ebooks in this root" } }
                    }
                    RootState::Error(message) => {
                        div.alert.alert-error {
                            (PreEscaped(include_str!("../../assets/svg/error.svg"))) span { "Could not scan this root: " (message) }
                        }
                    }
                }
            }
        }
    }
}

/// A standalone error card for a root whose section could not be looked up (its
/// index is out of range), carrying the same alert the in-fold error uses. Used by
/// the failed-write path when the view has no section to render into. The zero
/// `data-total-audiobooks` matches the invariant ADR-0025 documents (errored
/// sections fold out of the JS sum without a special case).
pub(crate) fn error_section(root: usize, message: &str) -> Markup {
    let section_id = format!("root-{root}-section");
    html! {
        section.card.root id=(section_id) data-root=(root) data-total-audiobooks="0" {
            div.alert.alert-error { (PreEscaped(include_str!("../../assets/svg/error.svg"))) span { (message) } }
        }
    }
}

/// The show-all status marker for a row: a success check on a covered folder. Gaps
/// are already flagged by the amber icon and the badge, and plain containers need
/// no marker, so neither gets one. Rendered only in show-all mode.
fn status_icon(node: &Node) -> Markup {
    html! {
        @if !node.missing_ebook {
            span.status title="covered" { (PreEscaped(include_str!("../../assets/svg/check.svg"))) }
        }
    }
}

/// The covering ebook and marker filenames for a covered row, in muted text just
/// after the status check. Show-all only, and empty for gaps and folders covered
/// from above, so nothing renders there.
fn cover_files_span(node: &Node, mode: ViewMode) -> Markup {
    html! {
        @if mode == ViewMode::All && !node.cover_files.is_empty() {
            span.cover-files title="covering files" { (node.cover_files.join(", ")) }
        }
    }
}

/// The structural-smell microlabel for a flagged row. A folder that directly holds
/// audio and also has gap subfolders reads as mixed (a book and a shelf at once). A
/// flagged leaf with no container around it (depth 0, including the root-itself gap)
/// reads as loose. A gap filed under a container at depth 1 or deeper gets nothing.
/// Non-flagged nodes never reach a true branch.
fn smell_label(node: &Node, depth: usize) -> Markup {
    html! {
        @if node.needs_ebook() {
            @if !node.children.is_empty() {
                span.smell.smell-mixed { "holds audio + subfolders" }
            } @else if depth == 0 {
                span.smell.smell-loose { "loose at top" }
            }
        }
    }
}

/// The muted "N files" count for a flagged row. Gated on a gap so it appears only
/// where the file disclosure does, never on a covered or container row.
fn file_count(node: &Node) -> Markup {
    html! {
        @if node.needs_ebook() && !node.audio_files.is_empty() {
            @let n = node.audio_files.len();
            span.file-count {
                @if n == 1 { "1 file" } @else { (n) " files" }
            }
        }
    }
}

/// The audio-file rows for a flagged folder, each a muted, non-actionable line with a
/// music glyph. Emits nothing on a container or covered row, so it is safe to call
/// unconditionally inside the container branch where only a mixed node has files.
fn file_rows(node: &Node) -> Markup {
    html! {
        @if node.needs_ebook() {
            @for name in &node.audio_files {
                li.file-row {
                    (PreEscaped(include_str!("../../assets/svg/music.svg")))
                    span.file-name { (name) }
                }
            }
        }
    }
}

fn render_node(
    node: &Node,
    root: usize,
    links: &[SearchLink],
    mode: ViewMode,
    counter: &std::cell::Cell<usize>,
    depth: usize,
) -> Markup {
    // A covered row dims only in show-all. Gaps-only never holds covered nodes.
    let covered = mode == ViewMode::All && !node.missing_ebook;
    // Buttons and links appear only where there is a gap to act on.
    let act = node.has_gap_within();
    html! {
        @if node.children.is_empty() {
            @if node.needs_ebook() {
                // A flagged leaf: an expandable row whose audio files sit hidden under
                // it until opened. It renders as a `<summary>` like a flagged
                // container, a shape `app.js` already handles (see `rowOf`).
                li {
                    details.node-files {
                        summary.row.flagged {
                            (chevron())
                            (folder_icon())
                            span.name { (node.name) }
                            span.badge.badge-warning title="needs ebook" { "needs ebook" }
                            (smell_label(node, depth))
                            (file_count(node))
                            @if mode == ViewMode::All { (status_icon(node)) }
                            (cover_files_span(node, mode))
                            span.spring {}
                            @if act {
                                (row_actions(root, &node.rel_path, &node.name, links, mode, counter))
                            }
                        }
                        ul.files { (file_rows(node)) }
                    }
                }
            } @else {
                // A non-flagged leaf (a covered or plain folder in show-all) stays a
                // static row.
                li {
                    div.row.covered[covered] {
                        span.leaf-pad {}
                        (folder_icon())
                        span.name { (node.name) }
                        @if mode == ViewMode::All { (status_icon(node)) }
                        (cover_files_span(node, mode))
                        span.spring {}
                    }
                }
            }
        } @else {
            li {
                details open {
                    summary.row
                        .flagged[node.needs_ebook()]
                        .covered[covered]
                        .container-top[!node.needs_ebook() && depth == 0]
                        .container-nested[!node.needs_ebook() && depth > 0] {
                        (chevron())
                        (folder_icon())
                        span.name { (node.name) }
                        @if node.needs_ebook() { span.badge.badge-warning title="needs ebook" { "needs ebook" } }
                        (smell_label(node, depth))
                        (file_count(node))
                        @if mode == ViewMode::All { (status_icon(node)) }
                        (cover_files_span(node, mode))
                        span.spring {}
                        @if act {
                            (row_actions(root, &node.rel_path, &node.name, links, mode, counter))
                        }
                    }
                    ul {
                        (file_rows(node))
                        @for child in &node.children { (render_node(child, root, links, mode, counter, depth + 1)) }
                    }
                }
            }
        }
    }
}

/// The per-row action cluster: a kebab trigger plus the marker buttons and search
/// links, wrapped in a group that doubles as a native popover. On desktop the
/// trigger is hidden and the group is `display: contents`, so its children flow
/// inline in the row. On mobile the kebab opens the group as a bottom action sheet
/// over a dimmed backdrop. The browser provides the toggle, one-open-at-a-time,
/// light-dismiss, and Esc.
fn row_actions(
    root: usize,
    rel: &str,
    name: &str,
    links: &[SearchLink],
    mode: ViewMode,
    counter: &std::cell::Cell<usize>,
) -> Markup {
    let group_id = next_id("acts", root, counter);
    html! {
        button.actions-trigger type="button"
            aria-label="Actions"
            aria-haspopup="menu"
            popovertarget=(group_id)
            onclick="event.stopPropagation()" { (PreEscaped(include_str!("../../assets/svg/kebab.svg"))) }
        div.actions-group id=(group_id) popover="auto" aria-label=(name) {
            div.sheet-header {
                span.sheet-grip aria-hidden="true" {}
                span.sheet-title { (name) }
            }
            (marker_buttons(root, rel, name, mode))
            (search_links(links, name, root, counter))
        }
    }
}

fn marker_buttons(root: usize, rel: &str, name: &str, mode: ViewMode) -> Markup {
    // In gaps-only the marked folder leaves the list, so `app.js` collapses its row
    // and the section swap waits for that to play. In show-all the row stays and
    // flips to covered in place, so the swap is immediate, and the row's reserved
    // min-height keeps the flip from shifting the rows below.
    let swap = match mode {
        ViewMode::GapsOnly => "outerHTML swap:250ms",
        ViewMode::All => "outerHTML",
    };
    html! {
        form.mark.actions hx-target="closest section.root" hx-swap=(swap) {
            input type="hidden" name="root" value=(root);
            input type="hidden" name="rel" value=(rel);
            input type="hidden" name="view" value=(mode.as_query());
            button.btn.btn-outline.btn-xs type="button"
                hx-post="/mark"
                hx-include="closest form"
                hx-vals=(r#"{"kind":"no_ebook"}"#)
                data-confirm-action="No ebook"
                data-confirm-file=".no_ebook"
                data-confirm-folder=(name)
                title="No ebook exists or can be sourced. Covers this folder and everything beneath it."
                onclick="event.stopPropagation()" {
                    span.sheet-icon { (PreEscaped(include_str!("../../assets/svg/no-entry.svg"))) }
                    span.label-long { "No ebook" }
                    span.label-short { "No ebook" }
                }
            button.btn.btn-outline.btn-xs type="button"
                hx-post="/mark"
                hx-include="closest form"
                hx-vals=(r#"{"kind":"ebook_elsewhere"}"#)
                data-confirm-action="Ebook elsewhere"
                data-confirm-file=".ebook_elsewhere"
                data-confirm-folder=(name)
                title="The ebook is in another folder. Covers this folder and everything beneath it."
                onclick="event.stopPropagation()" {
                    span.sheet-icon { (PreEscaped(include_str!("../../assets/svg/ebook-elsewhere.svg"))) }
                    span.label-long { "Ebook elsewhere" }
                    span.label-short { "Elsewhere" }
                }
        }
    }
}

/// Next DOM-safe id with the given prefix for a row's popover or action group.
/// The relative path can hold slashes and spaces, so a root index plus a
/// per-render counter is used instead. The counter is reset per
/// `render_section`, so ids are unique within one render.
fn next_id(prefix: &str, root: usize, counter: &std::cell::Cell<usize>) -> String {
    let n = counter.get();
    counter.set(n + 1);
    format!("{prefix}-{root}-{n}")
}

fn search_links(
    links: &[SearchLink],
    name: &str,
    root: usize,
    counter: &std::cell::Cell<usize>,
) -> Markup {
    html! {
        @if !links.is_empty() {
            @let query = urlencoding::encode(&clean_query(name)).into_owned();
            @let id = next_id("links", root, counter);
            span.links {
                span.sheet-divider { "Search" }
                button.btn.btn-outline.btn-xs.links-toggle type="button"
                    popovertarget=(id)
                    aria-label="Search for this book"
                    title="Search links"
                    onclick="event.stopPropagation()" { (PreEscaped(include_str!("../../assets/svg/search.svg"))) }
                div.links-menu popover="auto" id=(id) onclick="event.stopPropagation()" {
                    @for link in links {
                        a href=(link.url.replace("{query}", &query))
                            target="_blank" rel="noopener noreferrer" {
                                span.sheet-icon { (PreEscaped(include_str!("../../assets/svg/search.svg"))) }
                                (link.label)
                            }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// htmx 2.x parses `hx-swap-oob` by splitting on the *first* colon: the
    /// part before is the swap style, the part after is the CSS selector
    /// (`He` in `htmx.min.js`, confirmed against htmx 2.0.4 source). Any
    /// `hx-swap` modifier with a colon ("transition:true", "swap:200ms", and
    /// friends) inside the OOB attribute would land in the selector portion
    /// and silently break OOB routing: htmx fires `htmx:oobErrorNoTarget` and
    /// drops the swap. Section events would reach the browser but never
    /// update the DOM. The earlier `transition:true` regression
    /// (.scratch/autosync-page-not-updating/issues/01-section-events-arrive-but-dom-does-not-update.md)
    /// was exactly this. Lock the attribute to `<style>:<#id>` with no
    /// whitespace or extra colons so the next person to reach for an OOB
    /// modifier fails this test instead of shipping silent breakage.
    #[test]
    fn single_oob_section_attribute_survives_htmx_first_colon_parse() {
        let section = RootSection {
            path: "/some/root".to_string(),
            state: RootState::Clean,
            total_audiobooks: 0,
        };
        let html = single_oob_section(&section, 3, &[], ViewMode::GapsOnly).into_string();

        let oob_value = extract_attr_value(&html, "hx-swap-oob")
            .expect("rendered fragment carries an hx-swap-oob attribute");

        let (style, selector) = oob_value
            .split_once(':')
            .expect("OOB attribute uses the <style>:<selector> form");

        assert_eq!(style, "outerHTML", "OOB swap style");
        assert_eq!(
            selector, "#root-3-section",
            "OOB selector must be a plain id; htmx splits on the first colon, \
             so any whitespace or extra colon corrupts the selector",
        );
    }

    /// Pull the value of a double-quoted attribute out of a snippet of HTML.
    /// Good enough for fragments produced by maud, which always emits attribute
    /// values inside double quotes.
    fn extract_attr_value<'a>(html: &'a str, name: &str) -> Option<&'a str> {
        let needle = format!("{name}=\"");
        let start = html.find(&needle)? + needle.len();
        let end = html[start..].find('"')? + start;
        Some(&html[start..end])
    }

    #[allow(dead_code)]
    /// A directly-flagged leaf: holds audio, missing an ebook, audio filenames
    /// as given. Cover files empty.
    fn flagged_leaf(name: &str, rel: &str, audio: &[&str]) -> Node {
        Node {
            name: name.into(),
            rel_path: rel.into(),
            directly_holds_audio: true,
            missing_ebook: true,
            children: Vec::new(),
            cover_files: Vec::new(),
            audio_files: audio.iter().map(|s| (*s).into()).collect(),
        }
    }

    #[allow(dead_code)]
    /// A covered leaf: holds audio AND has at least one cover file, so it is
    /// not flagged. Used by all-view tests that pin cover-file rendering.
    fn covered_leaf(name: &str, rel: &str, cover_files: &[&str]) -> Node {
        Node {
            name: name.into(),
            rel_path: rel.into(),
            directly_holds_audio: true,
            missing_ebook: false,
            children: Vec::new(),
            cover_files: cover_files.iter().map(|s| (*s).into()).collect(),
            audio_files: Vec::new(),
        }
    }

    #[allow(dead_code)]
    /// A container row: no audio of its own, holds children.
    fn container(name: &str, rel: &str, children: Vec<Node>) -> Node {
        Node {
            name: name.into(),
            rel_path: rel.into(),
            directly_holds_audio: false,
            missing_ebook: false,
            children,
            cover_files: Vec::new(),
            audio_files: Vec::new(),
        }
    }

    #[allow(dead_code)]
    /// Wrap a forest of root-level nodes into the `RootState::Forest` arm.
    fn forest(roots: Vec<Node>) -> RootState {
        RootState::Forest(roots)
    }

    #[allow(dead_code)]
    /// One library root labeled with its display path, the given state, and
    /// the audiobook count `render_section` emits as `data-total-audiobooks`.
    fn section(path: &str, state: RootState, total: usize) -> RootSection {
        RootSection {
            path: path.into(),
            state,
            total_audiobooks: total,
        }
    }

    #[allow(dead_code)]
    /// A root the scanner walked and found nothing missing in.
    fn clean(path: &str, total: usize) -> RootSection {
        section(path, RootState::Clean, total)
    }

    #[allow(dead_code)]
    /// A root that errored at canonicalization or walk time.
    fn errored(path: &str, message: &str) -> RootSection {
        section(path, RootState::Error(message.into()), 0)
    }

    #[test]
    fn index_renders_a_flagged_folder() {
        let view = vec![section(
            "/lib",
            forest(vec![flagged_leaf("Book", "Book", &["01.mp3"])]),
            1,
        )];
        let html = render_view(&view, &[], ViewMode::GapsOnly).into_string();
        assert!(html.contains("Book"));
    }

    #[test]
    fn index_tags_container_rows_by_depth() {
        // Author (top container) -> Series (nested container) -> Book (flagged leaf).
        let view = vec![section(
            "/lib",
            forest(vec![container(
                "Author",
                "Author",
                vec![container(
                    "Series",
                    "Author/Series",
                    vec![flagged_leaf("Book", "Author/Series/Book", &["01.mp3"])],
                )],
            )]),
            1,
        )];
        let html = render_view(&view, &[], ViewMode::GapsOnly).into_string();
        // The top container is tagged for bold, the nested one for italic.
        assert!(html.contains(r#"class="row container-top""#));
        assert!(html.contains(r#"class="row container-nested""#));
        // The flagged leaf keeps exactly its existing class, with no depth tag.
        assert!(html.contains(r#"class="row flagged""#));
    }

    #[test]
    fn index_leaves_a_deep_gap_unmarked() {
        // A properly filed gap two levels down carries no smell.
        let view = vec![section(
            "/lib",
            forest(vec![container(
                "Author",
                "Author",
                vec![container(
                    "Series",
                    "Author/Series",
                    vec![flagged_leaf("Book", "Author/Series/Book", &["01.mp3"])],
                )],
            )]),
            1,
        )];
        let html = render_view(&view, &[], ViewMode::GapsOnly).into_string();
        assert!(!html.contains("loose at top"));
        assert!(!html.contains("holds audio + subfolders"));
    }

    #[test]
    fn show_all_keeps_depth_tags_on_covered_containers() {
        // Audio under a series with an ebook at the author level covers the
        // whole branch in show-all. The containers stay (no audio of their own,
        // so not flagged), and the book leaf flips from flagged to covered.
        let view = vec![section(
            "/lib",
            forest(vec![container(
                "Author",
                "Author",
                vec![container(
                    "Series",
                    "Author/Series",
                    vec![covered_leaf("Book", "Author/Series/Book", &[])],
                )],
            )]),
            1,
        )];
        let html = render_view(&view, &[], ViewMode::All).into_string();
        // The covered top and nested containers still carry their depth tags, so the
        // depth cue survives the view switch and composes with the covered class.
        assert!(html.contains(r#"class="row covered container-top""#));
        assert!(html.contains(r#"class="row covered container-nested""#));
        // The covered leaf book carries the bare covered class with no depth tag
        // (the trailing quote rules out a `covered container-*` prefix match).
        assert!(html.contains(r#"class="row covered""#));
    }

    #[test]
    fn section_carries_a_data_root_hook() {
        // The toast's Undo targets the section by index, so each section names it.
        let view = section(
            "/lib",
            forest(vec![flagged_leaf("Book", "Book", &["01.mp3"])]),
            1,
        );
        let html = render_section(&view, 0, None, &[], ViewMode::GapsOnly).into_string();
        assert!(html.contains(r#"data-root="0""#));
    }

    #[test]
    fn index_marks_loose_and_mixed_flagged_folders() {
        // A loose gap: a flagged leaf at the top of the root.
        let loose = flagged_leaf("The Hobbit", "The Hobbit", &["01.mp3"]);

        // A mixed gap: a parent that itself holds audio AND has a flagged child.
        let mixed = Node {
            name: "Terry Pratchett".into(),
            rel_path: "Terry Pratchett".into(),
            directly_holds_audio: true,
            missing_ebook: true,
            children: vec![flagged_leaf(
                "Going Postal",
                "Terry Pratchett/Going Postal",
                &["01.mp3"],
            )],
            cover_files: Vec::new(),
            audio_files: vec!["01.mp3".into()],
        };

        let view = vec![section("/lib", forest(vec![loose, mixed]), 3)];
        let html = render_view(&view, &[], ViewMode::GapsOnly).into_string();

        assert!(
            html.contains("loose at top"),
            "the top-level book is marked loose"
        );
        assert!(
            html.contains("holds audio + subfolders"),
            "the half-sorted author is marked mixed"
        );
    }

    #[test]
    fn index_shows_a_file_count_and_a_collapsed_file_list_on_a_flagged_leaf() {
        let view = vec![section(
            "/lib",
            forest(vec![flagged_leaf(
                "Book",
                "Book",
                &["01 - The Gunslinger.mp3"],
            )]),
            1,
        )];
        let html = render_view(&view, &[], ViewMode::GapsOnly).into_string();
        // The count sits on the row, and the file row is present but inside a closed
        // <details> (no `open`), so the names are hidden until the row is expanded.
        assert!(html.contains("1 file"));
        assert!(html.contains(r#"<details class="node-files">"#));
        assert!(html.contains("01 - The Gunslinger.mp3"));
    }

    #[test]
    fn index_pluralizes_the_file_count() {
        let view = vec![section(
            "/lib",
            forest(vec![flagged_leaf(
                "Book",
                "Book",
                &["01.mp3", "02.mp3", "03.mp3"],
            )]),
            1,
        )];
        let html = render_view(&view, &[], ViewMode::GapsOnly).into_string();
        assert!(html.contains("3 files"));
        // The trailing space pins this to the singular row text. A bare
        // "3 file" would also match the plural "3 files", so we anchor on
        // the space that follows on a row that says e.g. "3 file ▸".
        assert!(
            !html.contains("3 file "),
            "rendered singular instead of plural"
        );
    }

    #[test]
    fn mixed_node_shows_its_own_files_above_its_child_gap() {
        // A mixed author: holds its own loose audio file AND has a flagged child.
        let mixed = Node {
            name: "Terry Pratchett".into(),
            rel_path: "Terry Pratchett".into(),
            directly_holds_audio: true,
            missing_ebook: true,
            children: vec![flagged_leaf(
                "Going Postal",
                "Terry Pratchett/Going Postal",
                &["01.mp3"],
            )],
            cover_files: Vec::new(),
            audio_files: vec!["01 - The Colour of Magic.mp3".into()],
        };
        let view = vec![section("/lib", forest(vec![mixed]), 2)];
        let html = render_view(&view, &[], ViewMode::GapsOnly).into_string();
        // The mixed author's own loose file renders as a file row, and the child gap
        // still renders as a folder row carrying its badge.
        assert!(html.contains("01 - The Colour of Magic.mp3"));
        assert!(html.contains(r#"class="file-row""#));
        assert!(html.contains("Going Postal"));
    }

    #[test]
    fn index_wraps_the_sections_in_a_roots_container() {
        let view = vec![section(
            "/lib",
            forest(vec![flagged_leaf("Book", "Book", &["01.mp3"])]),
            1,
        )];
        let html = render_view(&view, &[], ViewMode::GapsOnly).into_string();
        // The root sections live inside a positioned wrapper so the rescan bar
        // can pin above them, and inside #roots so htmx can swap them in place.
        assert!(html.contains(r#"class="roots-wrap""#));
        assert!(html.contains(r#"id="roots""#));
        // The sections themselves are unchanged.
        assert!(html.contains(r#"class="card root""#));
    }

    #[test]
    fn each_root_renders_a_collapsible_summary_with_a_gap_count() {
        let view = vec![section(
            "/lib",
            forest(vec![container(
                "Author",
                "Author",
                vec![flagged_leaf("Book", "Author/Book", &["01.mp3"])],
            )]),
            1,
        )];
        let html = render_view(&view, &[], ViewMode::GapsOnly).into_string();
        // The root head is now a <summary> inside a collapsible <details>.
        assert!(html.contains(r#"class="root-fold""#));
        assert!(html.contains("root-head"));
        // One gap under this root, so the badge reads "1 gap".
        assert!(html.contains("1 gap"));
    }

    #[test]
    fn a_clean_root_badge_reads_no_gaps() {
        let view = vec![clean("/lib", 1)];
        let html = render_view(&view, &[], ViewMode::GapsOnly).into_string();
        assert!(html.contains("no gaps"));
    }

    #[test]
    fn all_view_shows_nothing_here_for_a_root_with_no_folders() {
        // A walked-but-empty root in show-all keeps the `Forest(vec![])` arm
        // so the "Nothing here" branch fires for the loose-root edge case.
        let view = vec![section("/lib", forest(vec![]), 0)];
        let html = render_view(&view, &[], ViewMode::All).into_string();
        assert!(html.contains("Nothing here"));
    }

    #[test]
    fn index_shows_the_clean_message_for_a_covered_root() {
        let view = vec![clean("/lib", 1)];
        let html = render_view(&view, &[], ViewMode::GapsOnly).into_string();
        assert!(html.contains("No missing ebooks in this root"));
    }

    #[test]
    fn section_open_tag_carries_total_audiobooks_data_attr() {
        // Two audiobook folders under a shared parent. The renderer takes
        // the count verbatim from RootSection.total_audiobooks.
        let view = section(
            "/lib",
            forest(vec![container(
                "A",
                "A",
                vec![
                    flagged_leaf("B1", "A/B1", &["01.mp3"]),
                    covered_leaf("B2", "A/B2", &["B2.epub"]),
                ],
            )]),
            2,
        );
        let html = render_section(&view, 0, None, &[], ViewMode::GapsOnly).into_string();
        // Walked root carries the audiobook total on its outer <section>.
        assert!(html.contains(r#"data-total-audiobooks="2""#));
    }

    #[test]
    fn section_open_tag_carries_zero_total_audiobooks_for_errored_root() {
        let view = errored("/no/such/root/xyz123", "no such file or directory");
        let html = render_section(&view, 0, None, &[], ViewMode::GapsOnly).into_string();
        // Errored root still carries the attribute so the JS aggregator
        // never has to special-case missing attrs.
        assert!(html.contains(r#"data-total-audiobooks="0""#));
    }
}
