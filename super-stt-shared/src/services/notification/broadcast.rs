// SPDX-License-Identifier: GPL-3.0-only
use super::NotificationManager;
use crate::models::protocol::NotificationEvent;
use anyhow::Result;
use chrono::Utc;
use log::{debug, warn};
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::broadcast;
use tokio::task::JoinSet;
use tokio::time::timeout;
use uuid::Uuid;

impl NotificationManager {
    /// Collect senders for subscribers that match `event_type`.
    fn collect_eligible(
        &self,
        event_type: &str,
    ) -> Vec<(String, broadcast::Sender<NotificationEvent>)> {
        self.subscribers
            .iter()
            .filter_map(|entry| {
                let s = entry.value();
                s.wants(event_type)
                    .then(|| (s.id.clone(), s.sender.clone()))
            })
            .collect()
    }

    /// Fan out `event` to every eligible sender concurrently, each with a
    /// per-subscriber timeout. Returns (`delivered_count`, `failed_ids`).
    async fn fan_out(
        &self,
        eligible: Vec<(String, broadcast::Sender<NotificationEvent>)>,
        event: NotificationEvent,
    ) -> (usize, Vec<String>) {
        let mut join_set = JoinSet::new();
        let timeout_duration = self.broadcast_timeout;
        for (subscriber_id, sender) in eligible {
            let event = event.clone();
            join_set.spawn(async move {
                let result = timeout(timeout_duration, async move { sender.send(event) }).await;
                match result {
                    Ok(Ok(_)) => (subscriber_id, true, None),
                    Ok(Err(e)) => (subscriber_id, false, Some(format!("Send failed: {e}"))),
                    Err(_) => (subscriber_id, false, Some("Timeout".to_string())),
                }
            });
        }

        let mut delivered = 0;
        let mut failed = Vec::new();
        while let Some(joined) = join_set.join_next().await {
            match joined {
                Ok((_id, true, _)) => delivered += 1,
                Ok((id, false, err)) => {
                    if let Some(msg) = err {
                        debug!("Failed to send event to subscriber {id}: {msg}");
                    }
                    failed.push(id);
                }
                Err(join_error) => warn!("Task join error during broadcast: {join_error}"),
            }
        }
        (delivered, failed)
    }

    /// Remove failed subscribers in a detached task (mirrors original behavior).
    fn reap_failed(&self, failed: Vec<String>) {
        if failed.is_empty() {
            return;
        }
        let subscribers = Arc::clone(&self.subscribers);
        tokio::spawn(async move {
            for id in failed {
                if let Some((_, s)) = subscribers.remove(&id) {
                    debug!("Cleaned up failed subscriber {}", s.id);
                }
            }
        });
    }

    /// Truly async broadcast — sends to all matching subscribers concurrently.
    ///
    /// # Errors
    /// Currently infallible in practice; returns `Result` for call-site
    /// compatibility.
    pub async fn broadcast_event(
        &self,
        event_type: String,
        client_id: String,
        data: Value,
    ) -> Result<usize> {
        let event = NotificationEvent {
            event_type_field: "notification".to_string(),
            event_type: event_type.clone(),
            client_id,
            timestamp: Utc::now().to_rfc3339(),
            data,
        };
        self.event_history
            .insert(Uuid::new_v4().to_string(), (event.clone(), Utc::now()));

        let eligible = self.collect_eligible(&event_type);
        if eligible.is_empty() {
            return Ok(0);
        }
        let (delivered, failed) = self.fan_out(eligible, event).await;
        self.reap_failed(failed);
        debug!("Event '{event_type}' delivered to {delivered} subscribers concurrently");
        Ok(delivered)
    }

    /// Synchronous version for backward compatibility and non-async contexts.
    /// Returns the number of subscribers the event was delivered to.
    #[must_use]
    pub fn broadcast_event_sync(&self, event_type: &str, client_id: &str, data: Value) -> usize {
        // For now, just use the blocking version
        // In the future, we could spawn a task if in async context
        self.broadcast_event_blocking(event_type, client_id, data)
    }

    /// Blocking version for non-async contexts
    fn broadcast_event_blocking(&self, event_type: &str, client_id: &str, data: Value) -> usize {
        let event = NotificationEvent {
            event_type_field: "notification".to_string(),
            event_type: event_type.to_string(),
            client_id: client_id.to_string(),
            timestamp: Utc::now().to_rfc3339(),
            data,
        };

        // Store in history
        let event_id = Uuid::new_v4().to_string();
        let stored_at = Utc::now();
        self.event_history
            .insert(event_id, (event.clone(), stored_at));

        // Broadcast to relevant subscribers
        let mut delivered = 0;
        for subscriber in self.subscribers.iter() {
            if subscriber.wants(event_type) {
                match subscriber.sender.send(event.clone()) {
                    Ok(_) => delivered += 1,
                    Err(_) => {
                        debug!("Failed to send event to subscriber {}", subscriber.id);
                    }
                }
            }
        }

        debug!("Event '{event_type}' delivered to {delivered} subscribers");
        delivered
    }

    /// Batch broadcast multiple events concurrently - useful for high-throughput scenarios
    ///
    /// # Errors
    ///
    /// Propagates an error if any underlying `broadcast_event` call fails.
    pub async fn broadcast_events_batch(
        &self,
        events: Vec<(String, String, Value)>,
    ) -> Result<Vec<usize>> {
        if events.is_empty() {
            return Ok(vec![]);
        }

        // Process events sequentially but keep individual broadcasts async/concurrent
        let mut results = Vec::new();
        for (event_type, client_id, data) in events {
            let result = self.broadcast_event(event_type, client_id, data).await?;
            results.push(result);
        }
        Ok(results)
    }

    /// Stream events to a specific subscriber asynchronously
    ///
    /// # Errors
    ///
    /// Returns an error if the subscriber is not found.
    pub async fn stream_to_subscriber(
        &self,
        subscriber_id: &str,
        events: Vec<NotificationEvent>,
    ) -> Result<usize> {
        let subscriber = self
            .subscribers
            .get(subscriber_id)
            .ok_or_else(|| anyhow::anyhow!("Subscriber {subscriber_id} not found"))?;

        let sender = subscriber.sender.clone();
        drop(subscriber); // Release the reference early

        let mut delivered = 0;
        for event in events {
            // Use timeout for each event
            match timeout(self.broadcast_timeout, async { sender.send(event) }).await {
                Ok(Ok(_)) => delivered += 1,
                Ok(Err(_)) => {
                    debug!("Channel closed for subscriber {subscriber_id}");
                    break;
                }
                Err(_) => {
                    debug!("Timeout sending to subscriber {subscriber_id}");
                    break;
                }
            }
        }

        debug!("Streamed {delivered} events to subscriber {subscriber_id}");
        Ok(delivered)
    }
}
