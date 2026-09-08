import 'package:hooks_riverpod/hooks_riverpod.dart';

/// Invalidates visible task reads after relay changes or a connection gap.
/// The counter carries no task data or authority; readers re-fetch via REST.
class TasksSyncSignalNotifier extends Notifier<int> {
  @override
  int build() => 0;

  void bump() => state++;
}

final tasksSyncSignalProvider = NotifierProvider<TasksSyncSignalNotifier, int>(
  TasksSyncSignalNotifier.new,
);
