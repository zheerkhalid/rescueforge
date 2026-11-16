//! RescueForge: file recovery for Windows.
//!
//! The logic lives in this library so that it can be unit-tested; `main.rs`
//! only parses the command line and opens the window.
//!
//! Safety rule of the whole crate: the scanned drive, disk or image is only
//! ever opened for reading (see [`source`]).
// `chunks_exact(n)` with a literal `n` is used all over the parsers; it reads
// better there than `as_chunks`, which clippy would prefer.
#![allow(clippy::chunks_exact_to_as_chunks)]

pub mod carve;
pub mod control;
pub mod demo;
pub mod fs;
pub mod imaging;
pub mod model;
pub mod partition;
pub mod preview;
pub mod recover;
pub mod scan;
pub mod selftest;
pub mod session;
pub mod settings;
pub mod source;
pub mod sys;
pub mod testimg;
pub mod ui;
pub mod util;

#[cfg(test)]
mod scan_tests;
