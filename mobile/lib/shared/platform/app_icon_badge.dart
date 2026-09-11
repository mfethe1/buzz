import 'package:app_badge_plus/app_badge_plus.dart';
import 'package:flutter/foundation.dart';

/// Updates the installed application's unread badge. The badge plugin has no
/// web implementation; browser clients keep their in-app unread indicators.
Future<void> updateAppIconBadge(int count) async {
  if (kIsWeb) return;
  await AppBadgePlus.updateBadge(count);
}
