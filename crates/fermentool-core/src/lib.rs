//! Fermentool control daemon, library crate.
//!
//! The binary (`src/main.rs`) wires these together:
//! [`config`] → [`store`] + [`engine`] on a [`control`] thread → the [`api`]
//! HTTP server, with [`notify`] following the journal on a thread of its own.

pub mod api;
pub mod config;
pub mod control;
pub mod engine;
pub mod notify;
pub mod scale;
pub mod store;
pub mod tracking;
pub mod trim;
pub mod transport;
