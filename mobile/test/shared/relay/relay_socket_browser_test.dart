@TestOn('browser')
library;

import 'dart:async';

import 'package:buzz/shared/relay/relay_socket.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:web_socket_channel/web_socket_channel.dart';

void main() {
  test('uses the browser transport instead of native-only dart:io', () async {
    final disconnected = Completer<Object?>();
    final socket = RelaySocket(
      // Port zero cannot host a listening service. This exercises the real
      // browser connection path without a network fixture or auth credentials.
      wsUrl: 'ws://127.0.0.1:0',
      nsec: null,
      onMessage: (_) {},
      onConnected: () => fail('port zero must not establish a connection'),
      onDisconnected: (error) {
        if (!disconnected.isCompleted) disconnected.complete(error);
      },
    );
    addTearDown(socket.dispose);

    await socket.connect();
    final error = await disconnected.future.timeout(const Duration(seconds: 5));

    expect(error, isA<WebSocketChannelException>());
    expect(error, isNot(isA<UnsupportedError>()));
  });
}
