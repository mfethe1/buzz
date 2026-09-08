import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:flutter_hooks/flutter_hooks.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../../shared/tasks/task.dart';
import '../../shared/tasks/task_assignee_directory.dart';
import '../../shared/tasks/task_channel.dart';
import '../../shared/tasks/task_query.dart';
import '../../shared/tasks/tasks_api.dart';
import '../../shared/theme/theme.dart';
import '../../shared/widgets/buzz_loading_indicator.dart';
import '../../shared/widgets/modal_presentation.dart';

Future<Task?> showWorkTaskCreateSheet({
  required BuildContext context,
  required TasksApi api,
  required TaskAssigneeDirectory directory,
  required ValueListenable<AsyncValue<List<TaskChannel>>> channels,
  required Future<void> Function() refreshChannels,
}) => showBuzzModalBottomSheet<Task>(
  context: context,
  title: 'New task',
  showDragHandle: true,
  isScrollControlled: true,
  constraints: BoxConstraints(
    maxWidth: 640,
    maxHeight: MediaQuery.sizeOf(context).height * 0.9,
  ),
  builder: (_) => _WorkTaskCreateSheet(
    api: api,
    directory: directory,
    channels: channels,
    refreshChannels: refreshChannels,
  ),
);

class _WorkTaskCreateSheet extends HookConsumerWidget {
  const _WorkTaskCreateSheet({
    required this.api,
    required this.directory,
    required this.channels,
    required this.refreshChannels,
  });

  final TasksApi api;
  final TaskAssigneeDirectory directory;
  final ValueListenable<AsyncValue<List<TaskChannel>>> channels;
  final Future<void> Function() refreshChannels;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final title = useTextEditingController();
    final body = useTextEditingController();
    final channelId = useState<String?>(null);
    final assignee = useState<String?>(null);
    final busy = useState(false);
    final error = useState<String?>(null);
    final uncertain = useState(false);
    final options = useValueListenable(channels);
    final scopeChanged =
        !identical(ref.watch(tasksApiProvider), api) ||
        !identical(ref.watch(taskAssigneeDirectoryProvider), directory);
    final selectedChannel = channelId.value;
    final agents = useTaskQuery(
      () => directory.load(selectedChannel),
      scope: [directory, selectedChannel],
      signal: 0,
    );
    final channelChoices = options.asData?.value ?? const <TaskChannel>[];
    final channelExists =
        selectedChannel == null ||
        channelChoices.any((c) => c.id == selectedChannel);
    final selectedEligible =
        assignee.value == null ||
        (agents.value.data ?? const []).any((a) => a.pubkey == assignee.value);
    final enabled = !scopeChanged && !busy.value && !uncertain.value;

    bool stillCurrent() =>
        context.mounted &&
        identical(ref.read(tasksApiProvider), api) &&
        identical(ref.read(taskAssigneeDirectoryProvider), directory);

    Future<void> create() async {
      if (!enabled) return;
      final taskTitle = title.text.trim();
      if (taskTitle.isEmpty || taskTitleLength(taskTitle) > maxTaskTitleChars) {
        error.value = 'Enter a title with 1–200 characters.';
        return;
      }
      busy.value = true;
      error.value = null;
      var submitted = false;
      try {
        // Refreshing never retries a write. Re-check the chosen conversation
        // and bot role while still bound to the form's original identity.
        await refreshChannels();
        if (!stillCurrent()) return;
        if (selectedChannel != null &&
            !(channels.value.asData?.value ?? const <TaskChannel>[]).any(
              (c) => c.id == selectedChannel,
            )) {
          throw StateError('conversation no longer available');
        }
        final currentAgents = await directory.load(selectedChannel);
        if (!stillCurrent()) return;
        if (assignee.value != null &&
            !currentAgents.any((a) => a.pubkey == assignee.value)) {
          assignee.value = null;
          agents.refresh();
          error.value =
              'This agent is no longer available. Choose an assignee again.';
          return;
        }
        submitted = true;
        final task = await api.createTask(
          title: taskTitle,
          body: body.text,
          channelId: selectedChannel,
          assignee: assignee.value,
        );
        if (context.mounted && stillCurrent()) Navigator.of(context).pop(task);
      } on Object catch (failure) {
        if (!stillCurrent()) return;
        final rejected =
            failure is TaskApiException &&
            failure.statusCode >= 400 &&
            failure.statusCode < 500;
        if (submitted && !rejected) {
          uncertain.value = true;
          error.value =
              "Couldn't confirm this task was created. Check Work before trying again.";
        } else {
          error.value = submitted
              ? "Couldn't create this task. Check its details and try again."
              : "Couldn't check this conversation and its agents. Refresh and try again.";
          agents.refresh();
        }
      } finally {
        if (context.mounted) busy.value = false;
      }
    }

    return SingleChildScrollView(
      child: Padding(
        padding: EdgeInsets.fromLTRB(
          Grid.gutter,
          0,
          Grid.gutter,
          MediaQuery.viewInsetsOf(context).bottom + Grid.gutter,
        ),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            if (scopeChanged)
              const Text(
                'Your workspace changed. Close this task and try again.',
              )
            else ...[
              TextField(
                key: const ValueKey('work-task-title'),
                controller: title,
                enabled: enabled,
                decoration: const InputDecoration(labelText: 'Title'),
                textCapitalization: TextCapitalization.sentences,
              ),
              const SizedBox(height: Grid.xs),
              TextField(
                key: const ValueKey('work-task-body'),
                controller: body,
                enabled: enabled,
                minLines: 2,
                maxLines: 5,
                decoration: const InputDecoration(
                  labelText: 'Description (optional)',
                ),
              ),
              const SizedBox(height: Grid.xs),
              DropdownButtonFormField<String>(
                key: ValueKey((
                  'work-channel-value',
                  selectedChannel,
                  channelExists,
                )),
                initialValue: channelExists ? selectedChannel ?? '' : '',
                decoration: const InputDecoration(labelText: 'Conversation'),
                items: [
                  const DropdownMenuItem(value: '', child: Text('Workspace')),
                  for (final channel in channelChoices)
                    DropdownMenuItem(
                      value: channel.id,
                      child: Text(
                        channel.name,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                      ),
                    ),
                ],
                onChanged: enabled
                    ? (value) {
                        channelId.value = value == '' ? null : value;
                        assignee.value = null;
                        error.value = null;
                      }
                    : null,
                // Stable descendant key lets tests and accessibility locate the field
                // independently of the value key used to reset its form state.
                onTap: null,
              ),
              if (options.hasError || !channelExists) ...[
                const Text("Couldn't load your conversations."),
                TextButton(
                  onPressed: enabled ? refreshChannels : null,
                  child: const Text('Refresh conversations'),
                ),
              ],
              const SizedBox(height: Grid.xs),
              if (selectedChannel != null &&
                  agents.value.connectionState != ConnectionState.done)
                const Center(child: BuzzLoadingIndicator(size: 24))
              else if (agents.value.hasError) ...[
                const Text("Couldn't load available agents."),
                TextButton(
                  onPressed: enabled ? agents.refresh : null,
                  child: const Text('Refresh agents'),
                ),
              ] else
                DropdownButtonFormField<String>(
                  key: ValueKey((
                    'work-assignee-value',
                    selectedChannel,
                    assignee.value,
                    selectedEligible,
                  )),
                  initialValue: selectedEligible ? assignee.value ?? '' : '',
                  isExpanded: true,
                  decoration: const InputDecoration(labelText: 'Assignee'),
                  items: [
                    const DropdownMenuItem(
                      value: '',
                      child: Text('Unassigned'),
                    ),
                    for (final agent in agents.value.data ?? const [])
                      DropdownMenuItem(
                        value: agent.pubkey,
                        child: Text(
                          '${agent.displayName?.trim().isNotEmpty == true ? agent.displayName!.trim() : 'Agent'} · ${agent.pubkey.substring(0, 8)}',
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                        ),
                      ),
                  ],
                  onChanged: enabled && selectedChannel != null
                      ? (value) => assignee.value = value == '' ? null : value
                      : null,
                ),
              if (error.value != null)
                Padding(
                  padding: const EdgeInsets.only(top: Grid.xs),
                  child: Text(error.value!),
                ),
              const SizedBox(height: Grid.sm),
              if (uncertain.value)
                FilledButton(
                  onPressed: () => Navigator.of(context).pop(),
                  child: const Text('Check Work'),
                )
              else
                FilledButton(
                  onPressed: enabled && channelExists && !agents.value.hasError
                      ? create
                      : null,
                  child: busy.value
                      ? const SizedBox(
                          height: 20,
                          width: 20,
                          child: BuzzLoadingIndicator(size: 20),
                        )
                      : const Text('Create task'),
                ),
            ],
          ],
        ),
      ),
    );
  }
}
