use std::{
    borrow::Cow,
    pin::Pin,
    task::{Context, Poll},
};

use axum::{
    extract::{
        FromRequestParts,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    http::request::Parts,
    response::IntoResponse,
};
use futures_util::{Sink, SinkExt, Stream, StreamExt};

/// WebSocket upgrade extractor used by all streaming routes.
pub struct WsUpgrade {
    ws: WebSocketUpgrade,
}

impl<S> FromRequestParts<S> for WsUpgrade
where
    S: Send + Sync,
{
    type Rejection = axum::extract::ws::rejection::WebSocketUpgradeRejection;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let ws = WebSocketUpgrade::from_request_parts(parts, state).await?;
        Ok(Self { ws })
    }
}

impl WsUpgrade {
    pub fn protocols<I>(mut self, protocols: I) -> Self
    where
        I: IntoIterator,
        I::Item: Into<Cow<'static, str>>,
    {
        self.ws = self.ws.protocols(protocols);
        self
    }

    pub fn on_upgrade<F, Fut>(self, callback: F) -> impl IntoResponse
    where
        F: FnOnce(AppWebSocket) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = ()> + Send + 'static,
    {
        self.ws.on_upgrade(move |socket| async move {
            callback(AppWebSocket {
                inner: Box::new(socket),
            })
            .await;
        })
    }
}

pub struct AppWebSocket {
    inner: Box<WebSocket>,
}

impl AppWebSocket {
    pub async fn send(&mut self, message: Message) -> anyhow::Result<()> {
        SinkExt::send(&mut self.inner, message)
            .await
            .map_err(anyhow::Error::from)
    }

    pub async fn recv(&mut self) -> anyhow::Result<Option<Message>> {
        match self.inner.next().await {
            Some(Ok(msg)) => Ok(Some(msg)),
            Some(Err(e)) => Err(anyhow::Error::from(e)),
            None => Ok(None),
        }
    }

    pub async fn close(&mut self) -> anyhow::Result<()> {
        SinkExt::close(&mut self.inner)
            .await
            .map_err(anyhow::Error::from)
    }
}

impl Stream for AppWebSocket {
    type Item = Result<Message, anyhow::Error>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.get_mut().inner)
            .poll_next(cx)
            .map(|opt| opt.map(|r| r.map_err(anyhow::Error::from)))
    }
}

impl Sink<Message> for AppWebSocket {
    type Error = anyhow::Error;

    fn poll_ready(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Pin::new(&mut self.get_mut().inner)
            .poll_ready(cx)
            .map_err(anyhow::Error::from)
    }

    fn start_send(self: Pin<&mut Self>, item: Message) -> Result<(), Self::Error> {
        Pin::new(&mut self.get_mut().inner)
            .start_send(item)
            .map_err(anyhow::Error::from)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Pin::new(&mut self.get_mut().inner)
            .poll_flush(cx)
            .map_err(anyhow::Error::from)
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Pin::new(&mut self.get_mut().inner)
            .poll_close(cx)
            .map_err(anyhow::Error::from)
    }
}
