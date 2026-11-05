// TASK-006: session API + token auth + static dashboard serve.

pub mod middleware;
pub mod routes;
#[path = "static.rs"]
pub mod static_files;

#[cfg(test)]
mod tests;
