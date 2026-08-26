use std::collections::HashMap;

use serde::{Deserialize, Serialize};

#[path = "config/core.rs"]
mod core;
#[path = "config/listener.rs"]
mod listener;
#[path = "config/observability.rs"]
mod observability;
#[path = "config/performance.rs"]
mod performance;
#[path = "config/resilience.rs"]
mod resilience;
#[path = "config/secrets.rs"]
mod secrets;
#[path = "config/security.rs"]
mod security;
#[cfg(test)]
#[path = "config/tests.rs"]
mod tests;
#[path = "config/upstream.rs"]
mod upstream;

pub use self::{
    core::*, listener::*, observability::*, performance::*, resilience::*, secrets::*, security::*,
    upstream::*,
};
