//! The marker type shared by the scanner, which detects coverage, and the
//! service, which writes markers. One enum keeps the on-disk filenames in a
//! single place, so detection and the write buttons cannot drift apart.

use serde::{Deserialize, Serialize};

/// A marker file a user writes to cover a folder on purpose (see CONTEXT.md).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Marker {
    /// `.no_ebook`: an ebook does not exist or could not be sourced.
    NoEbook,
    /// `.ebook_elsewhere`: the ebook exists but lives in another folder.
    EbookElsewhere,
}

impl Marker {
    /// Both markers, for iterating during detection.
    pub const ALL: [Marker; 2] = [Marker::NoEbook, Marker::EbookElsewhere];

    /// The on-disk filename: the single source of truth for detection and writes.
    pub const fn filename(self) -> &'static str {
        match self {
            Marker::NoEbook => ".no_ebook",
            Marker::EbookElsewhere => ".ebook_elsewhere",
        }
    }

    /// Classify a directory entry name during the scan.
    pub fn from_filename(name: &str) -> Option<Marker> {
        Marker::ALL.into_iter().find(|m| m.filename() == name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filename_round_trips_through_from_filename() {
        for marker in Marker::ALL {
            assert_eq!(Marker::from_filename(marker.filename()), Some(marker));
        }
    }

    #[test]
    fn from_filename_rejects_a_non_marker() {
        assert_eq!(Marker::from_filename("book.epub"), None);
        assert_eq!(Marker::from_filename(".no_ebook_backup"), None);
    }

    #[test]
    fn serde_uses_snake_case_tokens() {
        let parsed: Marker = serde_json::from_value(serde_json::json!("ebook_elsewhere")).unwrap();
        assert_eq!(parsed, Marker::EbookElsewhere);
        let token = serde_json::to_value(Marker::NoEbook).unwrap();
        assert_eq!(token, serde_json::json!("no_ebook"));
    }
}
