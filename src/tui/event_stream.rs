//! Event handling for the terminal UI.
//!
//! This module combines input events (keyboard, resize, etc.) with
//! application-generated draw events into a single stream that the
//! main loop can process.
//!
//! Components:
//! - `EventBroker`: Manages access to the underlying terminal input stream,
//!   allowing it to be paused and resumed (useful when handing off control
//!   to external programs like editors)
//! - `TuiEventStream`: The main event stream that yields keyboard events,
//!   draw requests, and other terminal events

use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::task::Context;
use std::task::Poll;

use crossterm::event::Event;
use tokio::sync::broadcast;
use tokio::sync::watch;
use tokio_stream::Stream;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::wrappers::WatchStream;
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;

use super::TuiEvent;

/// Result type for terminal events.
pub type TuiEventResult = std::io::Result<Event>;

/// Abstraction over terminal event sources.
///
/// Allows substituting a mock implementation for testing.
pub trait TuiEventSource: Send + 'static {
  fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<TuiEventResult>>;
}

/// Shared manager for the terminal input stream.
///
/// Multiple parts of the app may need to read input, but crossterm only
/// provides a single global stream. This broker mediates access and allows
/// the stream to be temporarily disabled (paused) when needed.
pub struct TuiEventBroker<S: TuiEventSource = CrosstermEventSource> {
  state: Mutex<TuiEventBrokerState<S>>,
  resume_events_tx: watch::Sender<()>,
}

/// Internal state tracking whether the input stream is active.
#[allow(dead_code)]
enum TuiEventBrokerState<S: TuiEventSource> {
  Paused,     // Stream is disabled
  Start,      // Will create new stream on next use
  Running(S), // Stream is active and being polled
}

impl<S: TuiEventSource + Default> TuiEventBrokerState<S> {
  /// Get the active stream, initializing it if necessary.
  fn active_event_source_mut(&mut self) -> Option<&mut S> {
    match self {
      TuiEventBrokerState::Paused => None,
      TuiEventBrokerState::Start => {
        *self = TuiEventBrokerState::Running(S::default());
        match self {
          TuiEventBrokerState::Running(events) => Some(events),
          TuiEventBrokerState::Paused | TuiEventBrokerState::Start => unreachable!(),
        }
      }
      TuiEventBrokerState::Running(events) => Some(events),
    }
  }
}

impl<S: TuiEventSource + Default> TuiEventBroker<S> {
  pub fn new() -> Self {
    let (resume_events_tx, _resume_events_rx) = watch::channel(());
    Self {
      state: Mutex::new(TuiEventBrokerState::Start),
      resume_events_tx,
    }
  }

  /// Get a signal that fires when the stream is resumed.
  pub fn resume_events_rx(&self) -> watch::Receiver<()> {
    self.resume_events_tx.subscribe()
  }
}

impl<S: TuiEventSource + Default> Default for TuiEventBroker<S> {
  fn default() -> Self {
    Self::new()
  }
}

/// Real terminal input using crossterm.
pub struct CrosstermEventSource(pub crossterm::event::EventStream);

impl Default for CrosstermEventSource {
  fn default() -> Self {
    Self(crossterm::event::EventStream::new())
  }
}

impl TuiEventSource for CrosstermEventSource {
  fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<TuiEventResult>> {
    Pin::new(&mut self.get_mut().0).poll_next(cx)
  }
}

/// Main event stream combining terminal input and draw requests.
///
/// Yields `TuiEvent::Key` for keyboard input, `TuiEvent::Draw` for
/// redraw requests, and `TuiEvent::Paste` for clipboard pastes.
/// Multiple instances can exist but only one should be polled at a time.
pub struct TuiEventStream<S: TuiEventSource + Default + Unpin = CrosstermEventSource> {
  broker: Arc<TuiEventBroker<S>>,
  draw_stream: BroadcastStream<()>,
  resume_stream: WatchStream<()>,
  terminal_focused: Arc<AtomicBool>,
  poll_draw_first: bool,
}

impl<S: TuiEventSource + Default + Unpin> TuiEventStream<S> {
  pub fn new(
    broker: Arc<TuiEventBroker<S>>,
    draw_rx: broadcast::Receiver<()>,
    terminal_focused: Arc<AtomicBool>,
  ) -> Self {
    let resume_stream = WatchStream::from_changes(broker.resume_events_rx());
    Self {
      broker,
      draw_stream: BroadcastStream::new(draw_rx),
      resume_stream,
      terminal_focused,
      poll_draw_first: false,
    }
  }

  /// Poll for terminal input events.
  fn poll_crossterm_event(&mut self, cx: &mut Context<'_>) -> Poll<Option<TuiEvent>> {
    loop {
      let poll_result = {
        let mut state = self
          .broker
          .state
          .lock()
          .unwrap_or_else(std::sync::PoisonError::into_inner);
        let events = match state.active_event_source_mut() {
          Some(events) => events,
          None => {
            drop(state);
            // Wait for resume signal while paused
            match Pin::new(&mut self.resume_stream).poll_next(cx) {
              Poll::Ready(Some(())) => continue,
              Poll::Ready(None) => return Poll::Ready(None),
              Poll::Pending => return Poll::Pending,
            }
          }
        };
        match Pin::new(events).poll_next(cx) {
          Poll::Ready(Some(Ok(event))) => Some(event),
          Poll::Ready(Some(Err(_))) | Poll::Ready(None) => {
            *state = TuiEventBrokerState::Start;
            return Poll::Ready(None);
          }
          Poll::Pending => {
            drop(state);
            // Also check for resume while waiting for input
            match Pin::new(&mut self.resume_stream).poll_next(cx) {
              Poll::Ready(Some(())) => continue,
              Poll::Ready(None) => return Poll::Ready(None),
              Poll::Pending => return Poll::Pending,
            }
          }
        }
      };

      if let Some(mapped) = poll_result.and_then(|event| self.map_crossterm_event(event)) {
        return Poll::Ready(Some(mapped));
      }
    }
  }

  /// Poll for draw notification events.
  fn poll_draw_event(&mut self, cx: &mut Context<'_>) -> Poll<Option<TuiEvent>> {
    match Pin::new(&mut self.draw_stream).poll_next(cx) {
      Poll::Ready(Some(Ok(()))) => Poll::Ready(Some(TuiEvent::Draw)),
      Poll::Ready(Some(Err(BroadcastStreamRecvError::Lagged(_)))) => {
        Poll::Ready(Some(TuiEvent::Draw))
      }
      Poll::Ready(None) => Poll::Ready(None),
      Poll::Pending => Poll::Pending,
    }
  }

  /// Convert crossterm events to our event type, filtering out unused ones.
  fn map_crossterm_event(&mut self, event: Event) -> Option<TuiEvent> {
    match event {
      Event::Key(key_event) => Some(TuiEvent::Key(key_event)),
      Event::Resize(_, _) => Some(TuiEvent::Draw),
      Event::Paste(pasted) => Some(TuiEvent::Paste(pasted)),
      Event::FocusGained => {
        self.terminal_focused.store(true, Ordering::Relaxed);
        Some(TuiEvent::Draw)
      }
      Event::FocusLost => {
        self.terminal_focused.store(false, Ordering::Relaxed);
        None
      }
      _ => None,
    }
  }
}

impl<S: TuiEventSource + Default + Unpin> Unpin for TuiEventStream<S> {}

impl<S: TuiEventSource + Default + Unpin> Stream for TuiEventStream<S> {
  type Item = TuiEvent;

  fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
    // Alternate between checking draw and input to ensure fairness
    let draw_first = self.poll_draw_first;
    self.poll_draw_first = !self.poll_draw_first;

    if draw_first {
      if let Poll::Ready(event) = self.poll_draw_event(cx) {
        return Poll::Ready(event);
      }
      if let Poll::Ready(event) = self.poll_crossterm_event(cx) {
        return Poll::Ready(event);
      }
    } else {
      if let Poll::Ready(event) = self.poll_crossterm_event(cx) {
        return Poll::Ready(event);
      }
      if let Poll::Ready(event) = self.poll_draw_event(cx) {
        return Poll::Ready(event);
      }
    }

    Poll::Pending
  }
}
