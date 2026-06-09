// SPDX-License-Identifier: GPL-3.0-only
use super::NotificationManager;
use crate::models::protocol::NotificationEvent;
use anyhow::Result;
use serde_json::Value;

impl NotificationManager {
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
        self.subscribers
            .iter()
            .any(|entry| entry.value().wants(event_type))
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
}
