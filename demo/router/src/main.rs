//! Session router for the missing-ebooks public demo.
//!
//! Spawns one seeded `explore` sandbox per visitor, pins the browser to it with
//! a cookie, and reverse-proxies every later request to that process. See
//! docs/superpowers/specs/2026-06-08-demo-site-design.md.

mod banner;
mod capacity;
mod config;
mod ports;
mod session;

fn main() {
    println!("router placeholder");
}
