//! Frame scheduling system for the TUI.
//!
//! This module provides a way for any part of the application to request
//! screen redraws, either immediately or after a delay. This is essential
//! for smooth animations without wasting CPU on unnecessary renders.
//!
//! The design uses a producer-consumer pattern:
//! - `FrameRequester` is a lightweight handle that can be cloned and passed around
//! - `FrameScheduler` is a background task that collects all requests and
//!   emits draw notifications at appropriate times
//!
//! Multiple draw requests are merged into a single notification to avoid
//! redundant rendering, and output is capped at 120 FPS.

use std::time::Duration;
use std::time::Instant;

use tokio::sync::broadcast;
use tokio::sync::mpsc;
use tokio::time::sleep_until;

use crate::utils::time::ONE_YEAR;

use super::frame_rate_limiter::FrameRateLimiter;

/// A handle for requesting screen redraws from anywhere in the app.
///
/// Clone this handle to pass it to widgets or background tasks that need
/// to trigger updates (e.g., for animations or async data loading).
#[derive(Clone, Debug)]
pub struct FrameRequester {
  frame_schedule_tx: mpsc::UnboundedSender<Instant>,
}

impl FrameRequester {
  /// Create a new requester and start the background scheduler.
  ///
  /// The `draw_tx` channel is used to notify the main event loop when
  /// a redraw should happen.
  pub fn new(draw_tx: broadcast::Sender<()>) -> Self {
    let (tx, rx) = mpsc::unbounded_channel();
    let scheduler = FrameScheduler::new(rx, draw_tx);
    tokio::spawn(scheduler.run());
    Self {
      frame_schedule_tx: tx,
    }
  }

  /// Request an immediate redraw.
  #[allow(dead_code)]
  pub fn schedule_frame(&self) {
    let _ = self.frame_schedule_tx.send(Instant::now());
  }

  /// Request a redraw after the specified delay.
  pub fn schedule_frame_in(&self, dur: Duration) {
    let _ = self.frame_schedule_tx.send(Instant::now() + dur);
  }
}

#[cfg(test)]
impl FrameRequester {
  /// Create a stub requester for testing that does nothing.
  #[allow(dead_code)]
  pub fn test_dummy() -> Self {
    let (tx, _rx) = mpsc::unbounded_channel();
    FrameRequester {
      frame_schedule_tx: tx,
    }
  }
}

/// Background task that processes frame requests and notifies the event loop.
///
/// This runs as a separate tokio task. It collects multiple requests and
/// batches them into single draw notifications to avoid redundant renders.
/// Frame rate is limited to 120 FPS to prevent excessive CPU usage.
struct FrameScheduler {
  receiver: mpsc::UnboundedReceiver<Instant>,
  draw_tx: broadcast::Sender<()>,
  rate_limiter: FrameRateLimiter,
}

impl FrameScheduler {
  fn new(receiver: mpsc::UnboundedReceiver<Instant>, draw_tx: broadcast::Sender<()>) -> Self {
    Self {
      receiver,
      draw_tx,
      rate_limiter: FrameRateLimiter::default(),
    }
  }

  /// Main loop: wait for requests and emit draw notifications.
  ///
  /// Runs until all requesters are dropped. Multiple requests before
  /// a deadline are merged into a single notification.
  async fn run(mut self) {
    let mut next_deadline: Option<Instant> = None;
    loop {
      let target = next_deadline.unwrap_or_else(|| Instant::now() + ONE_YEAR);
      let deadline = sleep_until(target.into());
      tokio::pin!(deadline);

      tokio::select! {
        draw_at = self.receiver.recv() => {
          let Some(draw_at) = draw_at else {
            // All requesters dropped; shut down.
            break;
          };
          let draw_at = self.rate_limiter.clamp_deadline(draw_at);
          next_deadline = Some(next_deadline.map_or(draw_at, |cur| cur.min(draw_at)));

          // Don't draw immediately - wait until the deadline to batch
          // multiple requests together into a single notification.
          continue;
        }
        _ = &mut deadline => {
          if next_deadline.is_some() {
            next_deadline = None;
            self.rate_limiter.mark_emitted(target);
            let _ = self.draw_tx.send(());
          }
        }
      }
    }
  }
}
