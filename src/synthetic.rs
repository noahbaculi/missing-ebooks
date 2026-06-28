//! In-memory synthetic shapes for the renderer benches. The seeder generates a
//! `Vec<ScannedFolder>` shaped by `total`, `depth`, `fanout`, and `gap_rate`,
//! and a thin wrapper drops the result into a `RootScan::Walked` so the
//! benched functions consume their natural input. No filesystem, no scenarios.

use std::path::PathBuf;

use crate::scanner::{RootScan, ScannedFolder};

/// Build a synthetic `Vec<ScannedFolder>` shaped to the knobs. Only leaf
/// folders carry `directly_holds_audio = true`. Coverage is applied at the
/// leaf per `gap_rate` so the gap-vs-covered ratio is deterministic.
pub fn generate(total: usize, depth: usize, fanout: usize, gap_rate: f64) -> Vec<ScannedFolder> {
    let mut out: Vec<ScannedFolder> = Vec::with_capacity(total);
    if total == 0 || depth == 0 || fanout == 0 {
        return out;
    }
    let mut frontier: Vec<(String, usize)> = vec![(String::new(), 0)];
    let mut emitted = 0usize;
    let gap_stride = if gap_rate > 0.0 {
        (1.0 / gap_rate).round().max(1.0) as usize
    } else {
        usize::MAX
    };
    let mut leaf_index = 0usize;
    while !frontier.is_empty() && emitted < total {
        let mut next: Vec<(String, usize)> = Vec::new();
        for (parent, level) in frontier.drain(..) {
            for child in 0..fanout {
                if emitted >= total {
                    break;
                }
                let name = if level == 0 {
                    format!("Author {child:04}")
                } else {
                    format!("{parent}/Item {child:04}")
                };
                let is_leaf = level + 1 == depth;
                if is_leaf {
                    let is_gap = leaf_index.is_multiple_of(gap_stride);
                    leaf_index += 1;
                    out.push(ScannedFolder {
                        rel_path: PathBuf::from(&name),
                        directly_holds_audio: true,
                        missing_ebook: is_gap,
                        cover_files: if is_gap {
                            std::sync::Arc::from(Vec::<String>::new())
                        } else {
                            std::sync::Arc::from(vec!["Book.epub".to_string()])
                        },
                        audio_files: std::sync::Arc::from(vec![
                            "01.mp3".to_string(),
                            "02.mp3".to_string(),
                        ]),
                    });
                } else {
                    out.push(ScannedFolder {
                        rel_path: PathBuf::from(&name),
                        directly_holds_audio: false,
                        missing_ebook: true,
                        cover_files: std::sync::Arc::from(Vec::<String>::new()),
                        audio_files: std::sync::Arc::from(Vec::<String>::new()),
                    });
                    next.push((name, level + 1));
                }
                emitted += 1;
            }
        }
        frontier = next;
    }
    out
}

/// Wrap `generate(..)` in a `RootScan::Walked` rooted at `/Audiobooks`, the
/// input shape `package_view`, `package_section`, and `render_view` consume.
pub fn synthetic_root_scan(total: usize, depth: usize, fanout: usize, gap_rate: f64) -> RootScan {
    RootScan::Walked {
        canonical_path: PathBuf::from("/Audiobooks"),
        folders: generate(total, depth, fanout, gap_rate),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_respects_total_cap() {
        let folders = generate(100, 4, 10, 0.5);
        assert!(folders.len() <= 100);
    }

    #[test]
    fn generate_emits_only_leaf_audio_at_full_depth() {
        let folders = generate(50, 3, 3, 0.5);
        for f in &folders {
            let components = f.rel_path.components().count();
            if f.directly_holds_audio {
                assert_eq!(components, 3, "audio only at leaves of depth 3");
            } else {
                assert!(components < 3, "containers sit above the leaf level");
            }
        }
    }

    #[test]
    fn generate_handles_zero_knobs() {
        assert!(generate(0, 3, 3, 0.5).is_empty());
        assert!(generate(10, 0, 3, 0.5).is_empty());
        assert!(generate(10, 3, 0, 0.5).is_empty());
    }

    #[test]
    fn generate_alternates_gap_and_covered_at_half_rate() {
        let folders = generate(20, 2, 4, 0.5);
        let leaves: Vec<&ScannedFolder> =
            folders.iter().filter(|f| f.directly_holds_audio).collect();
        assert!(!leaves.is_empty());
        let mut gaps = 0;
        let mut covered = 0;
        for leaf in &leaves {
            if leaf.missing_ebook {
                gaps += 1;
                assert!(leaf.cover_files.is_empty(), "a gap has no cover files");
            } else {
                covered += 1;
                assert!(!leaf.cover_files.is_empty(), "a covered leaf has a cover");
            }
        }
        assert!(gaps > 0 && covered > 0, "both kinds appear at rate 0.5");
    }

    #[test]
    fn synthetic_root_scan_carries_generated_folders() {
        let scan = synthetic_root_scan(100, 3, 5, 0.5);
        let RootScan::Walked {
            canonical_path,
            folders,
        } = scan
        else {
            panic!("synthetic_root_scan must produce a Walked variant");
        };
        assert_eq!(canonical_path, PathBuf::from("/Audiobooks"));
        assert_eq!(folders.len(), generate(100, 3, 5, 0.5).len());
    }
}
