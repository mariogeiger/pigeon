//! pigeon on one machine: the daemon that runs every group, its localhost
//! JSON API and web UI, and the command line on that API, all generated
//! from one catalog of actions; the relay a group may serve itself; and
//! the update that rebuilds pigeon and restarts the daemon onto it.

pub mod api;
pub mod args;
pub mod catalog;
pub mod cli;
pub mod client;
pub mod daemon;
pub mod draft;
mod files_page;
mod form;
mod group_pages;
pub mod home;
mod pages;
pub mod perform;
pub mod program;
pub mod relay;
pub mod render;
mod selection_page;
pub mod serve;
pub mod shared_root;
pub mod update;
mod web;

/// pigeon's version and the commit it was built from.
pub const VERSION: &str = concat!(env!("CARGO_PKG_VERSION"), " (", env!("PIGEON_COMMIT"), ")");
