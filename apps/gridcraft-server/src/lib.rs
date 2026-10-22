//! Trusted HTTP and persistence boundary for the existing GridCraft application.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod config;
pub mod providers;
pub mod runs;
pub mod storage;
pub mod web;
