import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_hooks/flutter_hooks.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../../shared/machines/computer.dart';
import '../../shared/machines/machines_api.dart';
import '../../shared/theme/theme.dart';

final computerClockProvider = Provider<DateTime Function()>(
  (ref) =>
      () => DateTime.now().toUtc(),
);

/// Repaint at the nearest observation expiry; never keep a cached ready label.
DateTime useComputerObservationClock(
  WidgetRef ref,
  List<EnrolledComputer> computers,
  bool visible,
) {
  final clock = ref.watch(computerClockProvider);
  final tick = useState(0);
  final now = clock();
  final deadlines = [
    for (final c in computers)
      if (c.freshAt(now)) c.expiresAt!,
  ];
  deadlines.sort();
  final next = deadlines.firstOrNull;
  useEffect(() {
    if (!visible || next == null) return null;
    final timer = Timer(
      next.difference(now) + const Duration(milliseconds: 1),
      () => tick.value++,
    );
    return timer.cancel;
  }, [next, visible, tick.value, clock]);
  return now;
}

class ComputerReadError extends StatelessWidget {
  const ComputerReadError({
    super.key,
    required this.error,
    required this.onRetry,
    this.detail = false,
  });
  final Object error;
  final VoidCallback onRetry;
  final bool detail;

  @override
  Widget build(BuildContext context) {
    final code = error is ComputerApiException
        ? (error as ComputerApiException).statusCode
        : null;
    final message = switch (code) {
      401 || 403 =>
        'Your access to this community changed. Return to Home to check your account.',
      404 when detail =>
        'This computer is no longer available to your account.',
      _ =>
        "Couldn't reach your computers. Check your connection and try again.",
    };
    return Padding(
      padding: const EdgeInsets.all(Grid.xl),
      child: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          Text(message, textAlign: TextAlign.center),
          const SizedBox(height: Grid.sm),
          TextButton(onPressed: onRetry, child: const Text('Try again')),
        ],
      ),
    );
  }
}
