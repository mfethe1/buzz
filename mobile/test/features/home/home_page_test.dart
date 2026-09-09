import 'dart:async';
import '../../helpers/golden_shot.dart';
import 'package:flutter/rendering.dart';
import 'package:buzz/features/home/home_page.dart';
import 'package:buzz/features/computers/computers_page.dart';
import 'package:buzz/shared/machines/machines_api.dart';
import 'package:buzz/features/work/work_page.dart';
import 'package:buzz/features/channels/channel.dart';
import 'package:buzz/features/channels/channels_provider.dart';
import 'package:buzz/shared/tasks/tasks_api.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:nostr/nostr.dart' as nostr;
import 'package:buzz/features/channels/channels_page.dart';
import 'package:buzz/features/profile/profile_avatar.dart';
import 'package:buzz/shared/theme/theme.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:shared_preferences/shared_preferences.dart';

void main() {
  Future<Widget> buildHome({
    int unreadInboxCount = 0,
    bool includeWork = false,
    bool includeComputers = false,
    bool pendingChannels = false,
    double textScale = 1,
    bool disableAnimations = false,
    Gradient? topSectionGradient,
  }) async {
    SharedPreferences.setMockInitialValues({});
    final prefs = await SharedPreferences.getInstance();
    return ProviderScope(
      overrides: [
        savedPrefsProvider.overrideWithValue(prefs),
        if (includeWork || includeComputers)
          channelsProvider.overrideWith(
            pendingChannels ? _PendingChannels.new : _WorkChannels.new,
          ),
        if (includeComputers)
          machinesApiProvider.overrideWithValue(
            MachinesApi(
              httpClient: MockClient(
                (_) async =>
                    http.Response('{"machines":[],"next_cursor":null}', 200),
              ),
              baseUrl: 'https://computers.example',
              nsec: nostr.Keys.generate().nsec,
            ),
          ),
        if (includeWork)
          tasksApiProvider.overrideWithValue(
            TasksApi(
              httpClient: MockClient(
                (_) async =>
                    http.Response('{"tasks":[],"next_cursor":null}', 200),
              ),
              baseUrl: 'https://work.example',
              nsec: nostr.Keys.generate().nsec,
            ),
          ),
      ],
      child: MaterialApp(
        theme: AppTheme.light(topSectionGradient: topSectionGradient),
        builder: (context, child) => MediaQuery(
          data: MediaQuery.of(context).copyWith(
            disableAnimations: disableAnimations,
            textScaler: TextScaler.linear(textScale),
          ),
          child: child!,
        ),
        home: HomePage(
          settingsPageBuilder: _buildSettingsPage,
          hasUnreadInbox: unreadInboxCount > 0,
          computersPageBuilder: includeComputers
              ? (context, onBack, visible) =>
                    ComputersPage(onBack: onBack, visible: visible)
              : null,
          workPageBuilder: includeWork
              ? (context, onBack, visible) => WorkPage(
                  onBack: onBack,
                  visible: visible,
                  channels: const AsyncData([]),
                  onRefreshChannels: () async {},
                )
              : null,
        ),
      ),
    );
  }

  testWidgets(
    'phone shortcuts remain readable and usable while conversations load',
    (tester) async {
      await loadAppFonts();
      tester.view.physicalSize = const Size(390, 844);
      tester.view.devicePixelRatio = 1;
      addTearDown(tester.view.resetPhysicalSize);
      addTearDown(tester.view.resetDevicePixelRatio);
      await tester.pumpWidget(
        await buildHome(
          includeWork: true,
          includeComputers: true,
          pendingChannels: true,
        ),
      );
      await tester.pump();
      await tester.pump();
      final title = tester.renderObject<RenderParagraph>(
        find.text('Computers'),
      );
      expect(
        title.getBoxesForSelection(
          const TextSelection(baseOffset: 0, extentOffset: 9),
        ),
        hasLength(1),
      );
      expect(
        tester
            .getSize(find.byKey(const ValueKey('home-computers-entry')))
            .height,
        greaterThanOrEqualTo(48),
      );
      await tester.tap(find.byKey(const ValueKey('home-computers-entry')));
      await tester.pump();
      await tester.pump();
      expect(find.byType(ComputersPage), findsOneWidget);
      await tester.pumpWidget(const SizedBox.shrink());
      await tester.pumpWidget(
        await buildHome(
          includeWork: true,
          includeComputers: true,
          textScale: 2,
        ),
      );
      await tester.pumpAndSettle();
      expect(tester.takeException(), isNull);
      final work = tester.getRect(
        find.byKey(const ValueKey('home-work-entry')),
      );
      final computers = tester.getRect(
        find.byKey(const ValueKey('home-computers-entry')),
      );
      expect(computers.top, greaterThan(work.bottom));
    },
  );

  testWidgets(
    'Computers and Work preserve conversations and Home Activity Search navigation',
    (tester) async {
      await tester.pumpWidget(
        await buildHome(includeWork: true, includeComputers: true),
      );
      await tester.pumpAndSettle();
      final conversations = tester.element(find.byType(ChannelsPage));
      await tester.tap(find.byKey(const ValueKey('home-computers-entry')));
      await tester.pumpAndSettle();
      expect(find.byType(ComputersPage), findsOneWidget);
      expect(
        find.textContaining('No computers have been added'),
        findsOneWidget,
      );
      for (final label in ['Home', 'Activity', 'Search']) {
        expect(find.bySemanticsLabel(label), findsOneWidget);
      }
      await tester.tap(find.byTooltip('Back to Home'));
      await tester.pumpAndSettle();
      expect(tester.element(find.byType(ChannelsPage)), same(conversations));
      await tester.tap(find.byKey(const ValueKey('home-work-entry')));
      await tester.pumpAndSettle();
      expect(find.byType(WorkPage), findsOneWidget);
      await tester.tap(find.bySemanticsLabel('Home'));
      await tester.pumpAndSettle();
      expect(
        find.byKey(const ValueKey('home-computers-entry')),
        findsOneWidget,
      );
      expect(find.text('general'), findsOneWidget);
    },
  );

  testWidgets(
    'Work opens from Home while conversation state and all three tabs remain available',
    (tester) async {
      await tester.pumpWidget(await buildHome(includeWork: true));
      await tester.pumpAndSettle();
      final conversations = tester.element(find.byType(ChannelsPage));
      expect(find.text('general'), findsOneWidget);
      await tester.tap(find.byKey(const ValueKey('home-work-entry')));
      await tester.pumpAndSettle();
      expect(find.byType(WorkPage), findsOneWidget);
      expect(find.text('No tasks yet.'), findsOneWidget);
      for (final label in ['Home', 'Activity', 'Search']) {
        expect(find.bySemanticsLabel(label), findsOneWidget);
      }
      expect(
        find.byTooltip('Create or start conversation').hitTestable(),
        findsNothing,
      );
      await tester.tap(find.byTooltip('Back to Home'));
      await tester.pumpAndSettle();
      expect(find.text('general'), findsOneWidget);
      expect(tester.element(find.byType(ChannelsPage)), same(conversations));
      await tester.tap(find.byKey(const ValueKey('home-work-entry')));
      await tester.pumpAndSettle();
      await tester.tap(find.bySemanticsLabel('Home'));
      await tester.pumpAndSettle();
      expect(find.text('general'), findsOneWidget);
      expect(find.byType(WorkPage), findsNothing);
    },
  );

  testWidgets('shows icon-only navigation and an aligned quick action', (
    tester,
  ) async {
    await tester.pumpWidget(await buildHome());
    await tester.pump();

    expect(find.text('Home'), findsNothing);
    expect(find.text('Activity'), findsNothing);
    expect(find.text('Search'), findsNothing);
    expect(find.bySemanticsLabel('Home'), findsOneWidget);
    expect(find.bySemanticsLabel('Activity'), findsOneWidget);
    expect(find.bySemanticsLabel('Search'), findsOneWidget);

    final quickAction = find.byTooltip('Create or start conversation');
    expect(quickAction, findsOneWidget);
    final launcherSize = tester.getSize(
      find.byType(ChannelQuickActionsLauncher),
    );
    expect(launcherSize.width, 800);
    expect(launcherSize.height, greaterThan(0));
    final motionRect = tester.getRect(
      find.byKey(const Key('channel-quick-actions-motion')),
    );
    expect(motionRect.width, const Size.square(56).width);
    expect(motionRect.left, greaterThanOrEqualTo(0));
    expect(tester.getSize(quickAction), const Size.square(56));
    final quickActionRect = tester.getRect(quickAction);
    expect(quickActionRect.left, greaterThanOrEqualTo(0));
    expect(quickActionRect.top, greaterThanOrEqualTo(0));
    expect(quickActionRect.right, lessThanOrEqualTo(800));
    expect(quickActionRect.bottom, lessThanOrEqualTo(600));
    final homeDestinationRect = tester.getRect(find.bySemanticsLabel('Home'));
    expect(
      quickActionRect.center.dy,
      closeTo(homeDestinationRect.center.dy, 0.01),
    );
  });

  testWidgets('keeps the Buzz backdrop behind the scalable Home screen', (
    tester,
  ) async {
    const gradient = LinearGradient(
      begin: Alignment.topCenter,
      end: Alignment.bottomCenter,
      colors: [Colors.yellow, Colors.blue],
    );
    await tester.pumpWidget(await buildHome(topSectionGradient: gradient));
    await tester.pump();

    final backdrop = find.byKey(
      const ValueKey('home-settings-transition-backdrop'),
    );
    final decoration =
        tester.widget<DecoratedBox>(backdrop).decoration as BoxDecoration;
    expect(decoration.gradient, gradient);
    expect(
      find.byKey(const ValueKey('home-settings-transition-scale')),
      findsOneWidget,
    );
    expect(
      tester
          .widget<Transform>(
            find.byKey(const ValueKey('home-settings-transition-scale')),
          )
          .transform
          .getMaxScaleOnAxis(),
      1,
    );
    expect(
      tester
          .widget<Opacity>(
            find.byKey(const ValueKey('home-settings-transition-opacity')),
          )
          .opacity,
      1,
    );
  });

  testWidgets('keeps Home opaque beneath the Settings transition', (
    tester,
  ) async {
    await tester.pumpWidget(await buildHome());
    await tester.pumpAndSettle();

    await tester.tap(find.byType(ProfileAvatar));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 95));

    double homeOpacity() => tester
        .widget<Opacity>(
          find.byKey(const ValueKey('home-settings-transition-opacity')),
        )
        .opacity;

    expect(homeOpacity(), 1);

    await tester.pumpAndSettle();
    Navigator.of(
      tester.element(
        find.byKey(
          const ValueKey('settings-transition-opacity'),
          skipOffstage: false,
        ),
      ),
    ).pop();
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 95));

    expect(homeOpacity(), 1);
  });

  testWidgets('uses one monotonic route animation for Settings and Home', (
    tester,
  ) async {
    await tester.pumpWidget(await buildHome());
    await tester.pumpAndSettle();

    double homeScale() => tester
        .widget<Transform>(
          find.byKey(const ValueKey('home-settings-transition-scale')),
        )
        .transform
        .storage[0];

    await tester.tap(find.byType(ProfileAvatar));
    await tester.pump();

    final settingsTransition = find.byKey(
      const ValueKey('settings-transition-opacity'),
      skipOffstage: false,
    );
    final settingsRoute = ModalRoute.of(tester.element(settingsTransition));

    final entranceScales = <double>[homeScale()];
    final routeValues = <double>[settingsRoute!.animation!.value];
    for (var frame = 0; frame < 15; frame++) {
      await tester.pump(const Duration(milliseconds: 16));
      entranceScales.add(homeScale());
      routeValues.add(settingsRoute.animation!.value);
    }
    expect(entranceScales.first, closeTo(1, 0.000001));
    final reversalFrames = <int>[];
    for (var frame = 1; frame < entranceScales.length; frame++) {
      if (entranceScales[frame] > entranceScales[frame - 1] + 0.000001) {
        reversalFrames.add(frame);
      }
    }
    expect(
      reversalFrames,
      isEmpty,
      reason:
          'Home must scale down in one direction on entrance. '
          'scales=$entranceScales route=$routeValues',
    );
    expect(entranceScales, everyElement(inInclusiveRange(0.97, 1)));
    expect(entranceScales.last, closeTo(0.97, 0.001));

    await tester.pumpAndSettle();
    Navigator.of(tester.element(settingsTransition)).pop();
    await tester.pumpAndSettle();
  });

  testWidgets('gives selection haptics only when the tab changes', (
    tester,
  ) async {
    final hapticCalls = <MethodCall>[];
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(SystemChannels.platform, (call) async {
          if (call.method == 'HapticFeedback.vibrate') {
            hapticCalls.add(call);
          }
          return null;
        });
    addTearDown(
      () => TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
          .setMockMethodCallHandler(SystemChannels.platform, null),
    );

    await tester.pumpWidget(await buildHome());
    await tester.pump();

    await tester.tap(find.byTooltip('Home'));
    await tester.pump();
    expect(hapticCalls, isEmpty);

    await tester.tap(find.byTooltip('Activity'));
    await tester.pump();
    expect(hapticCalls, hasLength(1));
    expect(hapticCalls.single.arguments, 'HapticFeedbackType.selectionClick');

    await tester.tap(find.byTooltip('Activity'));
    await tester.pump();
    expect(hapticCalls, hasLength(1));

    await tester.tap(find.byTooltip('Search'));
    await tester.pump();
    expect(hapticCalls, hasLength(2));
  });

  testWidgets('gives a light impact when the Home quick action is pressed', (
    tester,
  ) async {
    final hapticCalls = <MethodCall>[];
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(SystemChannels.platform, (call) async {
          if (call.method == 'HapticFeedback.vibrate') {
            hapticCalls.add(call);
          }
          return null;
        });
    addTearDown(
      () => TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
          .setMockMethodCallHandler(SystemChannels.platform, null),
    );

    await tester.pumpWidget(await buildHome());
    await tester.pump();

    await tester.tap(find.byTooltip('Create or start conversation'));
    await tester.pump();

    expect(hapticCalls, hasLength(1));
    expect(hapticCalls.single.arguments, 'HapticFeedbackType.lightImpact');
  });

  testWidgets('badges the Inbox tab when it has unread rows', (tester) async {
    await tester.pumpWidget(await buildHome(unreadInboxCount: 1));
    await tester.pump();

    expect(
      find.byKey(const ValueKey('activity-tab-unread-dot')),
      findsOneWidget,
    );
    final badge = tester.widget<Container>(
      find.byKey(const ValueKey('activity-tab-unread-dot')),
    );
    expect(badge.constraints?.maxWidth, 12);
    expect(badge.constraints?.maxHeight, 12);
    expect(find.bySemanticsLabel('Activity, unread'), findsOneWidget);
    AnimatedScale unreadDotScale() => tester.widget<AnimatedScale>(
      find.byKey(const ValueKey('activity-tab-unread-dot-scale')),
    );
    expect(unreadDotScale().scale, 1);
    expect(unreadDotScale().alignment, const Alignment(-0.5, 0.5));
    expect(unreadDotScale().duration, const Duration(milliseconds: 220));

    await tester.tap(find.byTooltip('Activity'));
    await tester.pump();

    expect(
      find.byKey(const ValueKey('activity-tab-unread-dot')),
      findsOneWidget,
    );
    expect(unreadDotScale().scale, 0);
    expect(find.bySemanticsLabel('Activity, unread'), findsNothing);

    await tester.tap(find.byTooltip('Home'));
    await tester.pump();

    expect(unreadDotScale().scale, 1);
  });

  testWidgets('fades and slides tab content in the selected direction', (
    tester,
  ) async {
    await tester.pumpWidget(await buildHome());
    await tester.pump();

    Transform bodyTransform() => tester.widget<Transform>(
      find.byKey(const ValueKey('frosted-scaffold-body-transition-transform')),
    );
    Opacity bodyOpacity() => tester.widget<Opacity>(
      find.byKey(const ValueKey('frosted-scaffold-body-transition-opacity')),
    );
    Transform appBarTransform() => tester.widget<Transform>(
      find.byKey(
        const ValueKey('frosted-app-bar-content-transition-transform'),
      ),
    );
    Opacity appBarOpacity() => tester.widget<Opacity>(
      find.byKey(const ValueKey('frosted-app-bar-content-transition-opacity')),
    );
    double bodyOffset() => bodyTransform().transform.getTranslation().x;
    double appBarOffset() => appBarTransform().transform.getTranslation().x;

    expect(bodyOffset(), closeTo(0, 0.001));
    expect(appBarOffset(), closeTo(0, 0.001));
    expect(bodyOpacity().opacity, closeTo(1, 0.001));
    expect(appBarOpacity().opacity, closeTo(1, 0.001));

    await tester.tap(find.byTooltip('Activity'));
    await tester.pump();

    expect(bodyOffset(), closeTo(24, 0.001));
    expect(appBarOffset(), closeTo(24, 0.001));
    expect(bodyOpacity().opacity, closeTo(0, 0.001));
    expect(appBarOpacity().opacity, closeTo(0, 0.001));
    expect(
      find.descendant(
        of: find.byKey(const ValueKey('frosted-app-bar-background')),
        matching: find.byKey(
          const ValueKey('frosted-app-bar-content-transition-transform'),
        ),
      ),
      findsOneWidget,
    );

    await tester.pump(const Duration(milliseconds: 120));

    expect(bodyOffset(), inExclusiveRange(0, 24));
    expect(appBarOffset(), inExclusiveRange(0, 24));
    expect(bodyOpacity().opacity, inExclusiveRange(0, 1));
    expect(appBarOpacity().opacity, inExclusiveRange(0, 1));

    await tester.pumpAndSettle();
    await tester.tap(find.byTooltip('Home'));
    await tester.pump();

    expect(bodyOffset(), closeTo(-24, 0.001));
    expect(appBarOffset(), closeTo(-24, 0.001));
    expect(bodyOpacity().opacity, closeTo(0, 0.001));
    expect(appBarOpacity().opacity, closeTo(0, 0.001));

    await tester.pumpAndSettle();
    expect(bodyOffset(), closeTo(0, 0.001));
    expect(appBarOffset(), closeTo(0, 0.001));
    expect(bodyOpacity().opacity, closeTo(1, 0.001));
    expect(appBarOpacity().opacity, closeTo(1, 0.001));
  });

  testWidgets('switches tab content instantly with reduced motion', (
    tester,
  ) async {
    await tester.pumpWidget(await buildHome(disableAnimations: true));
    await tester.pump();

    await tester.tap(find.byTooltip('Activity'));
    await tester.pump();

    final bodyTransform = tester.widget<Transform>(
      find.byKey(const ValueKey('frosted-scaffold-body-transition-transform')),
    );
    final bodyOpacity = tester.widget<Opacity>(
      find.byKey(const ValueKey('frosted-scaffold-body-transition-opacity')),
    );
    final appBarTransform = tester.widget<Transform>(
      find.byKey(
        const ValueKey('frosted-app-bar-content-transition-transform'),
      ),
    );
    final appBarOpacity = tester.widget<Opacity>(
      find.byKey(const ValueKey('frosted-app-bar-content-transition-opacity')),
    );
    expect(bodyTransform.transform.getTranslation().x, closeTo(0, 0.001));
    expect(appBarTransform.transform.getTranslation().x, closeTo(0, 0.001));
    expect(bodyOpacity.opacity, closeTo(1, 0.001));
    expect(appBarOpacity.opacity, closeTo(1, 0.001));
  });

  testWidgets('scales and fades the quick action as tabs change', (
    tester,
  ) async {
    await tester.pumpWidget(await buildHome());
    await tester.pump();

    double scale() => tester
        .widget<Transform>(find.byKey(const Key('channel-quick-actions-scale')))
        .transform
        .storage
        .first;
    double opacity() => tester
        .widget<Opacity>(find.byKey(const Key('channel-quick-actions-opacity')))
        .opacity;

    expect(scale(), closeTo(1, 0.001));
    expect(opacity(), closeTo(1, 0.001));

    await tester.tap(find.byTooltip('Activity'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 110));

    expect(scale(), inExclusiveRange(0.8, 1));
    expect(opacity(), inExclusiveRange(0, 1));

    await tester.pumpAndSettle();
    expect(scale(), closeTo(0.8, 0.001));
    expect(opacity(), closeTo(0, 0.001));

    await tester.tap(find.byTooltip('Home'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 110));

    expect(scale(), inExclusiveRange(0.8, 1));
    expect(opacity(), inExclusiveRange(0, 1));

    await tester.pumpAndSettle();
    expect(scale(), closeTo(1, 0.001));
    expect(opacity(), closeTo(1, 0.001));
  });
}

Widget _buildSettingsPage(BuildContext context) => const SizedBox.shrink();

class _WorkChannels extends ChannelsNotifier {
  @override
  Future<List<Channel>> build() async => [
    Channel(
      id: 'room',
      name: 'general',
      description: '',
      createdBy: 'owner',
      createdAt: DateTime.utc(2026),
      memberCount: 1,
      channelType: 'stream',
      visibility: 'private',
      isMember: true,
    ),
  ];
}

class _PendingChannels extends ChannelsNotifier {
  @override
  Future<List<Channel>> build() => Completer<List<Channel>>().future;
}
