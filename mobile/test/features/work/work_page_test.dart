import 'dart:convert';
import 'dart:async';

import 'package:buzz/features/work/work_page.dart';
import 'package:buzz/shared/tasks/tasks_api.dart';
import 'package:buzz/shared/tasks/task_assignee_directory.dart';
import 'package:buzz/shared/relay/relay.dart';
import 'package:buzz/shared/tasks/task_channel.dart';
import 'package:buzz/shared/tasks/tasks_sync.dart';
import 'package:buzz/shared/theme/theme.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:nostr/nostr.dart' as nostr;

Map<String, Object?> task(
  String id, {
  String status = 'todo',
  int revision = 0,
}) => {
  'id': id,
  'title': id,
  'status': status,
  'priority': 0,
  'revision': revision,
  'created_at': 1786000000,
  'updated_at': 1786000000,
};
http.Response page(List<Map<String, Object?>> tasks, [String? cursor]) =>
    http.Response(jsonEncode({'tasks': tasks, 'next_cursor': cursor}), 200);

void main() {
  late ProviderContainer container;
  Future<void> mount(
    WidgetTester tester,
    Future<http.Response> Function(http.Request) handler, {
    TaskAssigneeDirectory? directory,
    Future<void> Function()? refreshChannels,
    ValueNotifier<AsyncValue<List<TaskChannel>>>? channelOptions,
    ValueNotifier<bool>? visibility,
  }) async {
    final api = TasksApi(
      httpClient: MockClient(handler),
      baseUrl: 'https://work.example',
      nsec: nostr.Keys.generate().nsec,
    );
    container = ProviderContainer(
      overrides: [
        tasksApiProvider.overrideWithValue(api),
        if (directory != null)
          taskAssigneeDirectoryProvider.overrideWithValue(directory),
      ],
    );
    addTearDown(container.dispose);
    final options =
        channelOptions ??
        ValueNotifier<AsyncValue<List<TaskChannel>>>(
          const AsyncData([TaskChannel(id: 'room', name: 'General')]),
        );
    if (channelOptions == null) addTearDown(options.dispose);
    final shown = visibility ?? ValueNotifier(true);
    if (visibility == null) addTearDown(shown.dispose);
    await tester.pumpWidget(
      UncontrolledProviderScope(
        container: container,
        child: MaterialApp(
          theme: AppTheme.light(),
          home: ValueListenableBuilder<bool>(
            valueListenable: shown,
            builder: (context, visible, _) => ValueListenableBuilder(
              valueListenable: options,
              builder: (context, value, _) => WorkPage(
                onBack: () {},
                channels: value,
                onRefreshChannels: refreshChannels ?? () async {},
                visible: visible,
              ),
            ),
          ),
        ),
      ),
    );
    if (shown.value) {
      await tester.pumpAndSettle();
    } else {
      await tester.pump();
    }
  }

  testWidgets(
    'loads signed pages and resets the cursor when the status filter changes',
    (tester) async {
      final requests = <http.Request>[];
      await mount(tester, (request) async {
        requests.add(request);
        if (request.url.queryParameters['status'] == 'blocked') {
          return page([task('Blocked task', status: 'blocked')]);
        }
        return request.url.queryParameters.containsKey('before')
            ? page([task('Older task')])
            : page([task('First task')], 'cursor-1');
      });
      expect(find.text('First task'), findsOneWidget);
      await tester.tap(find.text('Load more'));
      await tester.pumpAndSettle();
      expect(find.text('Older task'), findsOneWidget);
      expect(requests.last.url.queryParameters['before'], 'cursor-1');
      expect(requests.last.headers['authorization'], startsWith('Nostr '));
      await tester.tap(find.byKey(const ValueKey('work-status-filter')));
      await tester.pumpAndSettle();
      await tester.tap(find.text('Blocked').last);
      await tester.pumpAndSettle();
      expect(find.text('First task'), findsNothing);
      expect(find.text('Blocked task'), findsOneWidget);
      expect(requests.last.url.queryParameters.containsKey('before'), isFalse);
    },
  );

  testWidgets('a failed next page retains tasks and retries the same cursor', (
    tester,
  ) async {
    var nextAttempts = 0;
    await mount(tester, (request) async {
      if (!request.url.queryParameters.containsKey('before')) {
        return page([task('Keep this task')], 'cursor-1');
      }
      nextAttempts++;
      return nextAttempts == 1
          ? http.Response('{"error":"private SQL details"}', 500)
          : page([task('Recovered task')]);
    });
    await tester.tap(find.text('Load more'));
    await tester.pumpAndSettle();
    expect(find.text('Keep this task'), findsOneWidget);
    expect(find.text('private SQL details'), findsNothing);
    await tester.tap(find.text('Try again'));
    await tester.pumpAndSettle();
    expect(find.text('Recovered task'), findsOneWidget);
    expect(nextAttempts, 2);
  });

  testWidgets(
    'refresh fences an in-flight page while preserving the visible snapshot',
    (tester) async {
      final delayed = Completer<http.Response>();
      final freshPage = Completer<http.Response>();
      var firstReads = 0;
      await mount(tester, (request) async {
        if (request.url.queryParameters.containsKey('before')) {
          return delayed.future;
        }
        firstReads++;
        return firstReads == 1
            ? page([task('Before refresh')], 'cursor-1')
            : freshPage.future;
      });
      await tester.tap(find.text('Load more'));
      await tester.pump();
      container.read(tasksSyncSignalProvider.notifier).bump();
      await tester.pump();
      delayed.complete(page([task('Stale page')]));
      await tester.pump();
      expect(find.text('Stale page'), findsNothing);
      expect(find.text('Before refresh'), findsOneWidget);
      freshPage.complete(page([task('After refresh')]));
      await tester.pumpAndSettle();
      expect(find.text('Stale page'), findsNothing);
      expect(find.text('Before refresh'), findsNothing);
      expect(find.text('After refresh'), findsOneWidget);
    },
  );

  testWidgets(
    'reconciliation preserves two-page continuity and removes stale rows',
    (tester) async {
      var generation = 0;
      await mount(tester, (request) async {
        final second = request.url.queryParameters['before'] == 'cursor-1';
        if (generation == 0) {
          return second
              ? page([task('Second old')])
              : page([task('First unchanged')], 'cursor-1');
        }
        return second
            ? page([task('Second changed', revision: 1)])
            : page([task('First unchanged')], 'cursor-1');
      });
      await tester.tap(find.text('Load more'));
      await tester.pumpAndSettle();
      expect(find.text('Second old'), findsOneWidget);

      generation++;
      container.read(tasksSyncSignalProvider.notifier).bump();
      await tester.pumpAndSettle();
      expect(find.text('First unchanged'), findsOneWidget);
      expect(find.text('Second old'), findsNothing);
      expect(find.text('Second changed'), findsOneWidget);
    },
  );

  testWidgets('transient reconciliation failure keeps loaded pages visible', (
    tester,
  ) async {
    var fail = false;
    await mount(tester, (request) async {
      if (fail) throw http.ClientException('temporary');
      return request.url.queryParameters.containsKey('before')
          ? page([task('Older visible')])
          : page([task('Newest visible')], 'cursor-1');
    });
    await tester.tap(find.text('Load more'));
    await tester.pumpAndSettle();
    fail = true;
    container.read(tasksSyncSignalProvider.notifier).bump();
    await tester.pumpAndSettle();
    expect(find.text('Newest visible'), findsOneWidget);
    expect(find.text('Older visible'), findsOneWidget);
  });

  testWidgets(
    'background refresh retains the loaded viewport and scroll position',
    (tester) async {
      var reads = 0;
      await mount(tester, (request) async {
        reads++;
        final second = request.url.queryParameters.containsKey('before');
        return page(
          List.generate(20, (i) => task('Task ${i + (second ? 20 : 0)}')),
          second ? null : 'next-page',
        );
      });
      await tester.scrollUntilVisible(find.text('Load more'), 250);
      await tester.tap(find.text('Load more'));
      await tester.pumpAndSettle();
      await tester.scrollUntilVisible(find.text('Task 25'), 200);
      await tester.pumpAndSettle();
      final position = tester
          .state<ScrollableState>(find.byType(Scrollable).first)
          .position;
      final before = position.pixels;
      final visibleBefore = tester.getTopLeft(find.text('Task 25'));
      await tester.pump(const Duration(seconds: 30));
      await tester.pumpAndSettle();
      expect(reads, 4);
      expect(find.text('Task 25'), findsOneWidget);
      expect(position.pixels, before);
      expect(tester.getTopLeft(find.text('Task 25')), visibleBefore);
    },
  );

  testWidgets('background failure retries the complete loaded window', (
    tester,
  ) async {
    var fail = false;
    var reads = 0;
    await mount(tester, (request) async {
      reads++;
      if (fail) throw http.ClientException('temporary transport outage');
      return request.url.queryParameters.containsKey('before')
          ? page([task('Second page retained')])
          : page([task('First page retained')], 'next-page');
    });
    await tester.tap(find.text('Load more'));
    await tester.pumpAndSettle();
    fail = true;
    await tester.pump(const Duration(seconds: 30));
    await tester.pumpAndSettle();
    expect(find.text('Second page retained'), findsOneWidget);
    expect(find.text("Couldn't refresh your tasks."), findsOneWidget);
    final beforeRetry = reads;
    fail = false;
    await tester.tap(find.text('Try again'));
    await tester.pumpAndSettle();
    expect(reads - beforeRetry, 2);
    expect(find.text("Couldn't refresh your tasks."), findsNothing);
    expect(find.text('Second page retained'), findsOneWidget);
  });

  for (final status in [401, 403, 404]) {
    testWidgets('background access denial $status clears previous rows', (
      tester,
    ) async {
      var denied = false;
      await mount(
        tester,
        (_) async => denied
            ? http.Response('{"error":"access removed"}', status)
            : page([task('Previously accessible')]),
      );
      denied = true;
      await tester.pump(const Duration(seconds: 30));
      await tester.pumpAndSettle();
      expect(find.text('Previously accessible'), findsNothing);
      expect(find.text("Couldn't load your tasks."), findsOneWidget);
      expect(find.text('Try again'), findsOneWidget);
    });
  }

  testWidgets(
    'failed background reads cannot preserve stale rows beyond 90 seconds',
    (tester) async {
      var fail = false;
      await mount(tester, (_) async {
        if (fail) throw http.ClientException('unreachable');
        return page([task('Last confirmed snapshot')]);
      });
      fail = true;
      await tester.pump(const Duration(seconds: 30));
      await tester.pumpAndSettle();
      expect(find.text('Last confirmed snapshot'), findsOneWidget);
      await tester.pump(const Duration(seconds: 61));
      await tester.pumpAndSettle();
      expect(find.text('Last confirmed snapshot'), findsNothing);
      expect(find.text('Try again'), findsOneWidget);
    },
  );

  testWidgets('pausing in a window read prevents later pages until resume', (
    tester,
  ) async {
    final delayed = Completer<http.Response>();
    var reads = 0;
    await mount(tester, (request) async {
      reads++;
      if (reads == 3) return delayed.future;
      return request.url.queryParameters.containsKey('before')
          ? page([task('Second visible')])
          : page([task('First visible')], 'next-page');
    });
    await tester.tap(find.text('Load more'));
    await tester.pumpAndSettle();
    await tester.pump(const Duration(seconds: 30));
    await tester.pump();
    expect(reads, 3);
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.inactive);
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.hidden);
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.paused);
    await tester.pump();
    delayed.complete(page([task('Retired response')], 'next-page'));
    await tester.pump();
    await tester.pump(const Duration(seconds: 60));
    expect(reads, 3);
    expect(find.text('Retired response'), findsNothing);
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.hidden);
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.inactive);
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.resumed);
    await tester.pumpAndSettle();
    expect(reads, 5);
    expect(find.text('Second visible'), findsOneWidget);
  });

  testWidgets(
    'visible Work recovers a missed task advisory within 30 seconds',
    (tester) async {
      var current = 'Before missed advisory';
      var reads = 0;
      await mount(tester, (request) async {
        reads++;
        return page([task(current, revision: reads - 1)]);
      });
      expect(find.text('Before missed advisory'), findsOneWidget);
      expect(reads, 1);

      current = 'After missed advisory';
      await tester.pump(const Duration(seconds: 30));
      await tester.pumpAndSettle();

      expect(find.text('Before missed advisory'), findsNothing);
      expect(find.text('After missed advisory'), findsOneWidget);
      expect(reads, 2);
    },
  );

  testWidgets(
    'creates an assigned task without starting execution, then opens its real detail',
    (tester) async {
      final owner = '1' * 64;
      final bot = '2' * 64;
      var directoryReads = 0;
      final directory = TaskAssigneeDirectory(
        actorPubkey: owner,
        query: (_) async {
          directoryReads++;
          return [
            NostrEvent(
              id: 'agent',
              pubkey: bot,
              createdAt: 1,
              kind: 10100,
              tags: const [],
              content: '{"display_name":"Mack"}',
              sig: '',
            ),
            NostrEvent(
              id: 'members',
              pubkey: '4' * 64,
              createdAt: 1,
              kind: 39002,
              tags: [
                ['d', 'room'],
                ['p', owner, '', 'owner'],
                ['p', bot, '', 'bot'],
              ],
              content: '',
              sig: '',
            ),
          ];
        },
      );
      Map<String, dynamic>? created;
      await mount(tester, (request) async {
        if (request.method == 'POST') {
          final payload = jsonDecode(request.body) as Map<String, dynamic>;
          expect(payload['assignee'], bot);
          expect(payload['channel_id'], 'room');
          expect(payload.containsKey('status'), isFalse);
          created = {...task('created'), ...payload};
          return http.Response(jsonEncode(created), 200);
        }
        if (request.url.path.endsWith('/created')) {
          return http.Response(
            jsonEncode({'task': created, 'events': []}),
            200,
          );
        }
        return page([
          if (created != null &&
              request.url.queryParameters['status'] != 'blocked')
            created!,
        ]);
      }, directory: directory);
      await tester.tap(find.byKey(const ValueKey('work-status-filter')));
      await tester.pumpAndSettle();
      await tester.tap(find.text('Blocked').last);
      await tester.pumpAndSettle();
      await tester.tap(find.byTooltip('New task'));
      await tester.pumpAndSettle();
      await tester.enterText(
        find.byKey(const ValueKey('work-task-title')),
        'Review the update',
      );
      await tester.tap(
        find.widgetWithText(DropdownButtonFormField<String>, 'Conversation'),
      );
      await tester.pumpAndSettle();
      await tester.tap(find.text('General').last);
      await tester.pumpAndSettle();
      await tester.tap(
        find.widgetWithText(DropdownButtonFormField<String>, 'Assignee'),
      );
      await tester.pumpAndSettle();
      await tester.tap(find.textContaining('Mack').last);
      await tester.pumpAndSettle();
      await tester.enterText(
        find.byKey(const ValueKey('work-task-body')),
        'Read the proposed changes before approving.',
      );
      await tester.tap(find.text('Create task'));
      await tester.pumpAndSettle();
      expect(directoryReads, 2);
      expect(find.text('All tasks'), findsOneWidget);
      expect(find.text('Review the update'), findsOneWidget);
      expect(find.text('Task created'), findsOneWidget);
      await tester.tap(find.byKey(const ValueKey('work-task-created')));
      await tester.pumpAndSettle();
      expect(find.byKey(const ValueKey('task-detail-content')), findsOneWidget);
      expect(find.text('To do'), findsWidgets);
      expect(find.text('Assigned to ${bot.substring(0, 8)}'), findsWidgets);
      expect(
        find.text('Read the proposed changes before approving.'),
        findsOneWidget,
      );
    },
  );
  Future<void> fillAssignedTask(WidgetTester tester) async {
    await tester.tap(find.byTooltip('New task'));
    await tester.pumpAndSettle();
    await tester.enterText(
      find.byKey(const ValueKey('work-task-title')),
      'Review the update',
    );
    await tester.tap(
      find.widgetWithText(DropdownButtonFormField<String>, 'Conversation'),
    );
    await tester.pumpAndSettle();
    await tester.tap(find.text('General').last);
    await tester.pumpAndSettle();
    await tester.tap(
      find.widgetWithText(DropdownButtonFormField<String>, 'Assignee'),
    );
    await tester.pumpAndSettle();
    await tester.tap(find.textContaining('Mack').last);
    await tester.pumpAndSettle();
  }

  TaskAssigneeDirectory eligibleDirectory({Future<bool> Function()? keepBot}) =>
      TaskAssigneeDirectory(
        actorPubkey: '1' * 64,
        query: (_) async {
          final eligible = await keepBot?.call() ?? true;
          return [
            NostrEvent(
              id: 'agent',
              pubkey: '2' * 64,
              createdAt: 1,
              kind: 10100,
              tags: const [],
              content: '{"display_name":"Mack"}',
              sig: '',
            ),
            NostrEvent(
              id: 'members',
              pubkey: '4' * 64,
              createdAt: 1,
              kind: 39002,
              tags: [
                ['d', 'room'],
                ['p', '1' * 64, '', 'owner'],
                if (eligible) ['p', '2' * 64, '', 'bot'],
              ],
              content: '',
              sig: '',
            ),
          ];
        },
      );

  testWidgets('a removed agent is not submitted from an old choice', (
    tester,
  ) async {
    var reads = 0;
    var writes = 0;
    final directory = eligibleDirectory(keepBot: () async => ++reads == 1);
    await mount(tester, (request) async {
      if (request.method == 'POST') writes++;
      return page([]);
    }, directory: directory);
    await fillAssignedTask(tester);
    await tester.tap(find.text('Create task'));
    await tester.pumpAndSettle();
    expect(writes, 0);
    expect(
      find.text('This agent is no longer available. Choose an assignee again.'),
      findsOneWidget,
    );
  });

  testWidgets(
    'an identity change during assignment validation cannot write through either identity',
    (tester) async {
      final pending = Completer<bool>();
      var reads = 0;
      var writes = 0;
      final directory = eligibleDirectory(
        keepBot: () async => ++reads == 1 ? true : pending.future,
      );
      await mount(tester, (request) async {
        if (request.method == 'POST') writes++;
        return page([]);
      }, directory: directory);
      await fillAssignedTask(tester);
      await tester.tap(find.text('Create task'));
      await tester.pump();
      final nextApi = TasksApi(
        httpClient: MockClient((request) async {
          if (request.method == 'POST') writes++;
          return page([]);
        }),
        baseUrl: 'https://other.example',
        nsec: nostr.Keys.generate().nsec,
      );
      container.updateOverrides([
        tasksApiProvider.overrideWithValue(nextApi),
        taskAssigneeDirectoryProvider.overrideWithValue(directory),
      ]);
      await tester.pump();
      pending.complete(true);
      await tester.pumpAndSettle();
      expect(writes, 0);
      expect(
        find.text('Your workspace changed. Close this task and try again.'),
        findsOneWidget,
      );
    },
  );

  testWidgets('an uncertain creation response cannot be blindly retried', (
    tester,
  ) async {
    var writes = 0;
    var reads = 0;
    await mount(tester, (request) async {
      if (request.method == 'POST') {
        writes++;
        throw http.ClientException('connection lost');
      }
      reads++;
      return page([]);
    }, directory: eligibleDirectory());
    await tester.tap(find.byTooltip('New task'));
    await tester.pumpAndSettle();
    await tester.enterText(
      find.byKey(const ValueKey('work-task-title')),
      'Review the update',
    );
    await tester.tap(find.text('Create task'));
    await tester.pumpAndSettle();
    expect(writes, 1);
    expect(find.text('Create task'), findsNothing);
    expect(
      find.text(
        "Couldn't confirm this task was created. Check Work before trying again.",
      ),
      findsOneWidget,
    );
    await tester.tap(find.text('Check Work'));
    await tester.pumpAndSettle();
    expect(reads, 2);
    expect(writes, 1);
  });

  testWidgets(
    'a repeated page cursor fails without hiding already loaded tasks',
    (tester) async {
      await mount(
        tester,
        (request) async => page([
          task(
            request.url.queryParameters.containsKey('before')
                ? 'Would loop'
                : 'First task',
          ),
        ], 'same-cursor'),
      );
      await tester.tap(find.text('Load more'));
      await tester.pumpAndSettle();
      expect(find.text('First task'), findsOneWidget);
      expect(find.text('Would loop'), findsNothing);
      expect(find.text("Couldn't load more tasks."), findsOneWidget);
    },
  );

  testWidgets(
    'resuming refetches workspace tasks that have no live notification',
    (tester) async {
      var reads = 0;
      await mount(
        tester,
        (_) async =>
            page([task(++reads == 1 ? 'Before resume' : 'After resume')]),
      );
      tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.inactive);
      tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.hidden);
      await tester.pump();
      await tester.pump(const Duration(seconds: 60));
      expect(reads, 1);
      tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.inactive);
      tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.resumed);
      await tester.pumpAndSettle();
      expect(reads, 2);
      expect(find.text('Before resume'), findsNothing);
      expect(find.text('After resume'), findsOneWidget);
    },
  );

  testWidgets('hidden Work neither polls nor refreshes until selected', (
    tester,
  ) async {
    final visibility = ValueNotifier(false);
    addTearDown(visibility.dispose);
    var reads = 0;
    await mount(tester, (_) async {
      reads++;
      return page([task('Visible after selection')]);
    }, visibility: visibility);
    expect(reads, 0);
    await tester.pump(const Duration(seconds: 60));
    expect(reads, 0);
    visibility.value = true;
    await tester.pumpAndSettle();
    expect(reads, 1);
  });
  testWidgets(
    'conversation removal updates an open form without submitting its old selection',
    (tester) async {
      final options = ValueNotifier<AsyncValue<List<TaskChannel>>>(
        const AsyncData([TaskChannel(id: 'room', name: 'General')]),
      );
      addTearDown(options.dispose);
      var writes = 0;
      await mount(
        tester,
        (request) async {
          if (request.method == 'POST') writes++;
          return page([]);
        },
        directory: eligibleDirectory(),
        channelOptions: options,
      );
      await fillAssignedTask(tester);
      options.value = const AsyncData([]);
      await tester.pumpAndSettle();
      expect(tester.takeException(), isNull);
      expect(find.text("Couldn't load your conversations."), findsOneWidget);
      expect(
        tester
            .widget<FilledButton>(
              find.widgetWithText(FilledButton, 'Create task'),
            )
            .onPressed,
        isNull,
      );
      expect(writes, 0);
    },
  );
  testWidgets(
    'two create invocations before the next frame send only one POST',
    (tester) async {
      final preflight = Completer<void>();
      var writes = 0;
      await mount(
        tester,
        (request) async {
          if (request.method == 'POST') {
            writes++;
            return http.Response('{"error":"title rejected"}', 400);
          }
          return page([]);
        },
        directory: eligibleDirectory(),
        refreshChannels: () => preflight.future,
      );
      await tester.tap(find.byTooltip('New task'));
      await tester.pumpAndSettle();
      await tester.enterText(
        find.byKey(const ValueKey('work-task-title')),
        'Review the update',
      );
      final callback = tester
          .widget<FilledButton>(
            find.widgetWithText(FilledButton, 'Create task'),
          )
          .onPressed!;
      callback();
      callback();
      // Both calls ran against the same built button; no pump/rebuild separates
      // them. The first call is waiting on actual preflight before either POST.
      preflight.complete();
      await tester.pumpAndSettle();
      expect(writes, 1);
    },
  );
}
