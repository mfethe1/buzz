import 'dart:async';
import 'dart:convert';

import 'package:buzz/features/channels/channel.dart';
import 'package:buzz/features/channels/channel_message_cache/channel_message_cache_storage.dart';
import 'package:buzz/features/channels/channel_messages_provider.dart';
import 'package:buzz/features/channels/channels_provider.dart';
import 'package:buzz/shared/relay/relay.dart';
import 'package:buzz/shared/theme/theme_provider.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:shared_preferences/shared_preferences.dart';

/// Provider-level hardening for the cold-start channel message cache.
///
/// `channel_message_cache_storage_test.dart` covers the store in isolation.
/// This file covers the two things only the notifier can decide, each of which
/// is a failure mode rather than a happy path:
///
///  * the **seed**: what a cold launch paints before the relay answers, and the
///    `hasLoadedMessages` distinction between "loaded and empty" and
///    "synthetic empty because we are not connected yet";
///  * the **write gate**: resolving `channelType` from a roster that is not
///    persisted, and failing closed (writing nothing) whenever that resolution
///    does not produce a non-DM channel.
const _channelId = '11111111-1111-4111-8111-111111111111';
const _dmChannelId = '22222222-2222-4222-8222-222222222222';
const _relayUrl = 'https://relay.example';
const _pubkey = 'pk_a';

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  group('cold-start seed', () {
    test('paints cached messages while disconnected instead of empty', () async {
      // The exact cold-start shape: prefs already hold a snapshot, the relay
      // has not connected yet. Before this feature the notifier had nothing in
      // memory and returned AsyncData([]) -> connection skeleton.
      SharedPreferences.setMockInitialValues({
        _cacheKey: _storeJson(_channelId, [_event('cached', 10)]),
      });
      final harness = await _Harness.create(connected: false);

      final state = harness.container.read(channelMessagesProvider(_channelId));

      expect(state.value?.map((e) => e.id), ['cached']);
      expect(harness.notifier(_channelId).hasLoadedMessages, isTrue);
    });

    test('an empty cached list does not claim a load happened', () async {
      // Negative test for the synthetic-empty distinction. An entry with zero
      // messages must leave hasLoadedMessages false, or the UI would render a
      // "this channel is empty" state for a channel it has never loaded.
      SharedPreferences.setMockInitialValues({
        _cacheKey: _storeJson(_channelId, const []),
      });
      final harness = await _Harness.create(connected: false);

      final state = harness.container.read(channelMessagesProvider(_channelId));

      expect(state.value, isEmpty);
      expect(harness.notifier(_channelId).hasLoadedMessages, isFalse);
    });

    test('no cache at all behaves exactly as before the feature', () async {
      SharedPreferences.setMockInitialValues({});
      final harness = await _Harness.create(connected: false);

      final state = harness.container.read(channelMessagesProvider(_channelId));

      expect(state.value, isEmpty);
      expect(harness.notifier(_channelId).hasLoadedMessages, isFalse);
    });

    test('a corrupt cache blob degrades to a spinner, never throws', () async {
      SharedPreferences.setMockInitialValues({_cacheKey: '{not json'});
      final harness = await _Harness.create(connected: false);

      expect(
        () => harness.container.read(channelMessagesProvider(_channelId)),
        returnsNormally,
      );
      expect(harness.notifier(_channelId).hasLoadedMessages, isFalse);
    });

    test('another identity cache is not painted into this identity', () async {
      // authz/isolation denial: the blob exists, but under a different pubkey.
      SharedPreferences.setMockInitialValues({
        'buzz.channel-message-cache.v1:$_relayUrl:someone_else': _storeJson(
          _channelId,
          [_event('theirs', 10)],
        ),
      });
      final harness = await _Harness.create(connected: false);

      final state = harness.container.read(channelMessagesProvider(_channelId));

      expect(state.value, isEmpty, reason: 'must not read across identities');
      expect(harness.notifier(_channelId).hasLoadedMessages, isFalse);
    });

    test('missing prefs override degrades instead of crashing', () async {
      // savedPrefsProvider throws until main() overrides it. The cache is
      // strictly optional, so the notifier must still build.
      final session = _FakeSession(connected: false);
      final container = ProviderContainer(
        overrides: [
          relaySessionProvider.overrideWith(() => session),
          relayConfigProvider.overrideWith(_FixedRelayConfig.new),
          myPubkeyProvider.overrideWithValue(_pubkey),
        ],
      );
      addTearDown(container.dispose);

      expect(
        () => container.read(channelMessagesProvider(_channelId)),
        returnsNormally,
      );
    });

    test(
      'live history replaces the seed rather than merging with it',
      () async {
        // The cache is not authority: a message deleted server-side must not
        // survive as a ghost once the relay answers.
        SharedPreferences.setMockInitialValues({
          _cacheKey: _storeJson(_channelId, [_event('deleted-since', 10)]),
        });
        final harness = await _Harness.create(
          historyResults: [
            [_event('live', 20)],
          ],
        );

        harness.container.read(channelMessagesProvider(_channelId));
        await harness.session.subscribed;
        await _pump();

        expect(
          harness.container
              .read(channelMessagesProvider(_channelId))
              .value
              ?.map((e) => e.id),
          ['live'],
          reason: 'the stale seed must not be merged back in',
        );
      },
    );
  });

  group('write gate resolves the channel type and fails closed', () {
    test(
      'an eligible stream channel is written after a history load',
      () async {
        SharedPreferences.setMockInitialValues({});
        final harness = await _Harness.create(
          channels: [_channel(_channelId, 'stream')],
          historyResults: [
            [_event('m1', 10)],
          ],
        );

        harness.container.read(channelMessagesProvider(_channelId));
        await harness.session.subscribed;
        await _pump();

        expect(harness.cached(_channelId)?.map((e) => e.id), ['m1']);
      },
    );

    test('a DM channel writes nothing at all', () async {
      SharedPreferences.setMockInitialValues({});
      final harness = await _Harness.create(
        channelId: _dmChannelId,
        channels: [_channel(_dmChannelId, 'dm')],
        historyResults: [
          [_event('secret', 10, content: 'top-secret-dm-body')],
        ],
      );

      harness.container.read(channelMessagesProvider(_dmChannelId));
      await harness.session.subscribed;
      await _pump();

      expect(harness.cached(_dmChannelId), isNull);
      expect(
        harness.prefs.getString(_cacheKey) ?? '',
        isNot(contains('top-secret-dm-body')),
        reason: 'no DM body may reach disk, even as a substring',
      );
    });

    test('roster lag (channel not yet known) skips the write', () async {
      // The load-bearing fail-closed case: ChannelsNotifier is async, so a
      // channel opened by deep link can finish its history load before the
      // roster resolves. Unresolved must be treated exactly like `dm`.
      SharedPreferences.setMockInitialValues({});
      final harness = await _Harness.create(
        channels: const [],
        historyResults: [
          [_event('m1', 10)],
        ],
      );

      harness.container.read(channelMessagesProvider(_channelId));
      await harness.session.subscribed;
      await _pump();

      expect(harness.cached(_channelId), isNull);
    });

    test('a still-loading roster skips the write', () async {
      SharedPreferences.setMockInitialValues({});
      final harness = await _Harness.create(
        channelsNotifier: _PendingChannels(),
        historyResults: [
          [_event('m1', 10)],
        ],
      );

      harness.container.read(channelMessagesProvider(_channelId));
      await harness.session.subscribed;
      await _pump();

      expect(harness.cached(_channelId), isNull);
    });

    test('an unknown future channel type is not cached', () async {
      SharedPreferences.setMockInitialValues({});
      final harness = await _Harness.create(
        channels: [_channel(_channelId, 'huddle-v9')],
        historyResults: [
          [_event('m1', 10)],
        ],
      );

      harness.container.read(channelMessagesProvider(_channelId));
      await harness.session.subscribed;
      await _pump();

      expect(
        harness.cached(_channelId),
        isNull,
        reason: 'an upstream-added type must fail closed, not cache silently',
      );
    });

    test('a failed history load writes nothing', () async {
      // offline/timeout: the write is only reached on the success path, so a
      // failure must not persist a partial or empty snapshot.
      SharedPreferences.setMockInitialValues({});
      final harness = await _Harness.create(
        channels: [_channel(_channelId, 'stream')],
        historyError: Exception('relay down'),
      );

      harness.container.read(channelMessagesProvider(_channelId));
      await harness.session.subscribed;
      await _pump();

      expect(harness.cached(_channelId), isNull);
    });

    test('a successful but empty history writes nothing', () async {
      SharedPreferences.setMockInitialValues({});
      final harness = await _Harness.create(
        channels: [_channel(_channelId, 'stream')],
        historyResults: const [[]],
      );

      harness.container.read(channelMessagesProvider(_channelId));
      await harness.session.subscribed;
      await _pump();

      expect(
        harness.cached(_channelId),
        isNull,
        reason: 'an empty entry would hold an LRU slot while painting nothing',
      );
    });

    test(
      'a later live event does not write behind the single writer',
      () async {
        // Only the post-history-load path writes. Live events are in-memory
        // overlay updates, so one serialized writer owns the shared prefs key.
        SharedPreferences.setMockInitialValues({});
        final harness = await _Harness.create(
          channels: [_channel(_channelId, 'stream')],
          historyResults: [
            [_event('m1', 10)],
          ],
        );

        harness.container.read(channelMessagesProvider(_channelId));
        await harness.session.subscribed;
        await _pump();
        harness.session.emit(_event('live', 20));
        await _pump();

        expect(
          harness.container
              .read(channelMessagesProvider(_channelId))
              .value
              ?.map((e) => e.id),
          ['m1', 'live'],
        );
        expect(harness.cached(_channelId)?.map((e) => e.id), [
          'm1',
        ], reason: 'the live event is an in-memory overlay, not a cache write');
      },
    );

    test('a DM write leaves a sibling stream entry untouched', () async {
      // Cross-channel negative: refusing a DM must not disturb the shared key.
      SharedPreferences.setMockInitialValues({
        _cacheKey: _storeJson(_channelId, [_event('kept', 10)]),
      });
      final harness = await _Harness.create(
        channelId: _dmChannelId,
        channels: [_channel(_dmChannelId, 'dm')],
        historyResults: [
          [_event('secret', 20)],
        ],
      );

      harness.container.read(channelMessagesProvider(_dmChannelId));
      await harness.session.subscribed;
      await _pump();

      expect(harness.cached(_channelId)?.map((e) => e.id), ['kept']);
      expect(harness.cached(_dmChannelId), isNull);
    });
  });
}

const _cacheKey = 'buzz.channel-message-cache.v1:$_relayUrl:$_pubkey';

String _storeJson(String channelId, List<NostrEvent> messages) => jsonEncode({
  'version': 1,
  'channels': [
    {
      'channelId': channelId,
      'updatedAt': 1,
      'messages': messages.map((e) => e.toJson()).toList(),
    },
  ],
});

NostrEvent _event(String id, int createdAt, {String content = 'hi'}) =>
    NostrEvent(
      id: id,
      pubkey: 'alice',
      createdAt: createdAt,
      kind: EventKind.streamMessageV2,
      tags: const [
        ['h', _channelId],
      ],
      content: content,
      sig: 'sig',
    );

Channel _channel(String id, String type) => Channel(
  id: id,
  name: 'c',
  channelType: type,
  visibility: 'open',
  description: '',
  createdBy: 'alice',
  createdAt: DateTime.utc(2026),
  memberCount: 1,
);

Future<void> _pump() => Future<void>.delayed(Duration.zero);

class _Harness {
  final ProviderContainer container;
  final _FakeSession session;
  final SharedPreferences prefs;

  _Harness(this.container, this.session, this.prefs);

  static Future<_Harness> create({
    String channelId = _channelId,
    bool connected = true,
    List<Channel> channels = const [],
    ChannelsNotifier? channelsNotifier,
    List<List<NostrEvent>> historyResults = const [],
    Object? historyError,
  }) async {
    final prefs = await SharedPreferences.getInstance();
    final session = _FakeSession(
      connected: connected,
      historyResults: historyResults,
      historyError: historyError,
    );
    final container = ProviderContainer(
      overrides: [
        savedPrefsProvider.overrideWithValue(prefs),
        relaySessionProvider.overrideWith(() => session),
        relayConfigProvider.overrideWith(_FixedRelayConfig.new),
        myPubkeyProvider.overrideWithValue(_pubkey),
        channelsProvider.overrideWith(
          () => channelsNotifier ?? _FixedChannels(channels),
        ),
      ],
    );
    addTearDown(container.dispose);
    if (channelsNotifier == null) {
      // ChannelsNotifier is an AsyncNotifier, so it is AsyncLoading for one
      // microtask even with a synchronous override. Settling it here keeps
      // "roster resolved" and "roster still loading" as distinct scenarios
      // instead of accidentally testing the loading case everywhere.
      await container.read(channelsProvider.future);
    }
    return _Harness(container, session, prefs);
  }

  ChannelMessagesNotifier notifier(String channelId) =>
      container.read(channelMessagesProvider(channelId).notifier);

  /// Reads back through the real storage layer, so these assertions exercise
  /// the same key derivation the app uses rather than a test-local guess.
  List<NostrEvent>? cached(String channelId) =>
      ChannelMessageCacheStorage(prefs).readChannel(
        baseUrl: _relayUrl,
        storedOrigin: _relayUrl,
        pubkey: _pubkey,
        channelId: channelId,
      );
}

class _FixedRelayConfig extends RelayConfigNotifier {
  @override
  RelayConfig build() => const RelayConfig(baseUrl: _relayUrl);
}

class _FixedChannels extends ChannelsNotifier {
  final List<Channel> channels;
  _FixedChannels(this.channels);

  @override
  Future<List<Channel>> build() async => channels;
}

class _PendingChannels extends ChannelsNotifier {
  final Completer<List<Channel>> _completer = Completer<List<Channel>>();

  @override
  Future<List<Channel>> build() => _completer.future;
}

class _FakeSession extends RelaySessionNotifier {
  final bool connected;
  final List<List<NostrEvent>> _historyResults;
  final Object? _historyError;
  final List<void Function(NostrEvent)> _listeners = [];
  final Completer<void> _subscribed = Completer<void>();
  final Completer<List<NostrEvent>> _history = Completer<List<NostrEvent>>();
  int _historyIndex = 0;

  _FakeSession({
    required this.connected,
    List<List<NostrEvent>> historyResults = const [],
    Object? historyError,
  }) : _historyResults = historyResults,
       _historyError = historyError;

  Future<void> get subscribed => _subscribed.future;

  @override
  SessionState build() => SessionState(
    status: connected ? SessionStatus.connected : SessionStatus.disconnected,
  );

  // Forces the legacy websocket history path, which is the branch these tests
  // drive; the channel-window fast path is covered by the provider suite.
  @override
  Future<List<NostrEvent>> queryRelay(
    List<NostrFilter> filters, {
    Duration timeout = const Duration(seconds: 8),
  }) async => throw Exception('window unsupported');

  @override
  Future<List<NostrEvent>> fetchHistory(
    NostrFilter filter, {
    Duration timeout = const Duration(seconds: 8),
  }) {
    if (_historyError != null) return Future.error(_historyError);
    if (_historyIndex < _historyResults.length) {
      return Future.value(_historyResults[_historyIndex++]);
    }
    return _history.future;
  }

  @override
  Future<void Function()> subscribe(
    NostrFilter filter,
    void Function(NostrEvent) onEvent, {
    void Function(String message)? onClosed,
  }) async {
    if (!_subscribed.isCompleted) _subscribed.complete();
    _listeners.add(onEvent);
    return () => _listeners.remove(onEvent);
  }

  void emit(NostrEvent event) {
    for (final listener in List.of(_listeners)) {
      listener(event);
    }
  }
}
