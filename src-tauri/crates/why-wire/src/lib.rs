//! The wire protocol between `why-hook` and the WHY app.
//!
//! Both sides link this crate, so they agree on three things without ever
//! talking to each other first:
//!
//! * [`envelope`]: what one forwarded hook event looks like ([`Envelope`]);
//! * [`frame`]: how envelopes are put on the socket (one JSON object per line);
//! * [`instance`]: where the socket, the spool and the allowlist live for the
//!   installed app and for each dev instance.
//!
//! Uses serde for the wire format and thiserror for errors. The hook runs once
//! per agent event, so this crate stays independent of Tauri and Tokio.

pub mod envelope;
pub mod frame;
pub mod instance;

pub use envelope::{Envelope, Harness, PROTOCOL_VERSION};
pub use frame::{MAX_LINE_BYTES, WireError, decode_line, encode_line};
pub use instance::{
    APP_IDENTIFIER, INSTANCE_ENV, Instance, InstancePaths, ParseInstanceError, PathEnv, PathError,
    installed_data_dir,
};
