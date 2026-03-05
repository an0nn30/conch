use alacritty_terminal::event::{Event as TermEvent, EventListener};
use tokio::sync::mpsc;

/// Bridges alacritty_terminal events into an async channel for iced consumption.
#[derive(Clone)]
pub struct EventProxy {
    sender: mpsc::UnboundedSender<TermEvent>,
}

impl EventProxy {
    pub fn new() -> (Self, mpsc::UnboundedReceiver<TermEvent>) {
        let (sender, receiver) = mpsc::unbounded_channel();
        (Self { sender }, receiver)
    }
}

impl EventListener for EventProxy {
    fn send_event(&self, event: TermEvent) {
        let _ = self.sender.send(event);
    }
}
