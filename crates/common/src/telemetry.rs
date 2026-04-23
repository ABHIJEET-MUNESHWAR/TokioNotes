//! JSON-structured tracing setup. Honours `RUST_LOG`.
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

pub fn init(service: &str) {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,sqlx=warn,hyper=warn"));
    let fmt_layer = fmt::layer().with_target(true).with_level(true).json();
    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(fmt_layer)
        .try_init();
    tracing::info!(service = service, "telemetry initialised");
}

