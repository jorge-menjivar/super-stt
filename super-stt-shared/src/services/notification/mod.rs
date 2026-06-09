// SPDX-License-Identifier: GPL-3.0-only
mod broadcast;
mod query;

use anyhow::Result;
use chrono::Utc;
use dashmap::DashMap;
use log::{debug, warn};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::broadcast as tokio_broadcast;
use tokio::time::{Duration, interval};
use uuid::Uuid;

use crate::models::protocol::NotificationEvent;

#[derive(Debug, Clone)]
pub struct Subscriber {
    pub id: String,
    pub event_types: Vec<String>,
    pub client_info: HashMap<String, Value>,
    pub sender: tokio_broadcast::Sender<NotificationEvent>,
    pub created_at: chrono::DateTime<Utc>,
}

impl Subscriber {
    /// Whether this subscriber should receive events of `event_type`: it
    /// subscribed to everything (empty list), to this type explicitly, or to
    /// the `"*"` wildcard. Single source of truth for event eligibility.
    fn wants(&self, event_type: &str) -> bool {
        self.event_types.is_empty() || self.event_types.iter().any(|t| t == event_type || t == "*")
    }
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
                    events_with_time.sort_by_key(|a| a.1);

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
    ) -> Result<(String, tokio_broadcast::Receiver<NotificationEvent>)> {
        if self.subscribers.len() >= self.max_subscribers {
            return Err(anyhow::anyhow!("Maximum number of subscribers reached"));
        }

        let client_id = Uuid::new_v4().to_string();
        let (sender, receiver) = tokio_broadcast::channel(100);

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

    /// Remove a subscriber by id. No-op if the id isn't registered.
    pub fn unsubscribe(&self, client_id: &str) {
        if let Some((_, subscriber)) = self.subscribers.remove(client_id) {
            debug!("Client {} unsubscribed", subscriber.id);
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
