use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};

use tokio::time::{Instant, Sleep, sleep};

pub(super) const INTERVAL: Duration = Duration::from_secs(15);
const MAX_CONSECUTIVE_PINGS: u8 = 30;
pub(super) const PING: &[u8] = b"event: ping\ndata: {\"type\":\"ping\"}\n\n";

pub(super) fn is_gemini_model(model: &str) -> bool {
    model.trim().split('/').any(|part| {
        part.get(..7)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("gemini-"))
    })
}

pub(super) struct KeepAlive {
    interval: Duration,
    deadline: Pin<Box<Sleep>>,
    tail: [u8; 4],
    at_boundary: bool,
    pings: u8,
}

impl KeepAlive {
    pub(super) fn new(interval: Duration) -> Self {
        let interval = interval.max(Duration::from_millis(10));
        Self {
            interval,
            deadline: Box::pin(sleep(interval)),
            tail: [0; 4],
            at_boundary: true,
            pings: 0,
        }
    }

    pub(super) fn observe(&mut self, data: &[u8]) {
        if data.is_empty() {
            return;
        }
        self.pings = 0;
        let tail = &data[data.len().saturating_sub(4)..];
        self.tail.rotate_left(tail.len());
        self.tail[4 - tail.len()..].copy_from_slice(tail);
        self.at_boundary = self.tail.ends_with(b"\n\n") || self.tail == *b"\r\n\r\n";
        self.restart_timer();
    }

    pub(super) fn poll_ping(&mut self, cx: &mut Context<'_>) -> Poll<()> {
        if self.pings >= MAX_CONSECUTIVE_PINGS || !self.at_boundary {
            return Poll::Pending;
        }
        std::task::ready!(self.deadline.as_mut().poll(cx));
        self.pings += 1;
        self.restart_timer();
        Poll::Ready(())
    }

    fn restart_timer(&mut self) {
        self.deadline.as_mut().reset(Instant::now() + self.interval);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_only_gemini_model_segments() {
        for model in [
            "gemini-4-neon-high",
            "matou-antigravity/gemini-4-neon-high",
            " google/GEMINI-2.5-pro ",
        ] {
            assert!(is_gemini_model(model), "{model}");
        }
        for model in [
            "",
            "deepseek-flash",
            "claude-opus-5-5-high",
            "matou-antigravity/claude-opus-5-5[1M]",
            "gpt-6.2-sol",
            "matou-chatgpt/gpt-6.2-sol",
            "nongemini-2.5-pro",
        ] {
            assert!(!is_gemini_model(model), "{model}");
        }
    }
}
