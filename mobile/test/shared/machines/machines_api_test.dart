import 'dart:convert';
import 'package:buzz/shared/machines/computer.dart';
import 'package:buzz/shared/machines/machines_api.dart';
import 'package:buzz/shared/relay/relay.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:nostr/nostr.dart' as nostr;

const computerId = '00000000-0000-0000-0000-000000000001';
Map<String, dynamic> computerJson(
  String owner, {
  String id = computerId,
  String label = 'Office computer',
  bool observed = false,
  DateTime? now,
}) {
  now ??= DateTime.utc(2026, 9, 9);
  return {
    'server_now': now.toIso8601String(),
    'machine_id': id,
    'owner_pubkey': owner,
    'coordinator_pubkey': 'a' * 64,
    'label': label,
    'runtime': 'hermes',
    'observation_sequence': observed ? 1 : 0,
    'reported_state': observed ? 'ready' : null,
    'fresh': observed,
    'observed_at': observed ? now.toIso8601String() : null,
    'received_at': observed ? now.toIso8601String() : null,
    'expires_at': observed
        ? now.add(const Duration(seconds: 120)).toIso8601String()
        : null,
  };
}

void main() {
  final keys = nostr.Keys.generate();
  MachinesApi api(Future<http.Response> Function(http.Request) handler) =>
      MachinesApi(
        httpClient: MockClient(handler),
        baseUrl: 'https://computers.example',
        nsec: keys.nsec,
      );
  test(
    'request latency consumes server validity and missing server clock is never fresh',
    () async {
      var elapsed = const Duration(minutes: 10);
      final serverNow = DateTime.utc(2026, 9, 9);
      final client = MachinesApi(
        httpClient: MockClient((_) async {
          elapsed += const Duration(seconds: 40);
          return http.Response(
            jsonEncode(
              computerJson(
                pubkeyFromNsec(keys.nsec)!,
                observed: true,
                now: serverNow,
              ),
            ),
            200,
          );
        }),
        baseUrl: 'https://computers.example',
        nsec: keys.nsec,
        elapsed: () => elapsed,
      );
      final c = await client.get(computerId);
      expect(c.freshAt(elapsed), isTrue);
      expect(c.freshAt(const Duration(minutes: 12)), isFalse);
      final missing = EnrolledComputer.fromJson(
        computerJson('b' * 64, observed: true)..remove('server_now'),
      );
      expect(missing.freshAt(Duration.zero), isFalse);
      expect(missing.statusAt(Duration.zero), 'Status unavailable');
    },
  );
  test(
    'real signing binds full cursor URI and emits distinct replay nonces',
    () async {
      final events = <Map<String, dynamic>>[];
      final client = api((request) async {
        expect(request.method, 'GET');
        expect(
          request.url.toString(),
          'https://computers.example/api/machines?limit=1&after=$computerId',
        );
        expect(request.headers.containsKey('x-pubkey'), isFalse);
        final event =
            jsonDecode(
                  utf8.decode(
                    base64Decode(
                      request.headers['authorization']!.substring(6),
                    ),
                  ),
                )
                as Map<String, dynamic>;
        events.add(event);
        expect(event['pubkey'], pubkeyFromNsec(keys.nsec));
        expect(event['kind'], 27235);
        expect(event['tags'], contains(equals(['u', request.url.toString()])));
        expect(event['tags'], contains(equals(['method', 'GET'])));
        expect(nostr.Event.fromJson(jsonEncode(event)).isValid(), isTrue);
        return http.Response(
          jsonEncode({
            'machines': [computerJson(pubkeyFromNsec(keys.nsec)!)],
            'next_cursor': computerId,
          }),
          200,
        );
      });
      expect(
        (await client.list(after: computerId, limit: 1)).computers.single.label,
        'Office computer',
      );
      await client.list(after: computerId, limit: 1);
      expect(events[0]['id'], isNot(events[1]['id']));
    },
  );
  test('rejects cross-owner data and mismatched detail IDs', () async {
    await expectLater(
      api(
        (_) async => http.Response(
          jsonEncode({
            'machines': [computerJson('b' * 64)],
          }),
          200,
        ),
      ).list(),
      throwsFormatException,
    );
    await expectLater(
      api(
        (_) async => http.Response(
          jsonEncode(
            computerJson(
              pubkeyFromNsec(keys.nsec)!,
              id: '00000000-0000-0000-0000-000000000002',
            ),
          ),
          200,
        ),
      ).get(computerId),
      throwsFormatException,
    );
  });
  test('rejects malformed routes and page cursors before sending', () async {
    var calls = 0;
    final client = api((_) async {
      calls++;
      return http.Response('{}', 200);
    });
    await expectLater(client.get('../users'), throwsArgumentError);
    await expectLater(client.list(after: 'bad'), throwsArgumentError);
    await expectLater(client.list(limit: 101), throwsArgumentError);
    expect(calls, 0);
    await expectLater(
      api(
        (_) async => http.Response('{"machines":[],"next_cursor":"bad"}', 200),
      ).list(),
      throwsFormatException,
    );
  });
  test(
    'preserves strict auth failures without rendering server internals',
    () async {
      for (final status in [401, 403, 404, 500]) {
        await expectLater(
          api(
            (_) async => http.Response('private internal error', status),
          ).get(computerId),
          throwsA(
            isA<ComputerApiException>().having(
              (e) => e.statusCode,
              'status',
              status,
            ),
          ),
        );
      }
    },
  );
  test('parses every runtime/state and expires at the exact deadline', () {
    final now = DateTime.utc(2026, 9, 9);
    for (final runtime in ['hermes', 'openclaw', 'codex', 'claude-code']) {
      for (final state in ['ready', 'busy', 'unavailable']) {
        final c = EnrolledComputer.fromJson(
          computerJson('b' * 64, observed: true, now: now)
            ..['runtime'] = runtime
            ..['reported_state'] = state,
        );
        expect(c.runtimeLabel, isNotEmpty);
        expect(c.freshAt(Duration.zero), isTrue);
        expect(c.freshAt(const Duration(seconds: 120)), isFalse);
      }
    }
    expect(
      EnrolledComputer.fromJson(computerJson('b' * 64)).statusAt(Duration.zero),
      'No update yet',
    );
  });
  test('rejects unknown, partial, unsafe or unbounded observations', () {
    for (final patch in <Map<String, dynamic>>[
      {'runtime': 'unknown'},
      {'reported_state': 'online'},
      {'observation_sequence': 9007199254740992},
      {'observation_sequence': -1},
      {'observation_sequence': 1.5},
      {'expires_at': null},
      {'expires_at': '2026-09-09T00:02:01Z'},
      {'observed_at': '2026-09-09 00:00:00'},
      {'machine_id': 'bad'},
      {'label': 'bad\nname'},
      {'fresh': 'yes'},
    ]) {
      expect(
        () => EnrolledComputer.fromJson(
          computerJson('b' * 64, observed: true)..addAll(patch),
        ),
        throwsFormatException,
        reason: patch.toString(),
      );
    }
    expect(
      () => EnrolledComputer.fromJson(computerJson('b' * 64)..['fresh'] = true),
      throwsFormatException,
    );
  });
}
