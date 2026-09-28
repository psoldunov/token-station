//! Core domain model for Token Station.
//!
//! This crate is the contract shared by the daemon, the providers and (through the
//! JSON snapshot) the desktop front ends. It holds no IO beyond reading config and
//! pricing text handed to it, so everything here is unit-testable.

pub mod alerts;
pub mod assemble;
pub mod config;
pub mod discovery;
pub mod meter;
pub mod pricing;
pub mod provider;
pub mod snapshot;
pub mod time;
pub mod tokens;

pub use provider::{Ingest, IngestError, Provider, RefreshOutcome};
pub use snapshot::{
    AccountTokens, BreakdownRow, Credits, DailyTokens, Level, Meter, MeterBar, ModelTotals,
    ProviderId, ProviderSnapshot, ProviderState, SCHEMA_VERSION, Snapshot, TokenReport,
    TokenTotals, UsageWindow, WindowKind,
};
