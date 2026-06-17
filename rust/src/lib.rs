#![allow(dead_code, deprecated)]

mod build_info;
mod cli;
pub mod completions_cache;
pub mod config;
pub mod crypto;
pub mod error;
pub mod git;
pub mod keyring;
pub mod proto;
pub mod reference;
pub mod remote;
pub mod suggest;
#[cfg(test)]
mod test_env;
pub mod tui;
