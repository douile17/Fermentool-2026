//! Fermentool control daemon — library crate.
//!
//! The binary (`src/main.rs`) is a thin shell over these modules. Milestone 4
//! adds [`store`]; later milestones add `config`, `engine`, `api`.

pub mod store;
