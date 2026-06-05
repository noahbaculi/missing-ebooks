//! missing-ebooks: surface audiobook folders that hold audio but no ebook.

pub mod config;
pub mod scanner;
pub mod tree;

// Declared now so the architecture is fixed in code. Filled in later increments.
pub mod service;
pub mod state;
pub mod web;
