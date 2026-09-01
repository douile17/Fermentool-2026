//! Fermentool control daemon — library crate.
//!
//! The binary (`src/main.rs`) is a thin shell over these modules.
//! Milestone 4 added [`store`]; milestone 5 adds [`engine`]; later milestones
//! add `config` and `api`.

pub mod engine;
pub mod store;
