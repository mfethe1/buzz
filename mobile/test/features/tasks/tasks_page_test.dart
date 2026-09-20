import 'dart:convert';

import 'package:buzz/features/tasks/tasks_page.dart';
import 'package:buzz/features/tasks/tasks_provider.dart';
import 'package:buzz/shared/relay/relay.dart';
import 'package:buzz/shared/tasks/tasks_api.dart';
import 'package:buzz/shared/theme/theme.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart' as http_testing;
import 'package:nostr/nostr.dart' as nostr;

late String _nsec;

class _TestRelayConfig extends RelayConfigNotifier {
  @override
  RelayConfig build() =>
      RelayConfig(baseUrl: 'https://relay.example.com', nsec: _nsec);
}

Map<String, dynamic> _task(
  String id,
  String title,
  String status, {
  String? assignee,
  String? dueAt,
}) {
  return {
    'id': id,
    'title': title,
    'status': status,
    'priority': 2,
    'created_at': '2026-09-19T12:00:00Z',
    'updated_at': '2026-09-19T12:00:00Z',
    'assignee': assignee,
    'due_at': dueAt,
  };
}

void main() {
  setUp(() {
    _nsec = nostr.Keys.generate().nsec;
  });

  /// Builds the Tasks tab over a relay stub, capturing the query strings the
  /// page actually issues so filter wiring is proven, not assumed.
  Widget buildTasks({
    required List<Map<String, dynamic>> tasks,
    List<Uri>? requested,
    int status = 200,
  }) {
    final client = http_testing.MockClient((request) async {
      requested?.add(request.url);
      if (status != 200) {
        return http.Response(
          jsonEncode({'error': 'relay unavailable'}),
          status,
          headers: {'content-type': 'application/json'},
        );
      }
      final wanted = request.url.queryParameters['status'];
      final page = wanted == null
          ? tasks
          : [
              for (final task in tasks)
                if (task['status'] == wanted) task,
            ];
      return http.Response(
        jsonEncode({'tasks': page}),
        200,
        headers: {'content-type': 'application/json'},
      );
    });

    return ProviderScope(
      overrides: [
        relayConfigProvider.overrideWith(_TestRelayConfig.new),
        tasksHttpClientProvider.overrideWithValue(client),
      ],
      child: MaterialApp(theme: AppTheme.light(), home: const TasksPage()),
    );
  }

  testWidgets('lists this community open tasks in relay order', (tester) async {
    final requested = <Uri>[];
    await tester.pumpWidget(
      buildTasks(
        requested: requested,
        tasks: [
          _task('t-1', 'Ship the push gateway', 'in_progress', assignee: 'ada'),
          _task('t-2', 'Renumber the migration', 'open'),
          _task('t-3', 'Archive the old fixtures', 'done'),
        ],
      ),
    );
    await tester.pumpAndSettle();

    // "Open" is the default and is resolved client-side, so the relay is
    // asked for everything and the terminal states are dropped here.
    expect(requested.single.queryParameters.containsKey('status'), isFalse);
    expect(requested.single.queryParameters['limit'], '$kTasksPageLimit');

    expect(find.text('Ship the push gateway'), findsOneWidget);
    expect(find.text('Renumber the migration'), findsOneWidget);
    expect(find.text('Archive the old fixtures'), findsNothing);

    // Relay order (newest-modified first) is preserved, not re-sorted.
    final first = tester.getTopLeft(find.text('Ship the push gateway'));
    final second = tester.getTopLeft(find.text('Renumber the migration'));
    expect(first.dy, lessThan(second.dy));

    // The assignee leads the subtitle so a task reads as someone's problem.
    expect(find.text('@ada'), findsOneWidget);
  });

  testWidgets('a status chip refetches with that relay status', (tester) async {
    final requested = <Uri>[];
    await tester.pumpWidget(
      buildTasks(
        requested: requested,
        tasks: [
          _task('t-1', 'Ship the push gateway', 'in_progress'),
          _task('t-2', 'Renumber the migration', 'open'),
        ],
      ),
    );
    await tester.pumpAndSettle();
    expect(find.text('Renumber the migration'), findsOneWidget);

    await tester.tap(find.text('In progress'));
    await tester.pumpAndSettle();

    expect(requested.last.queryParameters['status'], 'in_progress');
    expect(find.text('Ship the push gateway'), findsOneWidget);
    expect(find.text('Renumber the migration'), findsNothing);
  });

  testWidgets('an empty filter explains itself instead of showing blank', (
    tester,
  ) async {
    await tester.pumpWidget(buildTasks(tasks: const []));
    await tester.pumpAndSettle();

    expect(find.textContaining('No open tasks'), findsOneWidget);
  });

  testWidgets('a relay failure offers a retry', (tester) async {
    await tester.pumpWidget(buildTasks(tasks: const [], status: 503));
    await tester.pumpAndSettle();

    expect(find.text('Try again'), findsOneWidget);
  });
}
