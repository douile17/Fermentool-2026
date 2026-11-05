//! Timelapses (camera plan C): the schedule, the captures on disk, and the
//! `timelapse` thread that takes the images. Nothing here runs on the
//! control thread: the run is followed through the status broadcast.

pub mod files;
pub mod schedule;
mod service;
#[cfg(test)]
mod service_tests;

pub use schedule::Schedule;
pub use service::*;
