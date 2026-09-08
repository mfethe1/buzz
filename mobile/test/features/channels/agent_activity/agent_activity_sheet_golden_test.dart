// FORK-ONLY evidence test — requires test/helpers/golden_shot.dart, which is
// not on upstream main. Upstream PRs must not carry this file. It exists to
// keep the HW-014 golden captures honest over time in the fork.
//
// Coverage matrix (DIGEST_CONTRACT visual evidence):
//
//   #  | scenario            | surface        | theme
//   ---+---------------------+----------------+-------
//   01 | error + retry       | phone 390x844  | light
//   02 | post-tap connecting | phone 390x844  | light
//   03 | populated transcript| phone 390x844  | light
//   04 | error + retry       | phone 390x844  | dark
//   05 | populated transcript| tablet 834x1112| light
//   06 | error + retry       | tablet 834x1112| dark
//   07 | empty / waiting     | phone 390x844  | light
//   08 | idle / not connected| phone 390x844  | light
//
// This file intentionally builds its own ProviderScope/MaterialApp rather than
// using test/helpers/widget_helpers.dart: that helper IS carried on upstream
// main and hardcodes AppTheme.light(), and adding a theme parameter to it would
// mean carrying a fork delta on an upstream-owned file purely for evidence.
import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:hooks_riverpod/misc.dart';
import 'package:nostr/nostr.dart' as nostr;

import 'package:buzz/features/channels/agent_activity/agent_activity_sheet.dart';
import 'package:buzz/shared/crypto/nip44.dart';
import 'package:buzz/shared/profile/user_cache_provider.dart';
import 'package:buzz/shared/profile/user_profile.dart';
import 'package:buzz/shared/relay/relay.dart';
import 'package:buzz/shared/theme/theme.dart';

import '../../../helpers/golden_shot.dart';
import '../../../helpers/golden_renderer.dart';

/// Tablet capture surface (iPad-class logical size, portrait).
const Size kTabletLogicalSize = Size(834, 1112);

late String _goldenDirectory;
String _golden(String name) =>
    _goldenDirectory.isEmpty ? name : '$_goldenDirectory/$name';

void main() {
  setUpAll(() {
    final profiles =
        jsonDecode(
              File(
                '${mobilePackageRoot()}/test/features/channels/agent_activity/goldens/renderer_profiles.json',
              ).readAsStringSync(),
            )
            as List<dynamic>;
    final actual = readGoldenRendererFingerprint();
    final selection = selectGoldenRenderer(
      actual,
      profiles,
      updating: autoUpdateGoldenFiles,
    );
    _goldenDirectory = selection.directory;
    if (!selection.qualified) {
      // This preserves the prior canonical comparison on hosts such as CI's
      // Linux runner; it is not qualification of that renderer fingerprint.
      debugPrint(
        'Unqualified renderer: comparing canonical golden images strictly. '
        'Updates are disabled. ${jsonEncode(actual)}',
      );
    }
  });
  // -------------------------------------------------------------------------
  // 01 / 02 — the original two captures, unchanged so their hashes stay stable.
  // -------------------------------------------------------------------------

  testWidgets('01 error state with retry affordance (golden)', (tester) async {
    final rig = await _pumpSheet(tester);
    rig.session.closeAll('boom');
    await tester.pump();

    expect(find.text('Try again'), findsOneWidget);
    await captureShot(
      tester,
      find.byType(AgentActivitySheet),
      _golden('01-hw014-error-state-with-retry'),
      settle: false,
    );
  });

  testWidgets('02 post-tap connecting state (golden)', (tester) async {
    final rig = await _pumpSheet(tester);
    rig.session.closeAll('boom');
    await tester.pump();
    expect(find.text('Try again'), findsOneWidget);

    rig.session.gateNextSubscribe = true;
    await tester.tap(find.text('Try again'));
    await tester.pump();

    await captureShot(
      tester,
      find.byType(AgentActivitySheet),
      _golden('02-hw014-post-tap-connecting'),
      settle: false,
    );
  });

  // -------------------------------------------------------------------------
  // 03 — populated: the recovered transcript, which is the actual point of the
  // fix. Frames arrive on the NEW subscription after a retry.
  // -------------------------------------------------------------------------

  testWidgets('03 populated transcript after recovery (golden)', (
    tester,
  ) async {
    final rig = await _pumpSheet(tester);

    rig.session.closeAll('boom');
    await tester.pump();
    await tester.tap(find.text('Try again'));
    await tester.pump();
    await tester.pump();
    expect(find.text('Try again'), findsNothing);

    await _emitTranscript(tester, rig);

    expect(find.byType(ListView), findsOneWidget);
    await captureShot(
      tester,
      find.byType(AgentActivitySheet),
      _golden('03-hw014-populated-after-recovery'),
      settle: false,
    );
  });

  // -------------------------------------------------------------------------
  // 04 — the error state in dark theme. The retry affordance is drawn with
  // colors.error and a tonal filled button; both are theme-derived, so a dark
  // capture is the only thing that proves the affordance stays legible.
  // -------------------------------------------------------------------------

  testWidgets('04 error state with retry affordance, dark (golden)', (
    tester,
  ) async {
    final rig = await _pumpSheet(tester, brightness: Brightness.dark);
    rig.session.closeAll('boom');
    await tester.pump();

    expect(find.text('Try again'), findsOneWidget);
    await captureShot(
      tester,
      find.byType(AgentActivitySheet),
      _golden('04-hw014-error-state-with-retry-dark'),
      settle: false,
    );
  });

  // -------------------------------------------------------------------------
  // 05 / 06 — wide surface. DraggableScrollableSheet sizes off the viewport, so
  // a tablet capture is a genuinely different layout, not a rescale.
  // -------------------------------------------------------------------------

  testWidgets('05 populated transcript, tablet (golden)', (tester) async {
    final rig = await _pumpSheet(tester, logical: kTabletLogicalSize);

    rig.session.closeAll('boom');
    await tester.pump();
    await tester.tap(find.text('Try again'));
    await tester.pump();
    await tester.pump();

    await _emitTranscript(tester, rig);

    expect(find.byType(ListView), findsOneWidget);
    await captureShot(
      tester,
      find.byType(AgentActivitySheet),
      _golden('05-hw014-populated-tablet'),
      settle: false,
    );
  });

  testWidgets('06 error state with retry affordance, tablet dark (golden)', (
    tester,
  ) async {
    final rig = await _pumpSheet(
      tester,
      logical: kTabletLogicalSize,
      brightness: Brightness.dark,
    );
    rig.session.closeAll('boom');
    await tester.pump();

    expect(find.text('Try again'), findsOneWidget);
    await captureShot(
      tester,
      find.byType(AgentActivitySheet),
      _golden('06-hw014-error-state-tablet-dark'),
      settle: false,
    );
  });

  // -------------------------------------------------------------------------
  // 07 / 08 — the two non-error empty states, captured so a reviewer can see
  // that the retry affordance is absent when it should be. A screenshot set
  // that only shows the error state cannot demonstrate that.
  // -------------------------------------------------------------------------

  testWidgets('07 healthy empty/waiting state (golden)', (tester) async {
    await _pumpSheet(tester);

    expect(find.text('Waiting for activity\u2026'), findsOneWidget);
    expect(find.text('Try again'), findsNothing);
    await captureShot(
      tester,
      find.byType(AgentActivitySheet),
      _golden('07-hw014-empty-waiting'),
      settle: false,
    );
  });

  testWidgets('08 idle / not connected state (golden)', (tester) async {
    // No signing key -> the notifier reports idle, never error, so there must
    // be no retry affordance: retrying cannot fix a missing key.
    await _pumpSheet(tester, nsec: '');

    expect(find.text('Not connected'), findsOneWidget);
    expect(find.text('Try again'), findsNothing);
    await captureShot(
      tester,
      find.byType(AgentActivitySheet),
      _golden('08-hw014-idle-not-connected'),
      settle: false,
    );
  });
}

// ---------------------------------------------------------------------------
// Rig
// ---------------------------------------------------------------------------

class _Rig {
  final _GoldenRelaySession session;
  final nostr.Keys ownerKeychain;
  final nostr.Keys agentKeychain;

  _Rig(this.session, this.ownerKeychain, this.agentKeychain);
}

/// Pumps the sheet at [logical] size in [brightness], with fonts loaded.
///
/// Keys are generated from a fixed seed-free path but the captures never render
/// raw pubkeys (the seeded user cache supplies the display name), so the shots
/// stay byte-reproducible across runs.
Future<_Rig> _pumpSheet(
  WidgetTester tester, {
  Size logical = kPhoneLogicalSize,
  Brightness brightness = Brightness.light,
  String? nsec,
}) async {
  await loadAppFonts();
  setPhoneSurface(tester, logical: logical);

  final ownerKeychain = nostr.Keys.generate();
  final agentKeychain = nostr.Keys.generate();
  final session = _GoldenRelaySession();

  await tester.pumpWidget(
    ProviderScope(
      overrides: <Override>[
        relaySessionProvider.overrideWith(() => session),
        relayConfigProvider.overrideWith(
          () => _FakeRelayConfigNotifier(nsec ?? ownerKeychain.nsec),
        ),
        userCacheProvider.overrideWith(
          () => _SeededUserCacheNotifier({
            agentKeychain.public.toLowerCase(): UserProfile(
              pubkey: agentKeychain.public,
              displayName: 'Test Agent',
            ),
          }),
        ),
      ],
      child: MaterialApp(
        theme: brightness == Brightness.dark
            ? AppTheme.dark()
            : AppTheme.light(),
        home: Scaffold(
          body: AgentActivitySheet(
            channelId: 'test-channel',
            agentPubkey: agentKeychain.public,
          ),
        ),
      ),
    ),
  );
  await tester.pump();
  await tester.pump();

  return _Rig(session, ownerKeychain, agentKeychain);
}

/// Emits a fixed, representative transcript: a lifecycle event, a user prompt,
/// an assistant reply and an agent thought. Fixed timestamps and content keep
/// the capture reproducible.
Future<void> _emitTranscript(WidgetTester tester, _Rig rig) async {
  for (final payload in _transcriptFrames) {
    rig.session.emit(
      _observerEvent(
        ownerKeychain: rig.ownerKeychain,
        agentKeychain: rig.agentKeychain,
        payload: payload,
      ),
    );
    await tester.pump();
  }
}

const List<Map<String, dynamic>> _transcriptFrames = <Map<String, dynamic>>[
  {
    'seq': 1,
    'timestamp': '2026-09-06T09:00:01.000Z',
    'kind': 'turn_started',
    'channelId': 'test-channel',
    'turnId': 'turn-1',
    'payload': {
      'triggeringEventIds': ['1'],
    },
  },
  {
    'seq': 2,
    'timestamp': '2026-09-06T09:00:02.000Z',
    'kind': 'acp_read',
    'channelId': 'test-channel',
    'turnId': 'turn-1',
    'payload': {
      'method': 'session/update',
      'params': {
        'update': {
          'sessionUpdate': 'user_message_chunk',
          'messageId': 'm-1',
          'content': {
            'type': 'text',
            'text': 'Can you check the build status?',
          },
        },
      },
    },
  },
  {
    'seq': 3,
    'timestamp': '2026-09-06T09:00:03.000Z',
    'kind': 'acp_read',
    'channelId': 'test-channel',
    'turnId': 'turn-1',
    'payload': {
      'method': 'session/update',
      'params': {
        'update': {
          'sessionUpdate': 'agent_thought_chunk',
          'messageId': 'm-2',
          'content': {
            'type': 'text',
            'text': 'The subscription dropped, so I reconnected first.',
          },
        },
      },
    },
  },
  {
    'seq': 4,
    'timestamp': '2026-09-06T09:00:04.000Z',
    'kind': 'acp_read',
    'channelId': 'test-channel',
    'turnId': 'turn-1',
    'payload': {
      'method': 'session/update',
      'params': {
        'update': {
          'sessionUpdate': 'agent_message_chunk',
          'messageId': 'm-3',
          'content': {
            'type': 'text',
            'text': 'The build is green: 126 db, 262 core, 968 acp tests pass.',
          },
        },
      },
    },
  },
];

NostrEvent _observerEvent({
  required nostr.Keys ownerKeychain,
  required nostr.Keys agentKeychain,
  required Map<String, dynamic> payload,
}) {
  final conversationKey = getConversationKey(
    agentKeychain.secret,
    ownerKeychain.public,
  );
  final event = nostr.Event.from(
    kind: EventKind.agentObserverFrame,
    content: nip44Encrypt(conversationKey, jsonEncode(payload)),
    tags: [
      ['p', ownerKeychain.public],
      ['agent', agentKeychain.public],
      ['frame', 'telemetry'],
    ],
    secretKey: agentKeychain.secret,
    verify: false,
  );
  return NostrEvent.fromJson(event.toMap());
}

class _GoldenRelaySession extends RelaySessionNotifier {
  final List<void Function(NostrEvent)> _listeners = [];
  final List<void Function(String message)> _closedListeners = [];

  /// When set, the next [subscribe] never resolves, freezing the sheet in its
  /// connecting state so capture 02 can photograph it.
  bool gateNextSubscribe = false;

  @override
  SessionState build() => const SessionState(status: SessionStatus.connected);

  @override
  Future<void Function()> subscribe(
    NostrFilter filter,
    void Function(NostrEvent) onEvent, {
    void Function(String message)? onClosed,
  }) async {
    if (gateNextSubscribe) {
      gateNextSubscribe = false;
      await Completer<void>().future;
    }
    _listeners.add(onEvent);
    if (onClosed != null) {
      _closedListeners.add(onClosed);
    }
    return () {
      _listeners.remove(onEvent);
      if (onClosed != null) {
        _closedListeners.remove(onClosed);
      }
    };
  }

  void emit(NostrEvent event) {
    for (final listener in List.of(_listeners)) {
      listener(event);
    }
  }

  void closeAll(String message) {
    for (final listener in List.of(_closedListeners)) {
      listener(message);
    }
    _listeners.clear();
    _closedListeners.clear();
  }
}

class _FakeRelayConfigNotifier extends RelayConfigNotifier {
  final String? _nsec;

  _FakeRelayConfigNotifier(this._nsec);

  @override
  RelayConfig build() =>
      RelayConfig(baseUrl: 'http://localhost:3000', nsec: _nsec);
}

class _SeededUserCacheNotifier extends UserCacheNotifier {
  final Map<String, UserProfile> _seed;

  _SeededUserCacheNotifier(this._seed);

  @override
  Map<String, UserProfile> build() => _seed;

  @override
  Future<bool> preload(List<String> pubkeys) async => true;
}
