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

pub use self::core::*;
pub use self::listener::*;
pub use self::observability::*;
pub use self::performance::*;
pub use self::resilience::*;
pub use self::secrets::*;
pub use self::security::*;
pub use self::upstream::*;
