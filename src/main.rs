//! Interim CLI: load config, scan each library root, and print the gap tree.
//! The web UI replaces this entry point in a later increment.

use std::path::PathBuf;
use std::process::ExitCode;

use missing_ebooks::config::{Config, ConfigError, print_config_template};
use missing_ebooks::scanner::{ScanInputs, ScanSettings, scan};
use missing_ebooks::tree::{Node, build};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.iter().any(|a| a == "--print-config") {
        print!("{}", print_config_template());
        return ExitCode::SUCCESS;
    }

    let config = match Config::load(parse_config_path(&args).as_deref()) {
        Ok(cfg) => cfg,
        Err(err @ ConfigError::MissingLibraryRoots) => {
            eprintln!("{err}");
            return ExitCode::from(2);
        }
        Err(err) => {
            eprintln!("error: {err}");
            return ExitCode::from(1);
        }
    };

    let settings = match ScanSettings::compile(ScanInputs {
        audio_exts: &config.audio_exts,
        ebook_exts: &config.ebook_exts,
        excluded_dirs: &config.excluded_dirs,
        exclude_globs: &config.exclude_globs,
    }) {
        Ok(settings) => settings,
        Err(err) => {
            eprintln!("error: {err}");
            return ExitCode::from(1);
        }
    };

    for root in &config.library_roots {
        let canonical = match std::fs::canonicalize(root) {
            Ok(path) => path,
            Err(err) => {
                eprintln!("warning: skipping root {}: {err}", root.display());
                continue;
            }
        };
        println!("{}", canonical.display());
        let root_name = canonical
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(".");
        let forest = build(root_name, &scan(&canonical, &settings));
        if forest.is_empty() {
            println!("  (no missing ebooks in this root)");
        } else {
            for node in &forest {
                print_node(node, 1);
            }
        }
    }
    ExitCode::SUCCESS
}

fn parse_config_path(args: &[String]) -> Option<PathBuf> {
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if arg == "--config" {
            return iter.next().map(PathBuf::from);
        }
        if let Some(value) = arg.strip_prefix("--config=") {
            return Some(PathBuf::from(value));
        }
    }
    None
}

fn print_node(node: &Node, depth: usize) {
    let indent = "  ".repeat(depth);
    let suffix = if node.flagged { " *" } else { "" };
    println!("{indent}{}{suffix}", node.name);
    for child in &node.children {
        print_node(child, depth + 1);
    }
}
