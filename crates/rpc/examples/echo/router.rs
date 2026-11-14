//! Test methods served by the echo peer. Shared by `examples/echo/main.rs`
//! (the child process) and `tests/suite.rs` (in-process peers).

#![allow(dead_code)]

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use lsdocs_protocol::{Notification, Request};
use lsdocs_rpc::{Router, RpcError};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::mpsc::UnboundedSender;

macro_rules! request {
    ($name:ident = $method:literal, $params:ty => $result:ty) => {
        pub enum $name {}
        impl Request for $name {
            const METHOD: &'static str = $method;
            type Params = $params;
            type Result = $result;
        }
    };
}

macro_rules! notification {
    ($name:ident = $method:literal, $params:ty) => {
        pub enum $name {}
        impl Notification for $name {
            const METHOD: &'static str = $method;
            type Params = $params;
        }
    };
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SleepParams {
    pub ms: u64,
    pub tag: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Stats {
    /// Sleep handlers that finished.
    pub completed: u32,
    /// Sleep handlers dropped before finishing (cancelled).
    pub cancelled: u32,
}

request!(Echo = "test/echo", Value => Value);
request!(Ping = "test/ping", () => ());
request!(Sleep = "test/sleep", SleepParams => String);
request!(Double = "test/double", u32 => u32);
request!(Fail = "test/fail", String => ());
request!(CallBack = "test/call_back", Value => Value);
request!(GetStats = "test/stats", () => Stats);
request!(CloseMe = "test/close_me", () => ());
notification!(NotifyMe = "test/notify_me", String);
notification!(Notified = "test/notified", String);

#[derive(Default)]
struct Counters {
    completed: AtomicU32,
    cancelled: AtomicU32,
}

/// Counts a sleep as cancelled unless it is marked complete before drop.
struct SleepGuard {
    counters: Arc<Counters>,
    done: bool,
}

impl Drop for SleepGuard {
    fn drop(&mut self) {
        let counter = if self.done {
            &self.counters.completed
        } else {
            &self.counters.cancelled
        };
        counter.fetch_add(1, Ordering::SeqCst);
    }
}

/// The echo router. Notifications of `test/notified` received by this peer
/// are forwarded to `notified`, if given.
pub fn router(notified: Option<UnboundedSender<String>>) -> Router {
    let counters = Arc::new(Counters::default());
    let sleep_counters = counters.clone();
    Router::new()
        .request::<Echo, _, _>(|value, _| async move { Ok(value) })
        .request::<Ping, _, _>(|(), _| async { Ok(()) })
        .request::<Sleep, _, _>(move |params, _| {
            let counters = sleep_counters.clone();
            async move {
                let mut guard = SleepGuard {
                    counters,
                    done: false,
                };
                tokio::time::sleep(Duration::from_millis(params.ms)).await;
                guard.done = true;
                Ok(params.tag)
            }
        })
        .request::<Double, _, _>(|n, _| async move { Ok(n * 2) })
        .request::<Fail, _, _>(|message, _| async move { Err(RpcError::internal(message)) })
        .request::<CallBack, _, _>(|value, peer| async move {
            peer.request::<Echo>(&value)
                .await
                .map_err(|e| RpcError::internal(e.to_string()))
        })
        .request::<GetStats, _, _>(move |(), _| {
            let counters = counters.clone();
            async move {
                Ok(Stats {
                    completed: counters.completed.load(Ordering::SeqCst),
                    cancelled: counters.cancelled.load(Ordering::SeqCst),
                })
            }
        })
        // Closes this side; the response is still sent before shutdown.
        .request::<CloseMe, _, _>(|(), peer| async move {
            peer.close();
            Ok(())
        })
        .notification::<NotifyMe, _>(|message, peer| {
            let peer = peer.clone();
            tokio::spawn(async move {
                let _ = peer.notify::<Notified>(&message).await;
            });
        })
        .notification::<Notified, _>(move |message, _| {
            if let Some(tx) = &notified {
                let _ = tx.send(message);
            }
        })
}
