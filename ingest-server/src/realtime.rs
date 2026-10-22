//! Realtime publish hook.
//!
//! Only newly inserted events enter the durable Postgres outbox. This worker
//! forwards them to the configured webhook (`PIPELINE_URL`) and retries without
//! making the device re-upload.

use std::time::Duration;

use sqlx::PgPool;

use crate::db;

pub struct Publisher {
    pool: PgPool,
    client: reqwest::Client,
    url: String,
    token: String,
}

impl Publisher {
    pub fn new(pool: PgPool, url: String, token: String) -> Self {
        Self {
            pool,
            client: reqwest::Client::new(),
            url,
            token,
        }
    }

    pub async fn run(self) {
        loop {
            if let Err(error) = self.flush_once().await {
                tracing::error!("pipeline: outbox flush failed: {error}");
            }
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    }

    async fn flush_once(&self) -> anyhow::Result<()> {
        for pending in db::pending_pipeline_events(&self.pool, 100).await? {
            let response = self
                .client
                .post(&self.url)
                .bearer_auth(&self.token)
                .json(&pending.envelope)
                .send()
                .await;
            match response {
                Ok(response) if response.status().is_success() => {
                    db::mark_pipeline_delivered(&self.pool, &pending.event_id).await?;
                    tracing::debug!(event_id = %pending.event_id, "pipeline: delivered event");
                }
                Ok(response) => {
                    let status = response.status();
                    let body = response.text().await.unwrap_or_default();
                    self.defer(&pending, &format!("HTTP {status}: {body}"))
                        .await?;
                }
                Err(error) => self.defer(&pending, &error.to_string()).await?,
            }
        }
        Ok(())
    }

    async fn defer(
        &self,
        pending: &db::PendingPipelineEvent,
        error: &str,
    ) -> Result<(), sqlx::Error> {
        let exponent = pending.attempts.clamp(0, 8) as u32;
        let retry_after = (2_i64.pow(exponent)).clamp(5, 300);
        tracing::warn!(
            event_id = %pending.event_id,
            retry_after,
            "pipeline: delivery deferred: {error}"
        );
        db::defer_pipeline_event(&self.pool, &pending.event_id, retry_after, error).await
    }
}
