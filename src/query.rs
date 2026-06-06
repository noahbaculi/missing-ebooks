//! Build a search-link query from a folder name. The name is cleaned into a
//! readable query here; the caller percent-encodes it before filling a link
//! template. Cleaning is pure and runs at render time (see
//! docs/adr/0010-search-query-cleaned-at-render-time.md).

/// Clean a raw folder name into a search query. Drops bracketed segments,
/// normalizes `_` and `.` to spaces, collapses whitespace, and trims dangling
/// separator punctuation. Returns the raw name when cleaning empties the string.
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
    //    but not always.
    let spaced: String = without_brackets
        .chars()
        .map(|c| if c == '_' || c == '.' { ' ' } else { c })
        .collect();

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
}
