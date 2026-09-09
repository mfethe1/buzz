import 'dart:async';
import 'dart:convert';
import 'package:buzz/features/computers/computers_page.dart';
import 'package:buzz/shared/machines/computer_clock.dart';
import 'package:buzz/shared/community/community.dart';
import 'package:buzz/shared/community/community_provider.dart';
import 'package:buzz/shared/machines/machines_api.dart';
import 'package:buzz/shared/relay/relay.dart';
import 'package:buzz/shared/theme/theme.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:hooks_riverpod/legacy.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:nostr/nostr.dart' as nostr;
import '../../shared/machines/machines_api_test.dart'
    show computerJson, computerId;

final _selected = StateProvider<Future<Community?>>(
  (ref) => Future.value(null),
);
final _visible = StateProvider<bool>((ref) => true);
http.Response page(List<Map<String, dynamic>> rows, [String? cursor]) =>
    http.Response(jsonEncode({'machines': rows, 'next_cursor': cursor}), 200);
void main() {
  late ProviderContainer container;
  final keys = nostr.Keys.generate();
  final owner = pubkeyFromNsec(keys.nsec)!;
  final community = Community.create(
    name: 'Community one',
    relayUrl: 'wss://one.example',
    nsec: keys.nsec,
    pubkey: owner,
  );
  Future<void> mount(
    WidgetTester tester,
    Future<http.Response> Function(http.Request) handler, {
    Duration Function()? clock,
  }) async {
    container = ProviderContainer(
      overrides: [
        _selected.overrideWith((ref) => Future.value(community)),
        activeCommunityProvider.overrideWith((ref) => ref.watch(_selected)),
        machinesHttpClientProvider.overrideWithValue(MockClient(handler)),
        if (clock != null) computerClockProvider.overrideWithValue(clock),
      ],
    );
    addTearDown(container.dispose);
    await tester.pumpWidget(
      UncontrolledProviderScope(
        container: container,
        child: MaterialApp(
          theme: AppTheme.light(),
          home: Consumer(
            builder: (context, ref, _) =>
                ComputersPage(onBack: () {}, visible: ref.watch(_visible)),
          ),
        ),
      ),
    );
    await tester.pump();
    await tester.pump();
  }

  testWidgets('skewed local wall clock cannot extend a fresh report', (
    tester,
  ) async {
    final serverNow = DateTime.utc(2026, 9, 9);
    var elapsed = const Duration(minutes: 10);
    await mount(
      tester,
      (_) async => page([computerJson(owner, observed: true, now: serverNow)]),
      clock: () => elapsed,
    );
    await tester.pumpAndSettle();
    expect(find.textContaining('Reported ready'), findsOneWidget);
    elapsed += const Duration(seconds: 120);
    await tester.pump(const Duration(seconds: 120, milliseconds: 2));
    expect(find.textContaining('Reported ready'), findsNothing);
    expect(find.textContaining('Update expired'), findsOneWidget);
  });
  testWidgets(
    'detail authorization loss clears the parent private list before back',
    (tester) async {
      await mount(
        tester,
        (r) async => r.url.path.endsWith(computerId)
            ? http.Response('{}', 403)
            : page([computerJson(owner)], computerId),
      );
      await tester.pumpAndSettle();
      await tester.tap(find.text('Office computer'));
      await tester.pumpAndSettle();
      expect(
        find.textContaining('access to this community changed'),
        findsOneWidget,
      );
      await tester.tap(find.byTooltip('Back to Computers'));
      await tester.pumpAndSettle();
      expect(find.text('Office computer'), findsNothing);
      expect(find.text('Load more'), findsNothing);
    },
  );
  testWidgets(
    'signed pagination, detail and reload show actual runtime and observation',
    (tester) async {
      var detailReads = 0;
      final requests = <http.Request>[];
      await mount(tester, (request) async {
        requests.add(request);
        expect(request.headers['authorization'], startsWith('Nostr '));
        if (request.url.path.endsWith(computerId)) {
          detailReads++;
          return http.Response(
            jsonEncode(
              computerJson(
                owner,
                label: detailReads == 1
                    ? 'Office computer'
                    : 'Renamed computer',
              ),
            ),
            200,
          );
        }
        return request.url.queryParameters.containsKey('after')
            ? page([
                computerJson(
                  owner,
                  id: '00000000-0000-0000-0000-000000000002',
                  label: 'Travel laptop',
                ),
              ])
            : page([computerJson(owner)], computerId);
      });
      await tester.pumpAndSettle();
      await tester.tap(find.text('Load more'));
      await tester.pumpAndSettle();
      expect(find.text('Travel laptop'), findsOneWidget);
      expect(requests.last.url.queryParameters['after'], computerId);
      await tester.tap(find.text('Office computer'));
      await tester.pumpAndSettle();
      expect(detailReads, 1);
      expect(find.text('Agent runtime'), findsOneWidget);
      expect(find.text('Hermes'), findsOneWidget);
      expect(find.text('No update received'), findsOneWidget);
      await tester.tap(find.byTooltip('Refresh computer'));
      await tester.pumpAndSettle();
      expect(find.text('Renamed computer'), findsOneWidget);
      expect(detailReads, 2);
      expect(find.text('Assign work'), findsNothing);
      expect(find.text('Connect'), findsNothing);
    },
  );
  testWidgets('cached ready label expires while the screen stays open', (
    tester,
  ) async {
    final now = DateTime.utc(2026, 9, 9);
    var elapsed = Duration.zero;
    final record = computerJson(owner, observed: true, now: now);
    await mount(tester, (_) async => page([record]), clock: () => elapsed);
    await tester.pumpAndSettle();
    expect(find.textContaining('Reported ready'), findsOneWidget);
    elapsed = const Duration(seconds: 120);
    await tester.pump(const Duration(seconds: 120, milliseconds: 2));
    expect(find.textContaining('Update expired'), findsOneWidget);
    expect(find.textContaining('Reported ready'), findsNothing);
  });
  testWidgets(
    'page failure retries the same cursor without leaking server errors',
    (tester) async {
      var reads = 0;
      await mount(tester, (request) async {
        if (!request.url.queryParameters.containsKey('after')) {
          return page([computerJson(owner)], computerId);
        }
        reads++;
        return reads == 1
            ? http.Response('private SQL error', 500)
            : page([
                computerJson(
                  owner,
                  id: '00000000-0000-0000-0000-000000000002',
                  label: 'Recovered laptop',
                ),
              ]);
      });
      await tester.pumpAndSettle();
      await tester.tap(find.text('Load more'));
      await tester.pumpAndSettle();
      expect(find.text('Office computer'), findsOneWidget);
      expect(find.textContaining('private SQL'), findsNothing);
      await tester.tap(find.text('Try again'));
      await tester.pumpAndSettle();
      expect(find.text('Recovered laptop'), findsOneWidget);
      expect(reads, 2);
    },
  );
  testWidgets(
    'pending community transition clears old list before completion',
    (tester) async {
      await mount(
        tester,
        (_) async => page([computerJson(owner, label: 'Private computer')]),
      );
      await tester.pumpAndSettle();
      expect(find.text('Private computer'), findsOneWidget);
      final next = Completer<Community?>();
      container.read(_selected.notifier).state = next.future;
      await tester.pump();
      await tester.pump();
      expect(find.text('Private computer'), findsNothing);
      next.complete(null);
      await tester.pumpAndSettle();
      expect(find.textContaining('Choose a community'), findsOneWidget);
    },
  );
  testWidgets(
    'late list from a previous community cannot appear after switch',
    (tester) async {
      final pending = Completer<http.Response>();
      await mount(
        tester,
        (r) async => r.url.host == 'one.example' ? pending.future : page([]),
      );
      container.read(_selected.notifier).state = Future.value(
        community.copyWith(relayUrl: 'wss://two.example'),
      );
      await tester.pump();
      await tester.pump();
      pending.complete(page([computerJson(owner, label: 'Private computer')]));
      await tester.pumpAndSettle();
      expect(find.text('Private computer'), findsNothing);
      expect(
        find.textContaining('You haven’t added any computers'),
        findsOneWidget,
      );
    },
  );
  testWidgets(
    'late detail from a previous identity cannot appear after switch',
    (tester) async {
      final pending = Completer<http.Response>();
      final nextKey = nostr.Keys.generate();
      await mount(
        tester,
        (r) async => r.url.path.endsWith(computerId)
            ? pending.future
            : page([computerJson(owner)]),
      );
      await tester.pumpAndSettle();
      await tester.tap(find.text('Office computer'));
      await tester.pump();
      container.read(_selected.notifier).state = Future.value(
        community.copyWith(
          nsec: nextKey.nsec,
          pubkey: pubkeyFromNsec(nextKey.nsec),
        ),
      );
      await tester.pump();
      await tester.pump();
      pending.complete(
        http.Response(
          jsonEncode(computerJson(owner, label: 'Private details')),
          200,
        ),
      );
      await tester.pumpAndSettle();
      expect(find.text('Private details'), findsNothing);
      expect(find.text('Computer details'), findsNothing);
      expect(find.text('Office computer'), findsNothing);
    },
  );
  testWidgets(
    '401 and unavailable detail have usable account and retry states',
    (tester) async {
      var denied = false;
      await mount(
        tester,
        (r) async => r.url.path.endsWith(computerId)
            ? http.Response('{}', 404)
            : denied
            ? http.Response('{}', 401)
            : page([computerJson(owner)]),
      );
      await tester.pumpAndSettle();
      await tester.tap(find.text('Office computer'));
      await tester.pumpAndSettle();
      expect(find.textContaining('no longer available'), findsOneWidget);
      await tester.tap(find.byTooltip('Back to Computers'));
      await tester.pumpAndSettle();
      denied = true;
      await tester.tap(find.byTooltip('Refresh computers'));
      await tester.pumpAndSettle();
      expect(
        find.textContaining('access to this community changed'),
        findsOneWidget,
      );
      expect(find.text('Office computer'), findsNothing);
    },
  );
  testWidgets('returning to visible detail and resuming refetches it', (
    tester,
  ) async {
    var reads = 0;
    await mount(tester, (r) async {
      if (r.url.path.endsWith(computerId)) {
        reads++;
        return http.Response(
          jsonEncode(computerJson(owner, label: 'Update $reads')),
          200,
        );
      }
      return page([computerJson(owner)]);
    });
    await tester.pumpAndSettle();
    await tester.tap(find.text('Office computer'));
    await tester.pumpAndSettle();
    expect(reads, 1);
    container.read(_visible.notifier).state = false;
    await tester.pump();
    container.read(_visible.notifier).state = true;
    await tester.pumpAndSettle();
    expect(reads, 2);
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.inactive);
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.resumed);
    await tester.pumpAndSettle();
    expect(reads, 3);
    expect(find.text('Update 3'), findsOneWidget);
  });
}
