//! pigeon on one machine: the daemon that runs every group, its localhost
//! JSON API and web UI, and the command line on that API, all generated
//! from one catalog of actions; the relay a group may serve itself; and
//! the update that rebuilds pigeon and restarts the daemon onto it.

pub mod api;
pub mod args;
pub mod catalog;
pub mod cli;
pub mod client;
pub mod complete;
pub mod config_preview;
pub mod daemon;
mod file_status;
pub mod file_tree;
mod files_page;
mod form;
mod group_pages;
pub mod home;
mod overview_page;
mod pages;
pub mod perform;
pub mod program;
pub mod relay;
pub mod render;
pub mod serve;
pub mod service;
pub mod setup;
pub mod shared_root;
pub mod update;
mod web;

/// pigeon's version and the commit it was built from.
pub const VERSION: &str = concat!(env!("CARGO_PKG_VERSION"), " (", env!("PIGEON_COMMIT"), ")");

/// What this pigeon tells any machine that asks which pigeon it runs.
#[must_use]
pub fn announcement() -> pigeon_net::hello::Announcement {
    pigeon_net::hello::Announcement::speaking_ours(env!("CARGO_PKG_VERSION"), env!("PIGEON_COMMIT"))
}
