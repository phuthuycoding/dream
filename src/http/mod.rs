// TASK-006: session API + token auth + static dashboard serve.

pub mod middleware;
pub mod routes;
#[path = "static.rs"]
pub mod static_files;
pub mod tailscale;

#[cfg(test)]
mod tests;
