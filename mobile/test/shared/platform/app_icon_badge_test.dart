import 'package:buzz/shared/platform/app_icon_badge.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  test(
    'web avoids the native badge channel while native counts stay intact',
    () async {
      final calls = <MethodCall>[];
      final messenger =
          TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;
      const channel = MethodChannel('app_badge_plus');
      messenger.setMockMethodCallHandler(channel, (call) async {
        calls.add(call);
        return null;
      });
      addTearDown(() => messenger.setMockMethodCallHandler(channel, null));
      for (final count in [0, 1, 7]) {
        await updateAppIconBadge(count);
      }
      if (kIsWeb) {
        expect(calls, isEmpty);
      } else {
        expect(calls.map((call) => call.method), [
          'updateBadge',
          'updateBadge',
          'updateBadge',
        ]);
        expect(calls.map((call) => call.arguments), [
          {'count': 0},
          {'count': 1},
          {'count': 7},
        ]);
      }
    },
  );
}
