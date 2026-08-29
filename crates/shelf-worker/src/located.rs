//! Source locations for errors that bubble out of a handler.
//!
//! A wasm Worker has no unwinder, so there is no stack to walk when a `?`
//! fails — the `backtrace`/`findshlibs` machinery the PostHog Rust SDK uses
//! cannot run here at all. `#[track_caller]` gets at the missing information
//! from the other end: `.at()` records the file and line of the `?` it is
//! attached to, and `main` turns that into the frame PostHog Error Tracking
//! shows for the issue.
//!
//! So `db::tin_detail(&d1).await.at()?` reports the line that failed rather
//! than the router boundary that observed it.

use worker::{Error, Result};

/// Separates the message from `file:line`. A control character, so it cannot
/// collide with anything already in an error message; [`split`] takes it back
/// off in `main`, before the message can reach a client.
pub const SEP: char = '\x1f';

pub trait Located<T> {
    /// Records where this `?` failed.
    fn at(self) -> Result<T>;
}

impl<T> Located<T> for Result<T> {
    #[track_caller]
    fn at(self) -> Result<T> {
        self.map_err(|e| {
            let msg = e.to_string();
            // Already located: an error passing through several `.at()?` on
            // its way up keeps the innermost location, which is the failure.
            if msg.contains(SEP) {
                return Error::RustError(msg);
            }
            let loc = std::panic::Location::caller();
            Error::RustError(format!("{msg}{SEP}{}:{}", loc.file(), loc.line()))
        })
    }
}

/// Splits a located message into its message and `file:line`, if it has one.
pub fn split(msg: &str) -> (&str, Option<&str>) {
    match msg.split_once(SEP) {
        Some((msg, loc)) => (msg, Some(loc)),
        None => (msg, None),
    }
}
