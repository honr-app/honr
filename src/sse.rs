//! Latency is seconds, so the board is pushed rather than polled.

use crate::store::SharedBoard;

use axum::extract::State as AxState;
use axum::http::{header, HeaderValue};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use futures::stream::{self, StreamExt};
use std::convert::Infallible;
use std::time::Duration;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt as TokioStreamExt;

pub async fn events(AxState(b): AxState<SharedBoard>) -> Response {
    // Emit a comment frame immediately so proxies (Vite in particular) flush
    // response headers and the browser's EventSource fires `onopen` without
    // waiting for the first keep-alive (~15s) or a real board event.
    let hello = stream::once(async {
        Ok::<_, Infallible>(Event::default().comment("connected"))
    });

    let live = TokioStreamExt::filter_map(BroadcastStream::new(b.subscribe()), |msg| {
        // A lagged receiver means the client fell behind a burst. Dropping the
        // gap is fine: every event is a full upsert, so the next one for that
        // card re-syncs it, and the client can always re-fetch /api/board.
        let ev = msg.ok()?;
        Event::default().json_data(&ev).ok().map(Ok)
    });

    let sse = Sse::new(StreamExt::chain(hello, live)).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("keep-alive"),
    );

    let mut res = sse.into_response();
    // Hint reverse proxies not to buffer the event stream.
    res.headers_mut().insert(
        header::HeaderName::from_static("x-accel-buffering"),
        HeaderValue::from_static("no"),
    );
    res
}
