/// Community-wide task list state for the Tasks tab.
///
/// The relay's `GET /api/tasks` is already host-derived and tenant-scoped (see
/// [TasksApi.listTasks]), so this provider holds only the client-side filter
/// and the fetched page. Tasks arrive newest-modified-first and that order is
/// preserved — the tab is a recency feed, not a re-sorted board.
library;

import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../../shared/tasks/task.dart';
import '../../shared/tasks/tasks_api.dart';

/// How many tasks one page of the Tasks tab asks for.
///
/// The relay caps its own page size; this keeps the first paint small on a
/// phone rather than pulling a community's entire backlog.
const int kTasksPageLimit = 100;

/// The status filter shown in the Tasks tab's segmented control.
///
/// [TaskStatusFilter.open] is a client-side grouping rather than a relay
/// status: the relay filters one status at a time, so "Open" fetches
/// everything and drops the terminal states here.
enum TaskStatusFilter {
  /// Everything that is not done or cancelled.
  open('Open'),

  /// Only [TaskStatus.inProgress].
  inProgress('In progress'),

  /// Only [TaskStatus.blocked].
  blocked('Blocked'),

  /// Only [TaskStatus.done].
  done('Done'),

  /// No filtering at all, including cancelled tasks.
  all('All');

  const TaskStatusFilter(this.label);

  /// Human-readable label for the filter chip.
  final String label;

  /// The single relay-side status this filter maps to, if any.
  ///
  /// Returns `null` for [open] and [all], which are resolved in [matches]
  /// after fetching because neither is expressible as one `status=` value.
  TaskStatus? get wireStatus => switch (this) {
    TaskStatusFilter.inProgress => TaskStatus.inProgress,
    TaskStatusFilter.blocked => TaskStatus.blocked,
    TaskStatusFilter.done => TaskStatus.done,
    TaskStatusFilter.open || TaskStatusFilter.all => null,
  };

  /// Whether [task] belongs in this filter's list.
  bool matches(Task task) => switch (this) {
    TaskStatusFilter.all => true,
    TaskStatusFilter.open =>
      task.status != TaskStatus.done && task.status != TaskStatus.cancelled,
    TaskStatusFilter.inProgress => task.status == TaskStatus.inProgress,
    TaskStatusFilter.blocked => task.status == TaskStatus.blocked,
    TaskStatusFilter.done => task.status == TaskStatus.done,
  };
}

/// The filter currently applied to the Tasks tab.
///
/// Riverpod 3 retired `StateProvider`, so this is a minimal [Notifier] holding
/// the same single value.
class TaskStatusFilterNotifier extends Notifier<TaskStatusFilter> {
  @override
  TaskStatusFilter build() => TaskStatusFilter.open;

  /// Selects [filter], reloading [communityTasksProvider] when it changes.
  void select(TaskStatusFilter filter) => state = filter;
}

final taskStatusFilterProvider =
    NotifierProvider<TaskStatusFilterNotifier, TaskStatusFilter>(
      TaskStatusFilterNotifier.new,
    );

/// This community's tasks for the selected [taskStatusFilterProvider].
///
/// Watching the filter rather than passing it as a family argument keeps one
/// cached list per filter change and lets `ref.refresh` drive pull-to-refresh
/// without the page tracking a request id.
final communityTasksProvider = FutureProvider.autoDispose<List<Task>>((
  ref,
) async {
  final filter = ref.watch(taskStatusFilterProvider);
  final api = ref.watch(tasksApiProvider);
  if (!api.canSign) {
    // Without a signing key every call would throw from NIP-98 signing; an
    // empty list lets the page show its "sign in" empty state instead of an
    // error surface the user cannot act on.
    return const <Task>[];
  }
  final tasks = await api.listTasks(
    status: filter.wireStatus,
    limit: kTasksPageLimit,
  );
  return [
    for (final task in tasks)
      if (filter.matches(task)) task,
  ];
});
