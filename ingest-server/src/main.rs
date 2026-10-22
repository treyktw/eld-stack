//! ELD ingest server.
//!
//! Accepts device upload envelopes over HTTP, dedups on (device_id, seq),
//! stores in Postgres, and publishes new events to the realtime
//! layer (a generic webhook, via a transactional outbox). axum + tokio + tracing.

mod config;
mod db;
mod dedup;
mod error;
mod model;
mod realtime;
mod routes;
mod telemetry;

use std::sync::{Arc, Mutex};

use config::Config;
use dedup::DedupCache;
use realtime::Publisher;
use routes::AppState;

const DEDUP_CACHE_SIZE: usize = 4096;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _ = dotenvy::dotenv();
    // Held until `main` returns so queued events are flushed on shutdown.
    // Dropping this early discards the last errors before exit, which are
    // usually the ones worth having.
    let _telemetry = telemetry::init();

    // Config failures happen before anything is serving, so they are silent
    // in a container that simply never becomes healthy. Reported explicitly.
    let config = match Config::from_env() {
        Ok(config) => config,
        Err(e) => {
            tracing::error!("startup: configuration is invalid: {e}");
            return Err(e);
        }
    };
    tracing::info!("connecting to database");
    let pool = db::connect(&config.database_url).await.inspect_err(|e| {
        tracing::error!("startup: database connection failed: {e}");
    })?;
    db::migrate(&pool).await.inspect_err(|e| {
        tracing::error!("startup: schema migration failed: {e}");
    })?;
    tracing::info!("schema ready");

    if let (Some(url), Some(token)) = (config.pipeline_url.clone(), config.pipeline_token.clone()) {
        let publisher = Publisher::new(pool.clone(), url, token);
        tokio::spawn(publisher.run());
        tracing::info!("pipeline webhook forwarding enabled");
    } else {
        tracing::warn!(
            "pipeline webhook forwarding disabled; accepted records will remain in the outbox"
        );
    }

    let state = AppState {
        pool,
        config: config.clone(),
        dedup: Arc::new(Mutex::new(DedupCache::new(DEDUP_CACHE_SIZE))),
    };

    let app = routes::router(state);
    let listener = tokio::net::TcpListener::bind(&config.bind_addr)
        .await
        .inspect_err(|e| {
            tracing::error!("startup: cannot bind {}: {e}", config.bind_addr);
        })?;
    tracing::info!(
        devices = config.device_tokens.len(),
        "ingest server listening on {}",
        config.bind_addr
    );
    // A serve loop that exits is the server going away. Devices keep uploading
    // into a closed port and queue locally until their storage fills.
    if let Err(e) = axum::serve(listener, app).await {
        tracing::error!("serve loop exited: {e}");
        return Err(e.into());
    }
    Ok(())
}
