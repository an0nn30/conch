//! Message bus for inter-plugin communication.
//!
//! The bus provides three mechanisms:
//!
//! 1. **Event broadcast** (pub/sub) — plugins subscribe to event types and
//!    receive published events via their mailbox.
//! 2. **Direct query routing** (request/response) — a plugin sends a query to
//!    a target plugin and blocks until it responds (via oneshot channel).
//! 3. **Service registry** — plugins register named services; the bus resolves
//!    service names to plugin names for query routing.
//!
//! Each loaded plugin has a single mailbox channel (`PluginMail`) through which
//! all messages are delivered — bus events, queries, render requests, and
//! shutdown signals. This lets the plugin thread drain a single receiver.

use std::collections::HashMap;

use parking_lot::RwLock;
use serde_json::Value;
use tokio::sync::{mpsc, oneshot};

/// Capacity of each plugin's mailbox channel.
const MAILBOX_CAPACITY: usize = 256;

// ---------------------------------------------------------------------------
// Message types
// ---------------------------------------------------------------------------

/// A broadcast event published on the bus.
#[derive(Debug, Clone)]
pub struct BusMessage {
    /// Name of the plugin that published this event.
    pub source: String,
    /// Dotted event type (e.g., `"ssh.session_ready"`).
    pub event_type: String,
    /// Arbitrary JSON payload.
    pub data: Value,
}

/// A direct query sent to a specific plugin.
pub struct QueryRequest {
    /// Name of the plugin that sent the query.
    pub source: String,
    /// Method name (e.g., `"exec"`, `"get_sessions"`).
    pub method: String,
    /// JSON arguments.
    pub args: Value,
    /// Channel for the response.
    pub reply: oneshot::Sender<QueryResponse>,
}

/// Response to a [`QueryRequest`].
#[derive(Debug)]
pub struct QueryResponse {
    pub result: Result<Value, String>,
}

/// Envelope for all messages delivered to a plugin's mailbox.
///
/// Both the bus and the native loader send through the same channel, so the
/// plugin thread only needs one `mpsc::Receiver<PluginMail>`.
pub enum PluginMail {
    /// A broadcast event from the bus.
    BusEvent(BusMessage),
    /// A direct query from another plugin (or the host).
    BusQuery(QueryRequest),
    /// Host requests the plugin to render its widget tree.
    RenderRequest { reply: oneshot::Sender<String> },
    /// A widget interaction event (button click, text input, etc.).
    /// JSON-encoded `PluginEvent::Widget(...)`.
    WidgetEvent { json: String },
    /// Graceful shutdown signal.
    Shutdown,
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum BusError {
    /// The target plugin is not registered on the bus.
    PluginNotFound(String),
    /// No plugin provides the requested service.
    ServiceNotFound(String),
    /// The target plugin's mailbox channel is closed.
    ChannelClosed,
    /// The query response channel was dropped before a response arrived.
    ResponseDropped,
}

impl std::fmt::Display for BusError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PluginNotFound(n) => write!(f, "plugin not found: {n}"),
            Self::ServiceNotFound(n) => write!(f, "service not found: {n}"),
            Self::ChannelClosed => write!(f, "plugin mailbox closed"),
            Self::ResponseDropped => write!(f, "query response channel dropped"),
        }
    }
}

impl std::error::Error for BusError {}

// ---------------------------------------------------------------------------
// PluginBus
// ---------------------------------------------------------------------------

/// Central message bus for inter-plugin communication.
///
/// Thread-safe — all methods take `&self` and use interior mutability.
pub struct PluginBus {
    /// plugin_name → sender into that plugin's mailbox.
    plugin_senders: RwLock<HashMap<String, mpsc::Sender<PluginMail>>>,
    /// event_type → list of subscribed plugin names.
    subscriptions: RwLock<HashMap<String, Vec<String>>>,
    /// service_name → plugin_name that provides it.
    services: RwLock<HashMap<String, String>>,
}

impl PluginBus {
    pub fn new() -> Self {
        Self {
            plugin_senders: RwLock::new(HashMap::new()),
            subscriptions: RwLock::new(HashMap::new()),
            services: RwLock::new(HashMap::new()),
        }
    }

    /// Register a plugin and create its mailbox channel.
    ///
    /// Returns the receiving end. The bus (and anyone who calls
    /// [`sender_for`](Self::sender_for)) can send messages through it.
    pub fn register_plugin(&self, name: &str) -> mpsc::Receiver<PluginMail> {
        let (tx, rx) = mpsc::channel(MAILBOX_CAPACITY);
        self.plugin_senders.write().insert(name.to_string(), tx);
        rx
    }

    /// Get a clone of a plugin's mailbox sender.
    ///
    /// The native loader uses this to send `RenderRequest` / `Shutdown`.
    pub fn sender_for(&self, plugin_name: &str) -> Option<mpsc::Sender<PluginMail>> {
        self.plugin_senders.read().get(plugin_name).cloned()
    }

    /// Remove a plugin and all its subscriptions and services.
    pub fn unregister_plugin(&self, name: &str) {
        self.plugin_senders.write().remove(name);

        let mut subs = self.subscriptions.write();
        for subscribers in subs.values_mut() {
            subscribers.retain(|s| s != name);
        }

        let mut svcs = self.services.write();
        svcs.retain(|_, plugin| plugin != name);
    }

    // -- Pub/Sub ------------------------------------------------------------

    /// Subscribe a plugin to an event type.
    pub fn subscribe(&self, plugin_name: &str, event_type: &str) {
        self.subscriptions
            .write()
            .entry(event_type.to_string())
            .or_default()
            .push(plugin_name.to_string());
    }

    /// Publish an event to all subscribers (except the source plugin).
    ///
    /// Uses `try_send` so a slow/blocked subscriber cannot stall the publisher.
    /// Events that cannot be delivered are dropped with a warning log.
    pub fn publish(&self, source: &str, event_type: &str, data: Value) {
        let subs = self.subscriptions.read();
        let Some(subscribers) = subs.get(event_type) else {
            return;
        };

        let senders = self.plugin_senders.read();
        for sub_name in subscribers {
            // Don't echo back to the publisher.
            if sub_name == source {
                continue;
            }
            if let Some(sender) = senders.get(sub_name.as_str()) {
                let msg = BusMessage {
                    source: source.to_string(),
                    event_type: event_type.to_string(),
                    data: data.clone(),
                };
                if sender.try_send(PluginMail::BusEvent(msg)).is_err() {
                    log::warn!(
                        "bus: failed to deliver event {event_type} to {sub_name} (full/closed)"
                    );
                }
            }
        }
    }

    // -- Service Registry ---------------------------------------------------

    /// Register a named service provided by a plugin.
    pub fn register_service(&self, plugin_name: &str, service_name: &str) {
        self.services
            .write()
            .insert(service_name.to_string(), plugin_name.to_string());
    }

    /// Resolve a service name to the plugin that provides it.
    pub fn resolve_service(&self, service_name: &str) -> Option<String> {
        self.services.read().get(service_name).cloned()
    }

    // -- Direct Query -------------------------------------------------------

    /// Send a query to a plugin and block until the response arrives.
    ///
    /// Intended for use on plain OS threads (plugin threads calling via HostApi).
    /// **Panics** if called from inside a tokio async context.
    pub fn query_blocking(
        &self,
        target: &str,
        method: &str,
        args: Value,
        source: &str,
    ) -> Result<QueryResponse, BusError> {
        let sender = self
            .plugin_senders
            .read()
            .get(target)
            .cloned()
            .ok_or_else(|| BusError::PluginNotFound(target.to_string()))?;

        let (reply_tx, reply_rx) = oneshot::channel();
        let req = PluginMail::BusQuery(QueryRequest {
            source: source.to_string(),
            method: method.to_string(),
            args,
            reply: reply_tx,
        });

        sender
            .blocking_send(req)
            .map_err(|_| BusError::ChannelClosed)?;

        reply_rx
            .blocking_recv()
            .map_err(|_| BusError::ResponseDropped)
    }

    /// Send a query to a plugin (async version).
    pub async fn query(
        &self,
        target: &str,
        method: &str,
        args: Value,
        source: &str,
    ) -> Result<QueryResponse, BusError> {
        let sender = self
            .plugin_senders
            .read()
            .get(target)
            .cloned()
            .ok_or_else(|| BusError::PluginNotFound(target.to_string()))?;

        let (reply_tx, reply_rx) = oneshot::channel();
        let req = PluginMail::BusQuery(QueryRequest {
            source: source.to_string(),
            method: method.to_string(),
            args,
            reply: reply_tx,
        });

        sender
            .send(req)
            .await
            .map_err(|_| BusError::ChannelClosed)?;

        reply_rx.await.map_err(|_| BusError::ResponseDropped)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn register_and_unregister_plugin() {
        let bus = PluginBus::new();
        let _rx = bus.register_plugin("test");
        assert!(bus.sender_for("test").is_some());
        bus.unregister_plugin("test");
        assert!(bus.sender_for("test").is_none());
    }

    #[test]
    fn subscribe_and_publish() {
        let bus = PluginBus::new();
        let mut rx = bus.register_plugin("listener");
        bus.subscribe("listener", "ssh.connected");

        bus.publish("publisher", "ssh.connected", json!({"host": "10.0.0.1"}));

        let mail = rx.try_recv().unwrap();
        match mail {
            PluginMail::BusEvent(msg) => {
                assert_eq!(msg.source, "publisher");
                assert_eq!(msg.event_type, "ssh.connected");
                assert_eq!(msg.data["host"], "10.0.0.1");
            }
            _ => panic!("expected BusEvent"),
        }
    }

    #[test]
    fn publish_does_not_echo_to_source() {
        let bus = PluginBus::new();
        let mut rx = bus.register_plugin("self_pub");
        bus.subscribe("self_pub", "ping");
        bus.publish("self_pub", "ping", json!(null));

        // No message should arrive — source is excluded.
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn publish_to_unsubscribed_event_is_noop() {
        let bus = PluginBus::new();
        let mut rx = bus.register_plugin("listener");
        bus.subscribe("listener", "ssh.connected");

        bus.publish("src", "other.event", json!(null));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn multiple_subscribers_all_receive() {
        let bus = PluginBus::new();
        let mut rx_a = bus.register_plugin("a");
        let mut rx_b = bus.register_plugin("b");
        bus.subscribe("a", "tick");
        bus.subscribe("b", "tick");

        bus.publish("source", "tick", json!(42));

        assert!(matches!(rx_a.try_recv().unwrap(), PluginMail::BusEvent(_)));
        assert!(matches!(rx_b.try_recv().unwrap(), PluginMail::BusEvent(_)));
    }

    #[test]
    fn service_registry() {
        let bus = PluginBus::new();
        assert!(bus.resolve_service("exec").is_none());

        bus.register_service("ssh", "exec");
        assert_eq!(bus.resolve_service("exec").unwrap(), "ssh");
    }

    #[test]
    fn unregister_removes_subscriptions_and_services() {
        let bus = PluginBus::new();
        let _rx = bus.register_plugin("ssh");
        bus.subscribe("ssh", "tick");
        bus.register_service("ssh", "exec");

        bus.unregister_plugin("ssh");

        assert!(bus.resolve_service("exec").is_none());
        // Publish should not panic even though subscriber is gone.
        bus.publish("other", "tick", json!(null));
    }

    #[test]
    fn sender_for_missing_plugin_returns_none() {
        let bus = PluginBus::new();
        assert!(bus.sender_for("nonexistent").is_none());
    }

    #[tokio::test]
    async fn query_routes_to_target() {
        let bus = PluginBus::new();
        let mut rx = bus.register_plugin("target");

        // Spawn a handler that responds to queries.
        let handle = tokio::spawn(async move {
            if let Some(PluginMail::BusQuery(req)) = rx.recv().await {
                assert_eq!(req.method, "ping");
                let _ = req.reply.send(QueryResponse {
                    result: Ok(json!("pong")),
                });
            }
        });

        let resp = bus
            .query("target", "ping", json!(null), "caller")
            .await
            .unwrap();
        assert_eq!(resp.result.unwrap(), json!("pong"));
        handle.await.unwrap();
    }

    #[tokio::test]
    async fn query_missing_plugin_returns_error() {
        let bus = PluginBus::new();
        let err = bus
            .query("ghost", "method", json!(null), "src")
            .await
            .unwrap_err();
        assert!(matches!(err, BusError::PluginNotFound(_)));
    }

    #[test]
    fn render_request_via_sender() {
        let bus = PluginBus::new();
        let mut rx = bus.register_plugin("panel");
        let sender = bus.sender_for("panel").unwrap();

        let (reply_tx, reply_rx) = oneshot::channel();
        sender
            .try_send(PluginMail::RenderRequest { reply: reply_tx })
            .unwrap();

        match rx.try_recv().unwrap() {
            PluginMail::RenderRequest { reply } => {
                reply.send("[{\"type\":\"label\"}]".to_string()).unwrap();
            }
            _ => panic!("expected RenderRequest"),
        }

        assert_eq!(reply_rx.blocking_recv().unwrap(), "[{\"type\":\"label\"}]");
    }

    #[test]
    fn shutdown_via_sender() {
        let bus = PluginBus::new();
        let mut rx = bus.register_plugin("plugin");
        let sender = bus.sender_for("plugin").unwrap();

        sender.try_send(PluginMail::Shutdown).unwrap();
        assert!(matches!(rx.try_recv().unwrap(), PluginMail::Shutdown));
    }
}
