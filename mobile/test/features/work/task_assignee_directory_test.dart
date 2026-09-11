import 'dart:convert';

import 'package:buzz/shared/relay/relay.dart';
import 'package:buzz/shared/tasks/task_assignee_directory.dart';
import 'package:flutter_test/flutter_test.dart';

final _owner = '1' * 64;
final _bot = '2' * 64;
final _outsider = '3' * 64;

NostrEvent _event(
  int kind,
  String pubkey, {
  List<List<String>> tags = const [],
  Map<String, Object?> content = const {},
}) => NostrEvent(
  id: '$kind-$pubkey',
  pubkey: pubkey,
  createdAt: 1,
  kind: kind,
  tags: tags,
  content: jsonEncode(content),
  sig: '',
);

void main() {
  test(
    'offers registered conversation bots, not self-declared outsiders or people',
    () async {
      final directory = TaskAssigneeDirectory(
        actorPubkey: _owner,
        query: (filters) async {
          expect(filters.map((f) => f.toJson()['kinds']), [
            [10100],
            [39002],
          ]);
          return [
            _event(10100, _bot, content: {'display_name': 'Mack'}),
            _event(
              10100,
              _outsider,
              content: {
                'display_name': 'Mack',
                'channel_ids': ['room'],
              },
            ),
            _event(
              39002,
              '4' * 64,
              tags: [
                ['d', 'room'],
                ['p', _owner, '', 'owner'],
                ['p', _bot, '', 'bot'],
                ['p', _outsider, '', 'member'],
              ],
            ),
          ];
        },
      );
      final result = await directory.load('room');
      expect(result.map((a) => a.pubkey), [_bot]);
      expect(result.single.displayName, 'Mack');
    },
  );

  test(
    'membership loss fails even when an agent remains in the directory',
    () async {
      final directory = TaskAssigneeDirectory(
        actorPubkey: _owner,
        query: (_) async => [
          _event(10100, _bot),
          _event(
            39002,
            '4' * 64,
            tags: [
              ['d', 'room'],
              ['p', _bot, '', 'bot'],
            ],
          ),
        ],
      );
      await expectLater(directory.load('room'), throwsStateError);
    },
  );

  test(
    'workspace tasks have no conversation assignment and make no directory request',
    () async {
      final directory = TaskAssigneeDirectory(
        actorPubkey: _owner,
        query: (_) async => throw StateError('must not query'),
      );
      expect(await directory.load(null), isEmpty);
    },
  );

  test(
    'a failed directory read stays an error, not an empty successful directory',
    () async {
      final directory = TaskAssigneeDirectory(
        actorPubkey: _owner,
        query: (_) async => throw StateError('offline'),
      );
      await expectLater(directory.load('room'), throwsStateError);
    },
  );
}
