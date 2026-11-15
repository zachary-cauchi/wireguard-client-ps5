use tokio::sync::broadcast::{self, Receiver, Sender};

use crate::Result;

/// A broadcast-based event bus for relaying network packets or events.
#[derive(Clone)]
pub struct EventBus {
    sender: Sender<Event>,
}

impl EventBus {
    pub fn new() -> Self {
        let (sender, _) = broadcast::channel(32);

        Self { sender }
    }

    pub fn subscribe(&self) -> Receiver<Event> {
        self.sender.subscribe()
    }

    pub fn send(&self, event: Event) -> Result<()> {
        self.sender.send(event)?;
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub enum Event {
    SendRawPacket(Vec<u8>),
    ReceivedPacket(Vec<u8>),
}
