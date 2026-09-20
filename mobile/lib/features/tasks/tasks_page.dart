/// Community-wide task list — the Tasks tab.
///
/// Before this page, tasks were reachable only from inside the thread that
/// opened them (`features/channels/thread_detail_page/thread_task_chip.dart`),
/// even though the relay has always exposed a community-scoped
/// `GET /api/tasks`. This surfaces that list on its own destination so a task
/// can be found without remembering its thread.
///
/// Rows reuse [showTaskDetailSheet] rather than pushing a new route: the
/// detail sheet already renders a task's full event history and status
/// control, and a second implementation would drift from it.
library;

import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:flutter_hooks/flutter_hooks.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:lucide_icons_flutter/lucide_icons.dart';

import '../../shared/tasks/task.dart';
import '../../shared/tasks/tasks_api.dart';
import '../../shared/theme/theme.dart';
import '../../shared/widgets/bee_refresh_indicator.dart';
import '../../shared/widgets/buzz_loading_indicator.dart';
import '../../shared/widgets/frosted_app_bar.dart';
import '../../shared/widgets/frosted_scaffold.dart';
import '../channels/thread_detail_page/task_detail_sheet.dart';
import 'tasks_provider.dart';

/// Lists this community's tasks with a status filter and pull-to-refresh.
class TasksPage extends HookConsumerWidget {
  /// Creates the Tasks tab body.
  const TasksPage({this.tabReselection, super.key});

  /// Notifies this page when its already-selected tab is tapped again, so a
  /// second tap scrolls back to the top — the same affordance Activity has.
  final ValueListenable<int>? tabReselection;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final tasksAsync = ref.watch(communityTasksProvider);
    final filter = ref.watch(taskStatusFilterProvider);
    final scrollController = useScrollController();
    final reducedMotion = MediaQuery.disableAnimationsOf(context);

    useEffect(() {
      final reselection = tabReselection;
      if (reselection == null) return null;
      void scrollToTop() {
        if (!scrollController.hasClients) return;
        final position = scrollController.position;
        if (position.pixels <= position.minScrollExtent + 0.5) return;
        if (reducedMotion) {
          scrollController.jumpTo(position.minScrollExtent);
          return;
        }
        scrollController.animateTo(
          position.minScrollExtent,
          duration: const Duration(milliseconds: 240),
          curve: Curves.easeOutCubic,
        );
      }

      reselection.addListener(scrollToTop);
      return () => reselection.removeListener(scrollToTop);
    }, [tabReselection, scrollController, reducedMotion]);

    return FrostedScaffold(
      appBar: FrostedAppBar(
        title: const Text('Tasks'),
        bottomHeight: 48,
        bottom: _TaskFilterBar(
          selected: filter,
          onSelected: (next) =>
              ref.read(taskStatusFilterProvider.notifier).select(next),
        ),
      ),
      body: BeeRefreshIndicator(
        onRefresh: () async {
          // `future` is awaited so the indicator stays up for the whole
          // round-trip instead of vanishing the moment the refresh is queued.
          // ignore: unused_result — awaited for its timing, not its value.
          await ref.refresh(communityTasksProvider.future);
        },
        child: switch (tasksAsync) {
          AsyncValue(hasError: true, :final error) => _TasksError(
            message: error is TaskApiException
                ? error.message
                : 'Could not load tasks',
            onRetry: () => ref.invalidate(communityTasksProvider),
          ),
          AsyncValue(:final value?) =>
            value.isEmpty
                ? _TasksEmpty(filter: filter)
                : _TaskList(tasks: value, scrollController: scrollController),
          _ => const Center(child: BuzzLoadingIndicator()),
        },
      ),
    );
  }
}

class _TaskFilterBar extends StatelessWidget {
  const _TaskFilterBar({required this.selected, required this.onSelected});

  final TaskStatusFilter selected;
  final ValueChanged<TaskStatusFilter> onSelected;

  @override
  Widget build(BuildContext context) {
    return SizedBox(
      height: 48,
      child: ListView.separated(
        scrollDirection: Axis.horizontal,
        padding: const EdgeInsets.symmetric(
          horizontal: Grid.gutter,
          vertical: Grid.xxs,
        ),
        itemCount: TaskStatusFilter.values.length,
        separatorBuilder: (_, _) => const SizedBox(width: Grid.xxs),
        itemBuilder: (context, index) {
          final filter = TaskStatusFilter.values[index];
          return ChoiceChip(
            label: Text(filter.label),
            selected: filter == selected,
            onSelected: (isSelected) {
              if (isSelected) onSelected(filter);
            },
          );
        },
      ),
    );
  }
}

class _TaskList extends ConsumerWidget {
  const _TaskList({required this.tasks, required this.scrollController});

  final List<Task> tasks;
  final ScrollController scrollController;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    return ListView.separated(
      controller: scrollController,
      padding: EdgeInsets.fromLTRB(
        Grid.gutter,
        Grid.xxs,
        Grid.gutter,
        MediaQuery.paddingOf(context).bottom + Grid.gutter,
      ),
      itemCount: tasks.length,
      separatorBuilder: (_, _) => const SizedBox(height: Grid.xxs),
      itemBuilder: (context, index) => _TaskRow(task: tasks[index]),
    );
  }
}

class _TaskRow extends ConsumerWidget {
  const _TaskRow({required this.task});

  final Task task;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final (icon, statusColor) = _statusVisual(context, task.status);
    return InkWell(
      borderRadius: BorderRadius.circular(Grid.twelve),
      onTap: () =>
          showTaskDetailSheet(context: context, ref: ref, taskId: task.id),
      child: Padding(
        padding: const EdgeInsets.symmetric(
          vertical: Grid.twelve,
          horizontal: Grid.xxs,
        ),
        child: Row(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Icon(icon, size: 18, color: statusColor),
            const SizedBox(width: Grid.twelve),
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(
                    task.title,
                    maxLines: 2,
                    overflow: TextOverflow.ellipsis,
                    style: context.textTheme.bodyMedium?.copyWith(
                      // A finished task is struck through so a mixed "All"
                      // list reads at a glance without another status chip.
                      decoration: task.status == TaskStatus.done
                          ? TextDecoration.lineThrough
                          : null,
                      color: task.status == TaskStatus.done
                          ? context.colors.onSurfaceVariant
                          : null,
                    ),
                  ),
                  const SizedBox(height: Grid.half),
                  Text(
                    _subtitle(task),
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: context.textTheme.bodySmall?.copyWith(
                      color: context.colors.onSurfaceVariant,
                    ),
                  ),
                ],
              ),
            ),
          ],
        ),
      ),
    );
  }
}

/// Builds the one-line context under a task title.
///
/// Assignee and due date are the two fields that decide whether a task is
/// someone's problem today, so they lead; the relative update time is the
/// fallback when a task has neither.
String _subtitle(Task task) {
  final parts = <String>[
    if (task.assignee case final assignee? when assignee.isNotEmpty)
      '@$assignee',
    if (task.dueAt case final dueAt?) 'due ${_formatDate(dueAt)}',
  ];
  if (parts.isEmpty) return 'updated ${_formatRelative(task.updatedAt)}';
  return parts.join(' · ');
}

String _formatDate(DateTime value) {
  final local = value.toLocal();
  return '${local.month}/${local.day}';
}

String _formatRelative(DateTime value) {
  final delta = DateTime.now().difference(value.toLocal());
  if (delta.inMinutes < 1) return 'just now';
  if (delta.inHours < 1) return '${delta.inMinutes}m ago';
  if (delta.inDays < 1) return '${delta.inHours}h ago';
  if (delta.inDays < 7) return '${delta.inDays}d ago';
  return _formatDate(value);
}

(IconData, Color) _statusVisual(
  BuildContext context,
  TaskStatus status,
) => switch (status) {
  TaskStatus.todo => (LucideIcons.circle, context.colors.onSurfaceVariant),
  TaskStatus.inProgress => (LucideIcons.circleDashed, context.colors.primary),
  TaskStatus.blocked => (LucideIcons.circleAlert, context.colors.error),
  TaskStatus.done => (LucideIcons.circleCheck, context.colors.onSurfaceVariant),
  TaskStatus.cancelled => (
    LucideIcons.circleSlash,
    context.colors.onSurfaceVariant,
  ),
};

class _TasksEmpty extends StatelessWidget {
  const _TasksEmpty({required this.filter});

  final TaskStatusFilter filter;

  @override
  Widget build(BuildContext context) {
    // Rendered inside a scrollable so pull-to-refresh still works on an empty
    // list — a bare Center would not accept the drag gesture.
    return ListView(
      padding: const EdgeInsets.symmetric(vertical: Grid.xl),
      children: [
        Column(
          children: [
            Icon(
              LucideIcons.listChecks,
              size: Grid.lg,
              color: context.colors.onSurfaceVariant,
            ),
            const SizedBox(height: Grid.xxs),
            Text(
              filter == TaskStatusFilter.open
                  ? 'No open tasks'
                  : 'No ${filter.label.toLowerCase()} tasks',
              style: context.textTheme.bodyMedium?.copyWith(
                color: context.colors.onSurfaceVariant,
              ),
            ),
            const SizedBox(height: Grid.half),
            Padding(
              padding: const EdgeInsets.symmetric(horizontal: Grid.gutter),
              child: Text(
                'Open a task from a thread and it shows up here.',
                textAlign: TextAlign.center,
                style: context.textTheme.bodySmall?.copyWith(
                  color: context.colors.onSurfaceVariant,
                ),
              ),
            ),
          ],
        ),
      ],
    );
  }
}

class _TasksError extends StatelessWidget {
  const _TasksError({required this.message, required this.onRetry});

  final String message;
  final VoidCallback onRetry;

  @override
  Widget build(BuildContext context) {
    return ListView(
      padding: const EdgeInsets.symmetric(vertical: Grid.xl),
      children: [
        Column(
          children: [
            Icon(
              LucideIcons.circleAlert,
              size: Grid.lg,
              color: context.colors.error,
            ),
            const SizedBox(height: Grid.xxs),
            Padding(
              padding: const EdgeInsets.symmetric(horizontal: Grid.gutter),
              child: Text(
                message,
                textAlign: TextAlign.center,
                style: context.textTheme.bodyMedium?.copyWith(
                  color: context.colors.onSurfaceVariant,
                ),
              ),
            ),
            const SizedBox(height: Grid.xxs),
            TextButton(onPressed: onRetry, child: const Text('Try again')),
          ],
        ),
      ],
    );
  }
}
