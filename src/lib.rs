//! Library surface for the noworries CLI, so integration tests can drive the
//! same code paths the binary uses.
//
// Several spec fields (HTTP/DB/Kafka/Redis assertions) and framework methods
// are parsed and wired now but only consumed once the check runner + app start
// land in later phases. Allow dead_code crate-wide until then.
#![allow(dead_code)]

pub mod app;
pub mod checks;
pub mod compose;
pub mod docker;
pub mod edgecases;
pub mod externals;
pub mod flink;
pub mod framework;
pub mod git;
pub mod lifecycle;
pub mod mock;
pub mod report;
pub mod reports;
pub mod runner;
pub mod services;
pub mod spec;
