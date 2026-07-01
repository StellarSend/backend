// Health check endpoints
// GET /health       - liveness probe (always 200 if process alive)
// GET /health/deep  - readiness probe (checks DB + Horizon connectivity)
// Added by Chiamaka Eze (#159)
pub const HEALTH_ROUTER_VERSION: &str = "1.0";
