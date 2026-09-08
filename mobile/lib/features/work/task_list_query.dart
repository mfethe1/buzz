import 'dart:async';

import 'package:flutter/foundation.dart';

import '../../shared/tasks/task.dart';

/// Read-only page state for one identity and one task filter.
@immutable
class TaskListState {
  /// A loading state starts without pages from a previous filter or refresh.
  const TaskListState({
    this.tasks = const [],
    this.nextCursor,
    this.loading = true,
    this.loadingMore = false,
    this.error,
  });

  final List<Task> tasks;
  final String? nextCursor;
  final bool loading;
  final bool loadingMore;
  final Object? error;
}

/// Serializes page reads and coalesces invalidations into one fresh first page.
/// An identity/filter change creates a new query and disposes the old one.
class TaskListQuery extends ValueNotifier<TaskListState> {
  TaskListQuery(this._load) : super(const TaskListState());

  final Future<TaskPage> Function(String? before) _load;
  final Set<String> _seenCursors = {};
  Future<void>? _active;
  int _generation = 0;
  bool _refreshRequested = false;
  bool _disposed = false;

  /// Invalidates every loaded page; an old in-flight page cannot reappear.
  Future<void> refresh() {
    if (_disposed) return Future.value();
    _generation++;
    _refreshRequested = true;
    _seenCursors.clear();
    value = const TaskListState();
    return _active ??= Future<void>.microtask(() => _run(append: false));
  }

  /// Loads only the next requested page. Failure preserves its retry cursor.
  Future<void> loadMore() {
    if (_disposed || _active != null || value.nextCursor == null) {
      return _active ?? Future.value();
    }
    return _active = Future<void>.microtask(() => _run(append: true));
  }

  Future<void> _run({required bool append}) async {
    while (!_disposed) {
      _refreshRequested = false;
      final generation = _generation;
      final previous = value;
      final before = append ? previous.nextCursor : null;
      if (append) {
        value = TaskListState(
          tasks: previous.tasks,
          nextCursor: before,
          loading: false,
          loadingMore: true,
        );
      }
      try {
        final page = await _load(before);
        if (!_disposed && generation == _generation) {
          final next = page.nextCursor;
          if (next != null && (next == before || _seenCursors.contains(next))) {
            throw const FormatException('task pagination did not advance');
          }
          final tasks = <String, Task>{
            if (append)
              for (final task in previous.tasks) task.id: task,
          };
          for (final task in page.tasks) {
            final existing = tasks[task.id];
            if (existing == null || task.revision >= existing.revision) {
              tasks[task.id] = task;
            }
          }
          if (before != null) _seenCursors.add(before);
          value = TaskListState(
            tasks: List.unmodifiable(tasks.values),
            nextCursor: next,
            loading: false,
          );
        }
      } on Object catch (error) {
        if (!_disposed && generation == _generation) {
          value = TaskListState(
            tasks: append ? previous.tasks : const [],
            nextCursor: append ? before : null,
            loading: false,
            error: error,
          );
        }
      }
      if (!_refreshRequested) break;
      append = false;
    }
    _active = null;
  }

  @override
  void dispose() {
    _disposed = true;
    _generation++;
    super.dispose();
  }
}
