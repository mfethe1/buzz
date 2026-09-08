import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_hooks/flutter_hooks.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:lucide_icons_flutter/lucide_icons.dart';

import '../../shared/tasks/task.dart';
import '../../shared/tasks/task_channel.dart';
import '../../shared/tasks/task_assignee_directory.dart';
import '../../shared/tasks/task_assignee_label.dart';
import '../../shared/tasks/task_detail_sheet.dart';
import '../../shared/tasks/tasks_api.dart';
import '../../shared/tasks/tasks_sync.dart';
import '../../shared/tasks/thread_task_chip.dart';
import '../../shared/theme/theme.dart';
import '../../shared/widgets/buzz_loading_indicator.dart';
import 'task_list_query.dart';
import 'work_task_create_sheet.dart';

/// Tasks available to the current identity. The relay remains authoritative
/// for visibility, ordering and state; creating or assigning is not dispatch.
class WorkPage extends HookConsumerWidget {
  const WorkPage({
    super.key,
    required this.onBack,
    required this.channels,
    required this.onRefreshChannels,
    this.visible = true,
  });

  final VoidCallback onBack;
  final AsyncValue<List<TaskChannel>> channels;
  final Future<void> Function() onRefreshChannels;
  final bool visible;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final api = ref.watch(tasksApiProvider);
    final status = useState<TaskStatus?>(null);
    final channelOptions = useValueNotifier(channels);
    useEffect(() {
      var disposed = false;
      scheduleMicrotask(() {
        if (!disposed) channelOptions.value = channels;
      });
      return () => disposed = true;
    }, [channels]);
    final selectedStatus = status.value;
    final query = useMemoized(
      () => TaskListQuery(
        (before) =>
            api.listTaskPage(status: selectedStatus, limit: 20, before: before),
      ),
      [api, selectedStatus],
    );
    useEffect(() => query.dispose, [query]);
    final state = useListenable(query).value;
    final signal = ref.watch(tasksSyncSignalProvider);
    useEffect(() {
      var cancelled = false;
      if (visible) {
        scheduleMicrotask(() {
          if (!cancelled) query.refresh();
        });
      }
      return () => cancelled = true;
    }, [query, signal, visible]);
    useEffect(() {
      final listener = AppLifecycleListener(
        onResume: () {
          if (visible) unawaited(query.refresh());
        },
      );
      return listener.dispose;
    }, [query, visible]);

    return Scaffold(
      backgroundColor: context.colors.surface,
      appBar: AppBar(
        leading: IconButton(
          tooltip: 'Back to Home',
          onPressed: onBack,
          icon: const Icon(LucideIcons.arrowLeft),
        ),
        title: const Text('Work'),
        actions: [
          IconButton(
            tooltip: 'Refresh tasks',
            onPressed: query.refresh,
            icon: const Icon(LucideIcons.refreshCw),
          ),
          IconButton(
            tooltip: 'New task',
            icon: const Icon(LucideIcons.plus),
            onPressed: !api.canSign
                ? null
                : () async {
                    final result = await showWorkTaskCreateSheet(
                      context: context,
                      api: api,
                      directory: ref.read(taskAssigneeDirectoryProvider),
                      channels: channelOptions,
                      refreshChannels: onRefreshChannels,
                    );
                    if (!context.mounted ||
                        !identical(ref.read(tasksApiProvider), api)) {
                      return;
                    }
                    unawaited(query.refresh());
                    if (result != null) {
                      // Creation returns a To do task, which must not disappear
                      // behind an unrelated status filter.
                      status.value = null;
                      ref.read(tasksSyncSignalProvider.notifier).bump();
                      ScaffoldMessenger.of(context).showSnackBar(
                        const SnackBar(content: Text('Task created')),
                      );
                    }
                  },
          ),
        ],
      ),
      body: Column(
        children: [
          Padding(
            key: ValueKey((api, selectedStatus)),
            padding: const EdgeInsets.all(Grid.gutter),
            child: DropdownButtonFormField<TaskStatus?>(
              key: const ValueKey('work-status-filter'),
              initialValue: selectedStatus,
              decoration: const InputDecoration(labelText: 'Status'),
              items: [
                const DropdownMenuItem(value: null, child: Text('All tasks')),
                for (final option in TaskStatus.values)
                  DropdownMenuItem(
                    value: option,
                    child: Text(threadTaskStatusLabel(option)),
                  ),
              ],
              onChanged: (value) => status.value = value,
            ),
          ),
          Expanded(
            child: RefreshIndicator(
              onRefresh: query.refresh,
              child: ListView.builder(
                physics: const AlwaysScrollableScrollPhysics(),
                padding: EdgeInsets.only(
                  bottom: MediaQuery.paddingOf(context).bottom + Grid.xl,
                ),
                itemCount: state.tasks.length + 1,
                itemBuilder: (context, index) {
                  if (index < state.tasks.length) {
                    final task = state.tasks[index];
                    return ListTile(
                      key: ValueKey('work-task-${task.id}'),
                      title: Text(
                        task.title,
                        maxLines: 2,
                        overflow: TextOverflow.ellipsis,
                      ),
                      subtitle: Column(
                        crossAxisAlignment: CrossAxisAlignment.start,
                        children: [
                          Text(threadTaskStatusLabel(task.status)),
                          if (task.assignee != null)
                            TaskAssigneeLabel(pubkey: task.assignee!),
                        ],
                      ),
                      trailing: const Icon(LucideIcons.chevronRight, size: 18),
                      onTap: () async {
                        await showTaskDetailSheet(
                          context: context,
                          ref: ref,
                          taskId: task.id,
                        );
                        if (context.mounted) unawaited(query.refresh());
                      },
                    );
                  }
                  if (state.loading || state.loadingMore) {
                    return const Padding(
                      padding: EdgeInsets.all(Grid.xl),
                      child: Center(child: BuzzLoadingIndicator(size: 28)),
                    );
                  }
                  if (state.error != null) {
                    return Padding(
                      padding: const EdgeInsets.all(Grid.gutter),
                      child: Column(
                        children: [
                          Text(
                            state.tasks.isEmpty
                                ? "Couldn't load your tasks."
                                : "Couldn't load more tasks.",
                          ),
                          TextButton(
                            onPressed: state.tasks.isEmpty
                                ? query.refresh
                                : query.loadMore,
                            child: const Text('Try again'),
                          ),
                        ],
                      ),
                    );
                  }
                  if (state.nextCursor != null) {
                    return Center(
                      child: TextButton(
                        onPressed: query.loadMore,
                        child: const Text('Load more'),
                      ),
                    );
                  }
                  if (state.tasks.isEmpty) {
                    return Padding(
                      padding: const EdgeInsets.all(Grid.xl),
                      child: Text(
                        selectedStatus == null
                            ? 'No tasks yet.'
                            : 'No tasks with this status.',
                        textAlign: TextAlign.center,
                      ),
                    );
                  }
                  return const SizedBox.shrink();
                },
              ),
            ),
          ),
        ],
      ),
    );
  }
}
