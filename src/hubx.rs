use crate::http::{Req, Resp};
use crate::App;
use std::sync::Arc;

/// Return Some(response) for routes this module owns, None otherwise.
pub fn handle(_app: &Arc<App>, _req: &Req) -> Option<Resp> { None }
/// Called once at startup (spawn background threads here if needed).
pub fn start(_app: &Arc<App>) {}
