import 'package:buzz/shared/relay/relay.dart';
import 'package:buzz/shared/auth/auth_provider.dart';
import 'package:buzz/shared/tasks/tasks_sync.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

void main() {
  test(
    'task invalidation requires a connected session and one channel UUID',
    () {
      final container = ProviderContainer(
        overrides: [authProvider.overrideWith(_Unauthenticated.new)],
      );
      addTearDown(container.dispose);
      final session = container.read(relaySessionProvider.notifier);
      const frame = [
        'BUZZ_TASKS_SYNC_REQUIRED',
        '11111111-1111-4111-8111-111111111111',
      ];
      session.debugHandleMessage(frame);
      expect(container.read(tasksSyncSignalProvider), 0);
      session.debugSetSessionStatus(SessionStatus.connected);
      for (final malformed in <List<dynamic>>[
        ['BUZZ_TASKS_SYNC_REQUIRED'],
        ['BUZZ_TASKS_SYNC_REQUIRED', 12],
        ['BUZZ_TASKS_SYNC_REQUIRED', 'not a channel'],
        [...frame, 'unexpected'],
      ]) {
        session.debugHandleMessage(malformed);
      }
      expect(container.read(tasksSyncSignalProvider), 0);
      session.debugHandleMessage(frame);
      expect(container.read(tasksSyncSignalProvider), 1);
      session.debugDispose();
      session.debugHandleMessage(frame);
      expect(container.read(tasksSyncSignalProvider), 1);
    },
  );

  test('reconnect and a dropped fan-out refresh task readers', () async {
    final container = ProviderContainer(
      overrides: [authProvider.overrideWith(_Unauthenticated.new)],
    );
    addTearDown(container.dispose);
    await container.read(authProvider.future);
    final session = container.read(relaySessionProvider.notifier);
    await session.debugHandleConnected();
    expect(container.read(tasksSyncSignalProvider), 1);
    session.debugHandleMessage(['BUZZ_SYNC_REQUIRED', 'backpressure']);
    expect(container.read(tasksSyncSignalProvider), 2);
    session.debugHandleMessage(['BUZZ_SYNC_REQUIRED', 'backpressure']);
    expect(container.read(tasksSyncSignalProvider), 2);
  });
}

class _Unauthenticated extends AuthNotifier {
  @override
  Future<AuthState> build() async =>
      const AuthState(status: AuthStatus.unauthenticated);
}
