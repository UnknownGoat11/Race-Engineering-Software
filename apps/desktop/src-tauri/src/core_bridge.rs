//! Bridge between `app-core` and the WebView.
//!
//! * [`TauriUiSink`] is the app-wide sink handed to `AppCore`: it emits the JSON `core:*` events and remembers
//!   the latest `core:status` the driver pipeline produced (used by `get_core_status` and the close guard).
//! * [`ChannelSink`] is one sink per `bridge_subscribe` call. It owns the subscription's Tauri channel, so the
//!   hub can deliver the `RESET | BACKFILL` batches synchronously inside `UiHub::subscribe` (before the
//!   subscription id is even known to the command). Registering the channel by id after `subscribe` returned
//!   would lose those batches and silently drop the subscription.

use std::sync::Mutex;
use std::sync::PoisonError;

use serde::Serialize;
use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::{AppHandle, Emitter};

use app_core::bridge::sink::{SinkClosed, SubscriptionId, UiSink};
use app_core::bridge::status::{CoreStatus, Origin, UiEvent};

/// Tracing component of this module.
const COMPONENT: &str = "desktop.bridge";

/// Response of `bridge_subscribe`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BridgeSubscribeResponse {
    pub subscription_id: u64,
    /// Provenance of the data the subscription will carry (`synthetic` while the demo runs). The UI labels
    /// synthetic data from this value and from `core:status.origin` (DEFERRED #18).
    pub origin: Origin,
}

/// App-wide sink: JSON events only. Batches always go through a per-subscription [`ChannelSink`].
pub struct TauriUiSink {
    app_handle: Option<AppHandle>,
    last_status: Mutex<Option<CoreStatus>>,
}

impl TauriUiSink {
    pub fn new(app_handle: AppHandle) -> Self {
        Self {
            app_handle: Some(app_handle),
            last_status: Mutex::new(None),
        }
    }

    /// A sink without a WebView (unit tests): events are only cached.
    pub fn headless() -> Self {
        Self {
            app_handle: None,
            last_status: Mutex::new(None),
        }
    }

    /// The most recent `core:status` emitted by a pipeline, if any.
    pub fn last_status(&self) -> Option<CoreStatus> {
        self.last_status
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Forget the cached status (the pipeline that produced it stopped).
    pub fn clear_status(&self) {
        *self
            .last_status
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = None;
    }
}

impl UiSink for TauriUiSink {
    fn send_batch(&self, sub: SubscriptionId, _batch: Vec<u8>) -> Result<(), SinkClosed> {
        // Never registered as a hub subscriber; refusing makes a wiring mistake visible instead of
        // silently swallowing data.
        tracing::warn!(component = COMPONENT, sub = %sub, "batch sent to the event sink; dropping subscription");
        Err(SinkClosed)
    }

    fn emit(&self, event: UiEvent) {
        if let UiEvent::Status(status) = &event {
            *self
                .last_status
                .lock()
                .unwrap_or_else(PoisonError::into_inner) = Some(status.as_ref().clone());
        }
        let Some(ref handle) = self.app_handle else {
            return;
        };
        let event_name = event.event_name();
        let res = match &event {
            UiEvent::Status(status) => handle.emit(event_name, status),
            UiEvent::Scoring(val)
            | UiEvent::SessionEvent(val)
            | UiEvent::Capabilities(val)
            | UiEvent::EngineState(val) => handle.emit(event_name, val),
            UiEvent::Alert(alert) => handle.emit(event_name, alert),
            UiEvent::Engineer(eng) => handle.emit(event_name, eng),
            UiEvent::Error(err) => handle.emit(event_name, err),
        };
        if let Err(e) = res {
            tracing::warn!(component = COMPONENT, event = event_name, error = %e, "failed to emit UI event");
        }
    }
}

/// Where a [`ChannelSink`] delivers raw batches. Abstracted so the sink logic is testable without a WebView.
pub trait BatchChannel: Send + Sync + 'static {
    /// Deliver one encoded `UiBatch`; an error means the WebView side is gone.
    fn deliver(&self, batch: Vec<u8>) -> Result<(), String>;
}

impl BatchChannel for Channel<InvokeResponseBody> {
    fn deliver(&self, batch: Vec<u8>) -> Result<(), String> {
        self.send(InvokeResponseBody::Raw(batch))
            .map_err(|e| e.to_string())
    }
}

/// The sink of exactly one hub subscription.
pub struct ChannelSink<C: BatchChannel> {
    channel: C,
}

impl<C: BatchChannel> ChannelSink<C> {
    pub fn new(channel: C) -> Self {
        Self { channel }
    }
}

impl<C: BatchChannel> UiSink for ChannelSink<C> {
    fn send_batch(&self, sub: SubscriptionId, batch: Vec<u8>) -> Result<(), SinkClosed> {
        self.channel.deliver(batch).map_err(|error| {
            tracing::debug!(component = COMPONENT, sub = %sub, error = %error, "batch channel closed");
            SinkClosed
        })
    }

    fn emit(&self, _event: UiEvent) {
        // Events go through the app-wide `TauriUiSink`; the hub never emits on subscription sinks.
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, Ordering};

    use app_core::bridge::hub::UiHub;

    use super::*;

    #[derive(Default)]
    struct RecordingChannel {
        batches: Mutex<Vec<Vec<u8>>>,
        closed: AtomicBool,
    }

    impl BatchChannel for Arc<RecordingChannel> {
        fn deliver(&self, batch: Vec<u8>) -> Result<(), String> {
            if self.closed.load(Ordering::SeqCst) {
                return Err("closed".to_owned());
            }
            self.batches.lock().expect("lock").push(batch);
            Ok(())
        }
    }

    /// A valid canonical snapshot TLV block.
    fn block() -> Arc<[u8]> {
        let frame = telemetry_model::frame::TelemetryFrame::new(0, 1_000_000_000, Some(0.0));
        let mut buf = Vec::new();
        frame
            .encode_snapshot(&mut buf, telemetry_model::codec::RELAY_NS_UNKNOWN)
            .expect("snapshot encode");
        Arc::from(buf)
    }

    #[test]
    fn backfill_is_delivered_during_subscribe_and_subscription_survives() {
        let hub = UiHub::new();
        hub.push_block(block(), true);
        hub.push_block(block(), false);

        let channel = Arc::new(RecordingChannel::default());
        let sink = Arc::new(ChannelSink::new(Arc::clone(&channel)));
        let sub = hub.subscribe(sink);

        assert_eq!(
            hub.subscription_count(),
            1,
            "subscription must not be dropped"
        );
        assert!(hub.subscription_dropped(sub).is_some());
        assert!(
            !channel.batches.lock().expect("lock").is_empty(),
            "RESET|BACKFILL batch must reach the channel"
        );

        hub.push_block(block(), false);
        hub.flush_subscription(sub).expect("flush live");
        assert!(channel.batches.lock().expect("lock").len() >= 2);
    }

    #[test]
    fn closed_channel_drops_the_subscription() {
        let hub = UiHub::new();
        let channel = Arc::new(RecordingChannel::default());
        let sub = hub.subscribe(Arc::new(ChannelSink::new(Arc::clone(&channel))));
        channel.closed.store(true, Ordering::SeqCst);
        hub.push_block(block(), true);
        assert_eq!(hub.flush_subscription(sub), Err(SinkClosed));
        assert_eq!(hub.subscription_count(), 0);
    }

    #[test]
    fn event_sink_refuses_batches_and_caches_status() {
        let sink = TauriUiSink::headless();
        assert_eq!(sink.send_batch(SubscriptionId(1), vec![1]), Err(SinkClosed));
        assert!(sink.last_status().is_none());
    }
}
