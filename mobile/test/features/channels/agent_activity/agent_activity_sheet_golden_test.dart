// FORK-ONLY evidence test — requires test/helpers/golden_shot.dart, which is
// not on upstream main. Upstream PRs must not carry this file. It exists to
// keep the HW-014 golden captures honest over time in the fork.
import 'dart:async';

import 'package:flutter_test/flutter_test.dart';
import 'package:nostr/nostr.dart' as nostr;

import 'package:buzz/features/channels/agent_activity/agent_activity_sheet.dart';
import 'package:buzz/shared/profile/user_cache_provider.dart';
import 'package:buzz/shared/profile/user_profile.dart';
import 'package:buzz/shared/relay/relay.dart';

import '../../../helpers/golden_shot.dart';
import '../../../helpers/widget_helpers.dart';

void main() {
  testWidgets('01 error state with retry affordance (golden)', (tester) async {
    await loadAppFonts();
    setPhoneSurface(tester);

    final ownerKeychain = nostr.Keys.generate();
    final agentKeychain = nostr.Keys.generate();
    final session = _GoldenRelaySession();

    await tester.pumpWidget(
      WidgetHelpers.testable(
        overrides: [
          relaySessionProvider.overrideWith(() => session),
          relayConfigProvider.overrideWith(
            () => _FakeRelayConfigNotifier(ownerKeychain.nsec),
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
        child: AgentActivitySheet(
          channelId: 'test-channel',
          agentPubkey: agentKeychain.public,
        ),
      ),
    );
    await tester.pump();
    await tester.pump();

    session.closeAll('boom');
    await tester.pump();

    await captureShot(
      tester,
      find.byType(AgentActivitySheet),
      '01-hw014-error-state-with-retry',
      settle: false,
    );
  });

  testWidgets('02 post-tap connecting state (golden)', (tester) async {
    await loadAppFonts();
    setPhoneSurface(tester);

    final ownerKeychain = nostr.Keys.generate();
    final agentKeychain = nostr.Keys.generate();
    final session = _GoldenRelaySession();

    await tester.pumpWidget(
      WidgetHelpers.testable(
        overrides: [
          relaySessionProvider.overrideWith(() => session),
          relayConfigProvider.overrideWith(
            () => _FakeRelayConfigNotifier(ownerKeychain.nsec),
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
        child: AgentActivitySheet(
          channelId: 'test-channel',
          agentPubkey: agentKeychain.public,
        ),
      ),
    );
    await tester.pump();
    await tester.pump();

    session.closeAll('boom');
    await tester.pump();
    expect(find.text('Try again'), findsOneWidget);

    session.gateNextSubscribe = true;
    await tester.tap(find.text('Try again'));
    await tester.pump();

    await captureShot(
      tester,
      find.byType(AgentActivitySheet),
      '02-hw014-post-tap-connecting',
      settle: false,
    );
  });
}

class _GoldenRelaySession extends RelaySessionNotifier {
  final List<NostrFilter> filters = [];
  final List<void Function(NostrEvent)> _listeners = [];
  final List<void Function(String message)> _closedListeners = [];
  final List<Completer<void>> _subscribeGates = [];
  bool gateNextSubscribe = false;

  @override
  SessionState build() => const SessionState(status: SessionStatus.connected);

  @override
  Future<void Function()> subscribe(
    NostrFilter filter,
    void Function(NostrEvent) onEvent, {
    void Function(String message)? onClosed,
  }) async {
    filters.add(filter);
    if (gateNextSubscribe) {
      gateNextSubscribe = false;
      final gate = Completer<void>();
      _subscribeGates.add(gate);
      await gate.future;
    }
    _listeners.add(onEvent);
    if (onClosed != null) {
      _closedListeners.add(onClosed);
    }
    return () {
      filters.remove(filter);
      _listeners.remove(onEvent);
      if (onClosed != null) {
        _closedListeners.remove(onClosed);
      }
    };
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
