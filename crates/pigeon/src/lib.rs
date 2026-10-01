//! pigeon on one machine: the daemon that runs every group, its localhost
//! JSON API and web UI, and the command line on that API, all generated
//! from one catalog of actions; and the relay a group may serve itself.

pub mod api;
pub mod args;
pub mod catalog;
pub mod cli;
pub mod client;
pub mod daemon;
mod form;
mod group_pages;
pub mod home;
mod pages;
pub mod perform;
pub mod relay;
pub mod render;
pub mod serve;
pub mod shared_root;
mod web;
