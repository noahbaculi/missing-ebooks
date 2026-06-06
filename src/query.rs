//! Build a search-link query from a folder name. The name is cleaned into a
//! readable query here; the caller percent-encodes it before filling a link
//! template. Cleaning is pure and runs at render time (see
//! docs/adr/0010-search-query-cleaned-at-render-time.md).

/// Clean a raw folder name into a search query. Drops bracketed segments,
/// normalizes `_` and `.` to spaces, collapses whitespace, and trims dangling
/// separator punctuation. Returns the raw name when cleaning empties the string.
///
/// Bracket handling is lenient: a single depth counter spans `()`, `[]`, and
/// `{}`, so a stray closer is ignored and an unclosed opener drops the rest of
/// the name. Folder names are rarely malformed, so this is good enough.
#[must_use]
pub fn clean_query(name: &str) -> String {
    // 1. Drop (...), [...], and {...} segments. One depth counter spans all three
    //    bracket kinds, so nested segments fall out together; mismatched kinds are
    //    handled leniently, which is fine for folder names. No regex crate needed.
    let mut depth: usize = 0;
    let mut without_brackets = String::with_capacity(name.len());
    for ch in name.chars() {
        match ch {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth = depth.saturating_sub(1),
            _ if depth == 0 => without_brackets.push(ch),
            _ => {}
        }
    }

    // 2. Normalize `_` and `.` to spaces; folder names are usually space-separated
    //    but not always. The swap is byte-length-preserving, so the input length
    //    pre-sizes the output exactly.
    let mut spaced = String::with_capacity(without_brackets.len());
    spaced.extend(
        without_brackets
            .chars()
            .map(|c| if c == '_' || c == '.' { ' ' } else { c }),
    );

    // 3. Collapse whitespace runs to single spaces, then 4. trim the ends and any
    //    dangling separator punctuation. After step 2 only `-` can still dangle;
    //    `_` and `.` are listed defensively in case the order ever changes.
    let collapsed = spaced.split_whitespace().collect::<Vec<_>>().join(" ");
    let trimmed =
        collapsed.trim_matches(|c: char| c.is_whitespace() || matches!(c, '-' | '_' | '.'));

    // 5. Fall back to the raw name when cleaning empties the string.
    if trimmed.is_empty() {
        name.to_string()
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    #[test]
    fn strips_bracketed_segments_including_nested() {
        assert_eq!(clean_query("Book (Unabridged)"), "Book");
        assert_eq!(clean_query("Book [Disc 1]"), "Book");
        assert_eq!(clean_query("Book {extra}"), "Book");
        assert_eq!(clean_query("Series [Book 1 (Unabridged)]"), "Series");
    }

    #[test]
    fn normalizes_underscores_and_dots_to_spaces() {
        assert_eq!(clean_query("Author_Name.Book"), "Author Name Book");
    }

    #[test]
    fn collapses_whitespace_runs() {
        assert_eq!(clean_query("Author   Title"), "Author Title");
        assert_eq!(clean_query("A..B"), "A B");
    }

    #[test]
    fn trims_dangling_separator_punctuation() {
        assert_eq!(clean_query("- Book -"), "Book");
        assert_eq!(clean_query("Book - "), "Book");
    }

    #[test]
    fn keeps_an_internal_author_prefix() {
        assert_eq!(clean_query("Author - Title"), "Author - Title");
    }

    #[test]
    fn falls_back_to_the_raw_name_when_cleaning_empties_it() {
        assert_eq!(clean_query("(Unabridged)"), "(Unabridged)");
    }

    #[test]
    fn handles_unbalanced_brackets_leniently() {
        // A stray closer is a no-op (the depth counter saturates at zero), so the
        // text around it survives. An unclosed opener drops everything after it.
        assert_eq!(clean_query("Book )extra("), "Book extra");
        assert_eq!(clean_query("Book ("), "Book");
    }

    proptest! {
        /// Cleaning is idempotent: re-cleaning a cleaned query changes nothing.
        #[test]
        fn cleaning_is_idempotent(name in ".*") {
            let once = clean_query(&name);
            let twice = clean_query(&once);
            prop_assert_eq!(twice, once);
        }

        /// A non-empty name yields a non-empty query and an empty name yields an
        /// empty one. The raw-name fallback preserves this in both directions.
        #[test]
        fn cleaning_preserves_emptiness(name in ".*") {
            prop_assert_eq!(clean_query(&name).is_empty(), name.is_empty());
        }

        /// Outside the fallback, the query carries no bracket characters and no
        /// leading or trailing separator. The fallback returns the name verbatim,
        /// so it is the one case allowed to keep them.
        #[test]
        fn cleaning_drops_brackets_and_edge_separators(name in ".*") {
            let cleaned = clean_query(&name);
            prop_assume!(cleaned != name);
            // Pull the check into a local: a `{`/`}` char literal inside
            // `prop_assert!` would be read as a format placeholder.
            let has_bracket = cleaned.contains(['(', ')', '[', ']', '{', '}']);
            prop_assert!(!has_bracket, "kept a bracket: {cleaned:?}");
            let separator = |c: char| c.is_whitespace() || matches!(c, '-' | '_' | '.');
            prop_assert!(!cleaned.starts_with(separator));
            prop_assert!(!cleaned.ends_with(separator));
        }
    }
}
