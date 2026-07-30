//! Latency is seconds, so the board is pushed rather than polled.

use crate::store::SharedBoard;

use axum::extract::State as AxState;
use axum::response::sse::{Event, KeepAlive, Sse};
use futures::stream::Stream;
use std::convert::Infallible;
use std::time::Duration;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;

pub async fn events(
    AxState(b): AxState<SharedBoard>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let stream = BroadcastStream::new(b.subscribe()).filter_map(|msg| {
        // A lagged receiver means the client fell behind a burst. Dropping the
        // gap is fine: every event is a full upsert, so the next one for that
        // card re-syncs it, and the client can always re-fetch /api/board.
        let ev = msg.ok()?;
        Event::default().json_data(&ev).ok().map(Ok)
    });

    Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(15)).text("keep-alive"))
}
