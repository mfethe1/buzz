import 'dart:convert';

import 'package:buzz/features/channels/channel_message_cache/channel_message_cache_storage.dart';
import 'package:buzz/shared/relay/nostr_models.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shared_preferences/shared_preferences.dart';

NostrEvent event(String id, {int createdAt = 1000, String content = 'hi'}) =>
    NostrEvent(
      id: id,
      pubkey: 'a' * 64,
      createdAt: createdAt,
      kind: 42,
      tags: const [
        ['h', 'chan-1'],
      ],
      content: content,
      sig: 'b' * 128,
    );

const _baseUrl = 'https://relay.example';
const _storedOrigin = 'wss://relay.example';
const _pubkey =
    'cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc';
const _otherPubkey =
    'dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd';

void main() {
  late SharedPreferences prefs;
  late ChannelMessageCacheStorage storage;

  setUp(() async {
    SharedPreferences.setMockInitialValues({});
    prefs = await SharedPreferences.getInstance();
    storage = ChannelMessageCacheStorage(prefs);
  });

  bool write(
    String channelId,
    List<NostrEvent> messages, {
    ChannelCacheEligibility eligibility = ChannelCacheEligibility.eligible,
    int Function()? now,
  }) => storage.writeChannel(
    baseUrl: _baseUrl,
    storedOrigin: _storedOrigin,
    pubkey: _pubkey,
    channelId: channelId,
    messages: messages,
    eligibility: eligibility,
    now: now ?? () => 1,
  );

  List<NostrEvent>? read(
    String channelId, {
    String baseUrl = _baseUrl,
    String pubkey = _pubkey,
  }) => storage.readChannel(
    baseUrl: baseUrl,
    storedOrigin: _storedOrigin,
    pubkey: pubkey,
    channelId: channelId,
  );

  group('eligibility classification (fails closed)', () {
    test('dm channelType is never eligible', () {
      expect(eligibilityForChannelType('dm'), ChannelCacheEligibility.dm);
      expect(eligibilityForChannelType('dm').mayPersist, isFalse);
    });

    test('stream and forum are eligible', () {
      expect(
        eligibilityForChannelType('stream'),
        ChannelCacheEligibility.eligible,
      );
      expect(
        eligibilityForChannelType('forum'),
        ChannelCacheEligibility.eligible,
      );
    });

    test('null (roster lag) fails closed as unresolved', () {
      expect(
        eligibilityForChannelType(null),
        ChannelCacheEligibility.unresolved,
      );
      expect(eligibilityForChannelType(null).mayPersist, isFalse);
    });

    test('an unknown future channelType fails closed, not open', () {
      expect(
        eligibilityForChannelType('some-new-kind'),
        ChannelCacheEligibility.unresolved,
      );
      expect(eligibilityForChannelType('').mayPersist, isFalse);
      expect(eligibilityForChannelType('DM').mayPersist, isFalse);
    });
  });

  group('DM exclusion is enforced at write time', () {
    test('a dm channel writes nothing at all', () {
      expect(
        write(
          'dm-1',
          [event('e1', content: 'secret')],
          eligibility: ChannelCacheEligibility.dm,
        ),
        isFalse,
      );
      expect(read('dm-1'), isNull);
      expect(prefs.getKeys(), isEmpty);
    });

    test('an unresolved channel writes nothing at all', () {
      expect(
        write(
          'unknown-1',
          [event('e1')],
          eligibility: ChannelCacheEligibility.unresolved,
        ),
        isFalse,
      );
      expect(read('unknown-1'), isNull);
      expect(prefs.getKeys(), isEmpty);
    });

    test('no DM body reaches disk even as a substring', () {
      write('chan-1', [event('e1', content: 'public')]);
      expect(
        write(
          'dm-1',
          [event('e2', content: 'SENSITIVE-DM-BODY')],
          eligibility: ChannelCacheEligibility.dm,
        ),
        isFalse,
      );
      final raw = prefs.getString(
        cacheKeyFor(origin: _baseUrl, pubkey: _pubkey),
      )!;
      expect(raw, contains('public'));
      expect(raw, isNot(contains('SENSITIVE-DM-BODY')));
      expect(raw, isNot(contains('dm-1')));
    });

    test('a refused write leaves an existing entry untouched', () {
      write('chan-1', [event('e1', content: 'kept')]);
      expect(
        write(
          'chan-1',
          [event('e9', content: 'should-not-land')],
          eligibility: ChannelCacheEligibility.dm,
        ),
        isFalse,
      );
      expect(read('chan-1')!.map((e) => e.id), ['e1']);
    });
  });

  group('round trip', () {
    test('writes then reads back messages in order', () {
      write('chan-1', [
        event('e1', createdAt: 10),
        event('e2', createdAt: 20),
      ]);
      final got = read('chan-1')!;
      expect(got.map((e) => e.id), ['e1', 'e2']);
      expect(got.first.content, 'hi');
      expect(got.first.tags, [
        ['h', 'chan-1'],
      ]);
    });

    test('unknown channel reads null, not an empty list', () {
      write('chan-1', [event('e1')]);
      expect(read('chan-2'), isNull);
    });

    test('an empty message list stores no entry (nothing to paint)', () {
      expect(write('chan-1', const []), isTrue);
      expect(read('chan-1'), isNull);
    });

    test('re-writing the same channel replaces rather than appends', () {
      write('chan-1', [event('e1')]);
      write('chan-1', [event('e2'), event('e3')]);
      expect(read('chan-1')!.map((e) => e.id), ['e2', 'e3']);
    });
  });

  group('identity and relay scoping', () {
    test('another pubkey cannot read this identity messages', () {
      write('chan-1', [event('e1')]);
      expect(read('chan-1', pubkey: _otherPubkey), isNull);
    });

    test('another relay origin cannot read this community messages', () {
      write('chan-1', [event('e1')]);
      expect(read('chan-1', baseUrl: 'https://other.example'), isNull);
    });

    test('a legacy-origin blob is promoted onto the canonical key', () async {
      final legacy = ChannelMessageCacheStore(
        channels: [
          ChannelMessageCacheEntry(
            channelId: 'chan-1',
            updatedAt: 1,
            messages: [event('legacy-1')],
          ),
        ],
      );
      SharedPreferences.setMockInitialValues({
        cacheKeyFor(origin: _storedOrigin, pubkey: _pubkey): jsonEncode(
          legacy.toJson(),
        ),
      });
      prefs = await SharedPreferences.getInstance();
      storage = ChannelMessageCacheStorage(prefs);

      expect(read('chan-1')!.map((e) => e.id), ['legacy-1']);
    });
  });

  group('caps and eviction', () {
    test('per-channel message cap keeps the newest messages', () {
      write('chan-1', [
        for (var i = 0; i < maxCachedMessagesPerChannel + 10; i++)
          event('e$i', createdAt: i),
      ]);
      final got = read('chan-1')!;
      expect(got.length, maxCachedMessagesPerChannel);
      expect(got.first.id, 'e10');
      expect(got.last.id, 'e${maxCachedMessagesPerChannel + 9}');
    });

    test('channel cap evicts the least-recently-updated channel', () {
      for (var i = 0; i < maxCachedChannels + 2; i++) {
        write('chan-$i', [event('e$i')], now: () => i + 1);
      }
      expect(read('chan-0'), isNull);
      expect(read('chan-1'), isNull);
      for (var i = 2; i < maxCachedChannels + 2; i++) {
        expect(read('chan-$i'), isNotNull, reason: 'chan-$i should survive');
      }
    });

    test('re-writing an old channel refreshes its LRU position', () {
      for (var i = 0; i < maxCachedChannels; i++) {
        write('chan-$i', [event('e$i')], now: () => i + 1);
      }
      // Touch the oldest so the next insert evicts chan-1 instead.
      write('chan-0', [event('e0b')], now: () => 100);
      write('chan-new', [event('en')], now: () => 101);
      expect(read('chan-0'), isNotNull);
      expect(read('chan-1'), isNull);
    });

    test('byte budget trims a single oversized channel messages', () {
      final big = 'x' * 20000;
      write('chan-1', [
        for (var i = 0; i < 40; i++) event('e$i', createdAt: i, content: big),
      ]);
      final got = read('chan-1')!;
      expect(got, isNotEmpty);
      expect(got.length, lessThan(40));
      final raw = prefs.getString(
        cacheKeyFor(origin: _baseUrl, pubkey: _pubkey),
      )!;
      expect(utf8.encode(raw).length, lessThanOrEqualTo(maxCachedBytes));
      // Oldest-first trim: the newest message always survives.
      expect(got.last.id, 'e39');
    });

    test('a single unstorable message stores nothing rather than overflow', () {
      write('chan-1', [event('huge', content: 'x' * (maxCachedBytes * 2))]);
      final raw = prefs.getString(
        cacheKeyFor(origin: _baseUrl, pubkey: _pubkey),
      )!;
      expect(utf8.encode(raw).length, lessThanOrEqualTo(maxCachedBytes));
      expect(read('chan-1'), isNull);
    });
  });

  group('corruption tolerance', () {
    Future<void> seedRaw(String raw) async {
      SharedPreferences.setMockInitialValues({
        cacheKeyFor(origin: _baseUrl, pubkey: _pubkey): raw,
      });
      prefs = await SharedPreferences.getInstance();
      storage = ChannelMessageCacheStorage(prefs);
    }

    test('truncated JSON reads as empty instead of throwing', () async {
      await seedRaw('{"version":1,"channels":[{"channelId":"chan-1"');
      expect(read('chan-1'), isNull);
    });

    test('non-JSON garbage reads as empty', () async {
      await seedRaw('not json at all');
      expect(read('chan-1'), isNull);
    });

    test('empty string reads as empty', () async {
      await seedRaw('');
      expect(read('chan-1'), isNull);
    });

    test('a JSON list instead of an object reads as empty', () async {
      await seedRaw('[1,2,3]');
      expect(read('chan-1'), isNull);
    });

    test('an unknown version is treated as empty, never thrown', () async {
      await seedRaw(jsonEncode({'version': 99, 'channels': []}));
      expect(read('chan-1'), isNull);
    });

    test('a malformed event is skipped, the rest of the channel kept', () async {
      await seedRaw(
        jsonEncode({
          'version': 1,
          'channels': [
            {
              'channelId': 'chan-1',
              'updatedAt': 1,
              'messages': [
                {'id': 'bad'}, // missing required fields
                event('good').toJson(),
                'not-a-map',
              ],
            },
          ],
        }),
      );
      expect(read('chan-1')!.map((e) => e.id), ['good']);
    });

    test('a malformed channel entry is skipped, siblings kept', () async {
      await seedRaw(
        jsonEncode({
          'version': 1,
          'channels': [
            {'channelId': 42, 'updatedAt': 1, 'messages': []},
            {
              'channelId': 'chan-2',
              'updatedAt': 1,
              'messages': [event('e2').toJson()],
            },
          ],
        }),
      );
      expect(read('chan-1'), isNull);
      expect(read('chan-2')!.map((e) => e.id), ['e2']);
    });

    test('writing over a corrupt blob recovers rather than failing', () async {
      await seedRaw('{{{corrupt');
      expect(write('chan-1', [event('e1')]), isTrue);
      expect(read('chan-1')!.map((e) => e.id), ['e1']);
    });
  });

  group('concurrent writers on the shared key', () {
    test('interleaved channel writes both survive (read-modify-write)', () {
      write('chan-a', [event('ea')], now: () => 1);
      write('chan-b', [event('eb')], now: () => 2);
      expect(read('chan-a')!.map((e) => e.id), ['ea']);
      expect(read('chan-b')!.map((e) => e.id), ['eb']);
    });

    test('a DM write interleaved between two eligible writes is a no-op', () {
      write('chan-a', [event('ea')], now: () => 1);
      write(
        'dm-x',
        [event('ex', content: 'leak')],
        eligibility: ChannelCacheEligibility.dm,
        now: () => 2,
      );
      write('chan-b', [event('eb')], now: () => 3);
      expect(read('chan-a'), isNotNull);
      expect(read('chan-b'), isNotNull);
      expect(read('dm-x'), isNull);
    });
  });
}
