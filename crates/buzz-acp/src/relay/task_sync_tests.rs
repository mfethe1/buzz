use super::*;

/// Drive the production connector against a real localhost WebSocket peer.
/// The peer checks the signed NIP-42 response before sending a control frame
/// ahead of the authentication OK, matching the relay's priority queue ordering.
async fn connect_with_control_before_ok(
    control: Value,
) -> (
    Result<(WsStream, VecDeque<RelayMessage>), RelayError>,
    WebSocketStream<tokio::net::TcpStream>,
    Keys,
) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind local auth peer");
    let relay_url = format!("ws://{}", listener.local_addr().expect("local address"));
    let keys = Keys::generate();
    let challenge = "task-sync-auth-challenge";
    let peer = async {
        let (stream, _) = listener.accept().await.expect("accept client");
        let mut ws = tokio_tungstenite::accept_async(stream)
            .await
            .expect("upgrade WebSocket");
        ws.send(Message::Text(json!(["AUTH", challenge]).to_string().into()))
            .await
            .expect("send challenge");
        let auth = ws
            .next()
            .await
            .expect("client auth frame")
            .expect("read auth");
        let auth: Value =
            serde_json::from_str(auth.to_text().expect("text auth")).expect("parse auth envelope");
        assert_eq!(auth[0], "AUTH");
        let event: Event = serde_json::from_value(auth[1].clone()).expect("signed auth event");
        buzz_core::verify_event(&event).expect("valid NIP-42 signature");
        assert_eq!(event.kind, Kind::Authentication);
        assert_eq!(event.pubkey, keys.public_key());
        let tags = auth[1]["tags"].as_array().expect("auth tags");
        assert!(tags.contains(&json!(["challenge", challenge])));
        assert!(tags.contains(&json!(["relay", relay_url])));
        ws.send(Message::Text(control.to_string().into()))
            .await
            .expect("send priority control frame");
        ws.send(Message::Text(
            json!(["OK", event.id.to_hex(), true, "authenticated"])
                .to_string()
                .into(),
        ))
        .await
        .expect("send auth OK");
        ws
    };
    let (peer, connected) = timeout(Duration::from_secs(3), async {
        tokio::join!(peer, do_connect(&relay_url, &keys, None))
    })
    .await
    .expect("real auth handshake finishes within three seconds");
    (connected, peer, keys)
}

#[tokio::test]
async fn task_advisory_before_auth_ok_does_not_abort_connection_or_dispatch_work() {
    let advisory = json!(["BUZZ_TASKS_SYNC_REQUIRED", Uuid::new_v4()]);
    let (connected, mut peer, keys) = connect_with_control_before_ok(advisory.clone()).await;
    assert!(
        connected.is_ok(),
        "auth handshake failed: {:?}",
        connected.err()
    );
    let (mut client, buffer) = connected.expect("authenticated connection");
    assert_eq!(buffer.len(), 1, "the advisory arrived before the auth OK");
    let (event_tx, mut event_rx) = mpsc::channel(4);
    let (observer_tx, mut observer_rx) = mpsc::channel(4);
    let mut state = BgState::new();
    assert!(
        process_handshake_buffer(
            &mut client,
            buffer,
            &event_tx,
            &observer_tx,
            &mut state,
            &keys,
            "ws://localhost",
            &keys.public_key().to_hex(),
            None,
        )
        .await
    );
    // The same advisory is harmless after authentication as well.
    peer.send(Message::Text(advisory.to_string().into()))
        .await
        .expect("send post-auth advisory");
    let received = timeout(Duration::from_secs(1), client.next())
        .await
        .expect("receive advisory promptly")
        .expect("connection remains open")
        .expect("receive advisory");
    assert!(
        handle_ws_message(
            received,
            &mut client,
            &event_tx,
            &observer_tx,
            &mut state,
            &keys,
            "ws://localhost",
            &keys.public_key().to_hex(),
            None,
        )
        .await
    );
    assert!(matches!(
        event_rx.try_recv(),
        Err(mpsc::error::TryRecvError::Empty)
    ));
    assert!(matches!(
        observer_rx.try_recv(),
        Err(mpsc::error::TryRecvError::Empty)
    ));
    assert!(state.last_seen.is_empty());
    assert!(state.active_subscriptions.is_empty());
}

#[tokio::test]
async fn unknown_control_before_auth_ok_still_rejects_connection() {
    let (connected, _peer, _keys) =
        connect_with_control_before_ok(json!(["UNRECOGNIZED_CONTROL", Uuid::new_v4()])).await;
    assert!(
        matches!(connected, Err(RelayError::UnexpectedMessage(ref message))
        if message == "unknown message type: UNRECOGNIZED_CONTROL")
    );
}

#[test]
fn malformed_task_advisories_remain_protocol_errors() {
    for frame in [
        json!(["BUZZ_TASKS_SYNC_REQUIRED"]),
        json!(["BUZZ_TASKS_SYNC_REQUIRED", null]),
        json!(["BUZZ_TASKS_SYNC_REQUIRED", "not-a-channel-id"]),
        json!(["BUZZ_TASKS_SYNC_REQUIRED", Uuid::new_v4(), "extra"]),
    ] {
        assert!(
            matches!(
                parse_relay_message(&frame.to_string()),
                Err(RelayError::UnexpectedMessage(_))
            ),
            "malformed advisory was accepted: {frame}"
        );
    }
}
