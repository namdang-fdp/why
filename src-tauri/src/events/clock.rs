//! The one place the event stores read the wall clock.
//!
//! The stale-session rule ([`super::timeline::effective_status`]) needs "now",
//! but pure code must not read the clock (it could not be tested). So a store is
//! handed a [`Clock`] when it is built, reads it once per query and passes the
//! number down. Tests hand it a fixed one.

use std::fmt;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

/// Something that says what time it is, in Unix milliseconds.
#[derive(Clone)]
pub struct Clock(Arc<dyn Fn() -> u64 + Send + Sync>);

impl Clock {
    /// A clock backed by any function (a test can count calls or move time).
    pub fn new(now: impl Fn() -> u64 + Send + Sync + 'static) -> Self {
        Self(Arc::new(now))
    }

    /// The real time.
    pub fn system() -> Self {
        Self::new(|| {
            // A clock before 1970 is a broken machine, not a reason to fail.
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
                .unwrap_or(0)
        })
    }

    /// A clock that always says `ms`.
    pub fn fixed(ms: u64) -> Self {
        Self::new(move || ms)
    }

    pub fn now_ms(&self) -> u64 {
        (self.0)()
    }
}

impl fmt::Debug for Clock {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Clock(..)")
    }
}
