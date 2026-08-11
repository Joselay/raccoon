#[cfg(not(any(feature = "onig", feature = "fancy")))]
compile_error!("enable either the `onig` (default) or `fancy` syntax backend feature");

pub mod app;
pub mod cli;
pub mod config;
pub mod dashboard;
pub mod diff;
pub mod file_icon;
pub mod git_diff;
pub mod highlight;
pub mod repository;
pub mod terminal;
pub mod theme;
pub mod worker;
