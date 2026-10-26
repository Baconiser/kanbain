use axum::extract::ws::Message as AxumWsMessage;
use tokio_tungstenite::tungstenite;

/// Convert an axum WS message to a tungstenite WS message, preserving
/// the message type (text, binary, ping, pong, close).
pub fn axum_to_tungstenite(msg: AxumWsMessage) -> tungstenite::Message {
    match msg {
        AxumWsMessage::Text(text) => tungstenite::Message::Text(text.to_string().into()),
        AxumWsMessage::Binary(bytes) => tungstenite::Message::Binary(bytes.to_vec().into()),
        AxumWsMessage::Ping(bytes) => tungstenite::Message::Ping(bytes.to_vec().into()),
        AxumWsMessage::Pong(bytes) => tungstenite::Message::Pong(bytes.to_vec().into()),
        AxumWsMessage::Close(frame) => {
            tungstenite::Message::Close(frame.map(|cf| tungstenite::protocol::CloseFrame {
                code: tungstenite::protocol::frame::coding::CloseCode::from(cf.code),
                reason: cf.reason.to_string().into(),
            }))
        }
    }
}

/// Convert a tungstenite WS message to an axum WS message, preserving
/// the message type (text, binary, ping, pong, close).
pub fn tungstenite_to_axum(msg: tungstenite::Message) -> AxumWsMessage {
    match msg {
        tungstenite::Message::Text(text) => AxumWsMessage::Text(text.to_string().into()),
        tungstenite::Message::Binary(bytes) => AxumWsMessage::Binary(bytes.to_vec().into()),
        tungstenite::Message::Ping(bytes) => AxumWsMessage::Ping(bytes.to_vec().into()),
        tungstenite::Message::Pong(bytes) => AxumWsMessage::Pong(bytes.to_vec().into()),
        tungstenite::Message::Close(frame) => {
            AxumWsMessage::Close(frame.map(|cf| axum::extract::ws::CloseFrame {
                code: cf.code.into(),
                reason: cf.reason.to_string().into(),
            }))
        }
        tungstenite::Message::Frame(_) => AxumWsMessage::Binary(vec![].into()),
    }
}
