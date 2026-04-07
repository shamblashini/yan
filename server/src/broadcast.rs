use tokio::sync::broadcast;
use yan_shared::ops::Operation;

/// In-memory broadcast channel for pushing new ops to connected WebSocket clients.
pub struct Hub {
    tx: broadcast::Sender<Vec<Operation>>,
}

impl Hub {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(256);
        Self { tx }
    }

    /// Subscribe to incoming op broadcasts (one receiver per WebSocket connection).
    pub fn subscribe(&self) -> broadcast::Receiver<Vec<Operation>> {
        self.tx.subscribe()
    }

    /// Publish new ops to all connected WebSocket subscribers.
    /// Errors (no active subscribers) are silently ignored.
    pub fn publish(&self, ops: Vec<Operation>) {
        let _ = self.tx.send(ops);
    }
}
