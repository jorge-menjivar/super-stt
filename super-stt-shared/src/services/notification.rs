// SPDX-License-Identifier: GPL-3.0-only
use anyhow::Result;
use chrono::Utc;
use dashmap::DashMap;
use log::{debug, warn};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::broadcast;
use tokio::task::JoinSet;
use tokio::time::{Duration, interval, timeout};
use uuid::Uuid;

use crate::models::protocol::NotificationEvent;

#[derive(Debug, Clone)]
pub struct Subscriber {
    pub id: String,
    pub event_types: Vec<String>,
    pub client_info: HashMap<String, Value>,
    pub sender: broadcast::Sender<NotificationEvent>,
    pub created_at: chrono::DateTime<Utc>,
}

pub struct NotificationManager {
    pub subscribers: Arc<DashMap<String, Subscriber>>,
    event_history: Arc<DashMap<String, (NotificationEvent, chrono::DateTime<Utc>)>>,
    max_history_size: usize,
    max_subscribers: usize,
    cleanup_handle: Option<tokio::task::JoinHandle<()>>,
    broadcast_timeout: Duration,
}

impl NotificationManager {
    #[must_use]
    pub fn new(max_history_size: usize, max_subscribers: usize) -> Self {
        Self {
            subscribers: Arc::new(DashMap::new()),
            event_history: Arc::new(DashMap::new()),
            max_history_size,
            max_subscribers,
            cleanup_handle: None,
            broadcast_timeout: Duration::from_millis(100), // Timeout per subscriber
        }
    }

    /// Configure the timeout for broadcasting to each subscriber
    pub fn set_broadcast_timeout(&mut self, timeout: Duration) {
        self.broadcast_timeout = timeout;
    }

    /// Get current broadcast timeout
    #[must_use]
    pub fn get_broadcast_timeout(&self) -> Duration {
        self.broadcast_timeout
    }

    /// Start background cleanup task
    pub fn start_background_cleanup(&mut self) {
        let event_history = Arc::clone(&self.event_history);
        let subscribers = Arc::clone(&self.subscribers);
        let max_history_size = self.max_history_size;

        let handle = tokio::spawn(async move {
            let mut cleanup_interval = interval(Duration::from_secs(30)); // Cleanup every 30 seconds

            loop {
                cleanup_interval.tick().await;

                // Cleanup old events
                if event_history.len() > max_history_size {
                    let mut events_with_time: Vec<(String, chrono::DateTime<Utc>)> = event_history
                        .iter()
                        .map(|entry| (entry.key().clone(), entry.value().1))
                        .collect();

                    // Sort by timestamp (oldest first)
                    events_with_time.sort_by(|a, b| a.1.cmp(&b.1));

                    // Remove oldest events
                    let to_remove = events_with_time.len().saturating_sub(max_history_size);
                    for (key, _) in events_with_time.iter().take(to_remove) {
                        event_history.remove(key);
                    }

                    debug!("Cleaned up {to_remove} old events");
                }

                // Cleanup disconnected subscribers
                let mut to_remove = Vec::new();
                for entry in subscribers.iter() {
                    let subscriber = entry.value();
                    if subscriber.sender.receiver_count() == 0 {
                        to_remove.push(subscriber.id.clone());
                    }
                }

                for id in to_remove {
                    if let Some((_, subscriber)) = subscribers.remove(&id) {
                        debug!("Cleaned up disconnected subscriber {}", subscriber.id);
                    }
                }
            }
        });

        self.cleanup_handle = Some(handle);
    }

    /// # Errors
    ///
    /// Returns an error if the maximum number of subscribers is reached.
    pub fn subscribe(
        &self,
        event_types: Vec<String>,
        client_info: HashMap<String, Value>,
    ) -> Result<(String, broadcast::Receiver<NotificationEvent>)> {
        if self.subscribers.len() >= self.max_subscribers {
            return Err(anyhow::anyhow!("Maximum number of subscribers reached"));
        }

        let client_id = Uuid::new_v4().to_string();
        let (sender, receiver) = broadcast::channel(100);

        let subscriber = Subscriber {
            id: client_id.clone(),
            event_types,
            client_info,
            sender,
            created_at: Utc::now(),
        };

        self.subscribers.insert(client_id.clone(), subscriber);
        debug!("Client {client_id} subscribed");

        Ok((client_id, receiver))
    }

    /// # Errors
    ///
    /// Returns an error if the subscriber is not found.
    pub fn unsubscribe(&self, client_id: &str) {
        if let Some((_, subscriber)) = self.subscribers.remove(client_id) {
            debug!("Client {} unsubscribed", subscriber.id);
        }
    }

    /// Truly async broadcast event - sends to all subscribers concurrently
    ///
    /// # Errors
    ///
    /// Returns an error if the event type is not found.
    pub async fn broadcast_event(
        &self,
        event_type: String,
        client_id: String,
        data: Value,
    ) -> Result<usize> {
        let event = NotificationEvent {
            event_type_field: "notification".to_string(),
            event_type: event_type.clone(),
            client_id: client_id.clone(),
            timestamp: Utc::now().to_rfc3339(),
            data,
        };

        // Store in history with timestamp for cleanup
        let event_id = Uuid::new_v4().to_string();
        let stored_at = Utc::now();
        self.event_history
            .insert(event_id.clone(), (event.clone(), stored_at));

        // Collect eligible subscribers
        let eligible_subscribers: Vec<(String, broadcast::Sender<NotificationEvent>)> = self
            .subscribers
            .iter()
            .filter_map(|entry| {
                let subscriber = entry.value();
                let should_send = subscriber.event_types.is_empty()
                    || subscriber.event_types.contains(&event_type)
                    || subscriber.event_types.contains(&"*".to_string());

                if should_send {
                    Some((subscriber.id.clone(), subscriber.sender.clone()))
                } else {
                    None
                }
            })
            .collect();

        if eligible_subscribers.is_empty() {
            return Ok(0);
        }

        // Create a JoinSet to manage concurrent sends
        let mut join_set = JoinSet::new();
        let event_clone = event.clone();
        let timeout_duration = self.broadcast_timeout;

        // Spawn async tasks for each subscriber
        for (subscriber_id, sender) in &eligible_subscribers {
            let sender = sender.clone();
            let event = event_clone.clone();
            let subscriber_id = subscriber_id.clone();
            let inner_timeout_duration = timeout_duration;

            join_set.spawn(async move {
                // Add timeout to prevent hanging on slow subscribers
                let result =
                    timeout(inner_timeout_duration, async move { sender.send(event) }).await;

                match result {
                    Ok(send_result) => match send_result {
                        Ok(_) => (subscriber_id, true, None),
                        Err(e) => (subscriber_id, false, Some(format!("Send failed: {e}"))),
                    },
                    Err(_) => (subscriber_id, false, Some("Timeout".to_string())),
                }
            });
        }

        // Collect results from all tasks
        let mut delivered = 0;
        let mut failed_subscribers = Vec::new();

        while let Some(result) = join_set.join_next().await {
            match result {
                Ok((subscriber_id, success, error)) => {
                    if success {
                        delivered += 1;
                    } else {
                        failed_subscribers.push(subscriber_id.clone());
                        if let Some(error_msg) = error {
                            debug!(
                                "Failed to send event to subscriber {subscriber_id}: {error_msg}"
                            );
                        }
                    }
                }
                Err(join_error) => {
                    warn!("Task join error during broadcast: {join_error}");
                }
            }
        }

        // Clean up failed subscribers asynchronously
        if !failed_subscribers.is_empty() {
            let subscribers = Arc::clone(&self.subscribers);
            tokio::spawn(async move {
                for id in failed_subscribers {
                    if let Some((_, subscriber)) = subscribers.remove(&id) {
                        debug!("Cleaned up failed subscriber {}", subscriber.id);
                    }
                }
            });
        }

        debug!("Event '{event_type}' delivered to {delivered} subscribers concurrently");

        Ok(delivered)
    }

    /// Synchronous version for backward compatibility and non-async contexts
    ///
    /// # Errors
    ///
    /// Returns an error if the event type is not found.
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
            let should_send = subscriber.event_types.is_empty()
                || subscriber.event_types.contains(&event_type.to_string())
                || subscriber.event_types.contains(&"*".to_string());

            if should_send {
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
    /// Returns an error if the events are empty.
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

    /// Get broadcasting statistics
    #[must_use]
    pub fn get_broadcast_stats(&self) -> Value {
        let total_subscribers = self.subscribers.len();
        let total_events = self.event_history.len();

        // Count active vs inactive subscribers
        let mut active_subscribers = 0;
        for subscriber in self.subscribers.iter() {
            if subscriber.sender.receiver_count() > 0 {
                active_subscribers += 1;
            }
        }

        serde_json::json!({
            "total_subscribers": total_subscribers,
            "active_subscribers": active_subscribers,
            "inactive_subscribers": total_subscribers - active_subscribers,
            "total_events_in_history": total_events,
            "max_history_size": self.max_history_size,
            "max_subscribers": self.max_subscribers,
            "broadcast_timeout_ms": self.broadcast_timeout.as_millis(),
        })
    }

    /// # Errors
    ///
    /// Returns an error if the timestamp cannot be parsed.
    pub fn get_recent_events(
        &self,
        since_timestamp: Option<String>,
        event_types: Option<Vec<String>>,
        limit: u32,
    ) -> Result<Vec<NotificationEvent>> {
        let limit = limit.min(1000) as usize;
        let mut events: Vec<NotificationEvent> = self
            .event_history
            .iter()
            .map(|entry| entry.value().0.clone())
            .collect();

        // Filter by timestamp if provided
        if let Some(since) = since_timestamp {
            let since_dt = chrono::DateTime::parse_from_rfc3339(&since)?;
            events.retain(|event| {
                if let Ok(event_dt) = chrono::DateTime::parse_from_rfc3339(&event.timestamp) {
                    event_dt > since_dt
                } else {
                    false
                }
            });
        }

        // Filter by event types if provided
        if let Some(types) = event_types
            && !types.is_empty()
            && !types.contains(&"*".to_string())
        {
            events.retain(|event| types.contains(&event.event_type));
        }

        // Sort by timestamp (newest first)
        events.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));

        // Limit results
        events.truncate(limit);

        Ok(events)
    }

    #[must_use]
    pub fn get_subscriber_info(&self) -> Value {
        let subscribers: Vec<Value> = self
            .subscribers
            .iter()
            .map(|entry| {
                let subscriber = entry.value();
                serde_json::json!({
                    "id": subscriber.id,
                    "event_types": subscriber.event_types,
                    "client_info": subscriber.client_info,
                    "created_at": subscriber.created_at.to_rfc3339()
                })
            })
            .collect();

        serde_json::json!({
            "total_subscribers": subscribers.len(),
            "subscribers": subscribers,
            "max_subscribers": self.max_subscribers,
            "event_history_size": self.event_history.len(),
            "max_history_size": self.max_history_size
        })
    }

    #[must_use]
    pub fn get_total_subscribers(&self) -> usize {
        self.subscribers.len()
    }

    /// Check if there are any subscribers to a specific event type
    #[must_use]
    pub fn has_subscribers_for_event(&self, event_type: &str) -> bool {
        self.subscribers.iter().any(|entry| {
            let subscriber = entry.value();
            subscriber.event_types.is_empty()
                || subscriber.event_types.contains(&event_type.to_string())
                || subscriber.event_types.contains(&"*".to_string())
        })
    }

    pub fn cleanup_disconnected_subscribers(&self) {
        let mut to_remove = Vec::new();

        for entry in self.subscribers.iter() {
            let subscriber = entry.value();
            if subscriber.sender.receiver_count() == 0 {
                to_remove.push(subscriber.id.clone());
            }
        }

        for id in to_remove {
            self.unsubscribe(&id);
        }
    }

    pub fn shutdown(&self) {
        warn!("Shutting down notification manager");

        // Cancel background cleanup task
        if let Some(handle) = &self.cleanup_handle {
            handle.abort();
        }

        self.subscribers.clear();
        self.event_history.clear();
    }
}
