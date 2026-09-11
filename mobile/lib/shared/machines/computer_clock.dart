import 'package:hooks_riverpod/hooks_riverpod.dart';

final _clock = Stopwatch()..start();
Duration computerElapsed() => _clock.elapsed;

/// API request starts and UI expiry timers share this monotonic time domain.
final computerClockProvider = Provider<Duration Function()>(
  (ref) => computerElapsed,
);
