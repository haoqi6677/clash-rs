use std::{net::SocketAddr, sync::Arc};

use axum::{
    extract::{ws::Message, ConnectInfo, State, WebSocketUpgrade},
    response::IntoResponse,
    Json,
};

use hyper::body::HttpBody;
use tracing::warn;

use crate::app::api::AppState;

pub async fn handle(
    ws: WebSocketUpgrade,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    ws.on_failed_upgrade(move |e| {
        warn!("ws upgrade error: {} with {}", e, addr);
    })
    .on_upgrade(move |mut socket| async move {
        let mut rx = state.log_source_tx.subscribe();
        while let Ok(evt) = rx.recv().await {
            let res = Json(evt).into_response().data().await.unwrap().unwrap();

            if let Err(e) = socket
                .send(Message::Text(String::from_utf8(res.to_vec()).unwrap()))
                .await
            {
                warn!("ws send error: {}", e);
                break;
            }
        }
    })
}
