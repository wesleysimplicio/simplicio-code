//! Client-side contract for the `site_simpleti` account/subscription backend.
//!
//! This crate intentionally does **not** perform network I/O. `site_simpleti`
//! is a separate repository/backend that this client does not have access to
//! build or run; shipping an HTTP implementation against an API that does not
//! exist yet would just be untested code. What this crate provides instead:
//!
//! 1. Typed request/response schemas (see [`schema`]) that mirror
//!    `docs/contracts/site-simpleti-openapi.yaml` field-for-field, so a
//!    future HTTP client has ready-made, `serde`-checked types instead of
//!    loose `serde_json::Value` plumbing, and so the two repos have a single
//!    source of truth to diff against when the contract changes.
//! 2. [`idempotency`], a pure (no I/O) implementation of the "process a
//!    webhook event at most once per `event_id`" rule required by the
//!    contract's webhook invariant. This is the one piece of business logic
//!    from issue #15 that is genuinely implementable and testable without
//!    the backend existing, so it is implemented for real, with tests, rather
//!    than left as a doc.
//!
//! See `docs/contracts/site-simpleti-api.md` for the full rationale and for
//! what remains out of scope of this repository.

pub mod idempotency;
pub mod schema;
