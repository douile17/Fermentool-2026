//! Fermentool control daemon — library crate.
//!
//! The binary (`src/main.rs`) wires these together:
//! [`config`] → [`store`] + [`engine`] on a [`control`] thread → the [`api`]
//! HTTP server.

pub mod api;
pub mod config;
pub mod control;
pub mod engine;
pub mod store;
