import 'dart:async';

import 'package:flutter/foundation.dart';

import '../../shared/tasks/task.dart';
import '../../shared/tasks/tasks_api.dart';

/// Read-only page state for one identity and one task filter.
@immutable
class TaskListState {
  const TaskListState({
    this.tasks = const [],
    this.nextCursor,
    this.loading = true,
    this.loadingMore = false,
    this.error,
    this.reconciliationFailed = false,
  });

  final List<Task> tasks;
  final String? nextCursor;
  final bool loading;
  final bool loadingMore;
  final Object? error;
  final bool reconciliationFailed;
}

enum _Read { reset, reconcile, append }

/// Serializes reads within one identity/filter and fences superseded results.
class TaskListQuery extends ValueNotifier<TaskListState> {
  TaskListQuery(this._load) : super(const TaskListState());

  final Future<TaskPage> Function(String? before) _load;
  final Set<String> _seenCursors = {};
  Future<void>? _active;
  _Read? _pending;
  _Read? _running;
  int _generation = 0;
  int _loadedPageCount = 0;
  bool _disposed = false;
  bool _enabled = true;
  bool _fresh = false;
  Timer? _freshness;

  /// Hide/paused work does not start more pages or publish its old response.
  void setEnabled(bool enabled) {
    if (_disposed || _enabled == enabled) return;
    _enabled = enabled;
    if (!enabled) {
      _generation++;
      _pending = null;
    }
  }

  /// Explicit reset discards the previous page window immediately.
  Future<void> refresh() {
    if (_disposed || !_enabled) return Future.value();
    _generation++;
    _pending = _Read.reset;
    _loadedPageCount = 0;
    _seenCursors.clear();
    value = const TaskListState();
    return _start();
  }

  /// Revalidate the loaded window without clearing rows or extending cursors.
  /// Timer ticks join an existing reconciliation; an advisory fences it and
  /// requests one replacement read. Pagination is completed before a timer read.
  Future<void> reconcile({bool invalidate = false}) {
    if (_disposed || !_enabled) return Future.value();
    if (!invalidate && _active != null && _running == _Read.reconcile) {
      return _active!;
    }
    if (invalidate) _generation++;
    if (_pending != _Read.reset) _pending = _Read.reconcile;
    return _start();
  }

  Future<void> loadMore() {
    if (_disposed ||
        !_enabled ||
        _active != null ||
        value.nextCursor == null ||
        value.reconciliationFailed) {
      return _active ?? Future.value();
    }
    _pending = _Read.append;
    return _start();
  }

  Future<void> _start() => _active ??= Future<void>.microtask(_drain);

  Future<void> _drain() async {
    try {
      while (!_disposed && _enabled && _pending != null) {
        final mode = _pending!;
        _pending = null;
        _running = mode;
        final generation = _generation;
        final previous = value;
        if (mode == _Read.append) {
          value = TaskListState(
            tasks: previous.tasks,
            nextCursor: previous.nextCursor,
            loading: false,
            loadingMore: true,
          );
        }
        try {
          await _readWindow(mode, generation, previous);
        } on Object catch (error) {
          if (_disposed || !_enabled || generation != _generation) continue;
          final denied =
              error is TaskApiException &&
              (error.statusCode == 401 ||
                  error.statusCode == 403 ||
                  error.statusCode == 404);
          final preserve = mode != _Read.reset && _fresh && !denied;
          if (!preserve) {
            _loadedPageCount = 0;
            _seenCursors.clear();
          }
          value = TaskListState(
            tasks: preserve ? previous.tasks : const [],
            nextCursor: preserve ? previous.nextCursor : null,
            loading: false,
            error: error,
            reconciliationFailed: mode != _Read.append,
          );
        }
      }
    } finally {
      _running = null;
      _active = null;
    }
  }

  Future<void> _readWindow(
    _Read mode,
    int generation,
    TaskListState previous,
  ) async {
    final append = mode == _Read.append;
    final target = mode == _Read.reconcile && _loadedPageCount > 0
        ? _loadedPageCount
        : 1;
    var cursor = append ? previous.nextCursor : null;
    final seen = <String>{if (append) ..._seenCursors};
    final oldById = {for (final task in previous.tasks) task.id: task};
    final rows = <String, Task>{if (append) ...oldById};
    var pages = 0;
    var expired = false;
    // Each page is bounded by 15 seconds; do not start another after the
    // 30-second window budget. An already-started page may finish first.
    final budget = Timer(const Duration(seconds: 30), () => expired = true);
    try {
      for (var index = 0; index < target; index++) {
        if (_disposed || !_enabled || generation != _generation) return;
        if (expired) throw TimeoutException('Task reconciliation timed out');
        final page = await _load(cursor).timeout(const Duration(seconds: 15));
        if (_disposed || !_enabled || generation != _generation) return;
        if (expired) throw TimeoutException('Task reconciliation timed out');
        final next = page.nextCursor;
        if (next != null && (next == cursor || seen.contains(next))) {
          throw const FormatException('task pagination did not advance');
        }
        if (cursor != null) seen.add(cursor);
        for (final task in page.tasks) {
          final old = oldById[task.id];
          final candidate = old != null && old.revision >= task.revision
              ? old
              : task;
          final current = rows[task.id];
          if (current == null || candidate.revision >= current.revision) {
            rows[task.id] = candidate;
          }
        }
        pages++;
        cursor = next;
        if (cursor == null) break;
      }
    } finally {
      budget.cancel();
    }
    _loadedPageCount = append ? _loadedPageCount + pages : pages;
    _seenCursors
      ..clear()
      ..addAll(seen);
    if (!append) _recordFreshWindow();
    final tasks = List<Task>.unmodifiable(rows.values);
    value = TaskListState(
      tasks: listEquals(previous.tasks, tasks) ? previous.tasks : tasks,
      nextCursor: cursor,
      loading: false,
    );
  }

  void _recordFreshWindow() {
    _fresh = true;
    _freshness?.cancel();
    _freshness = Timer(const Duration(seconds: 90), () {
      _fresh = false;
      if (_disposed || value.error == null) return;
      _loadedPageCount = 0;
      _seenCursors.clear();
      value = TaskListState(
        loading: false,
        error: value.error,
        reconciliationFailed: true,
      );
    });
  }

  @override
  void dispose() {
    _disposed = true;
    _generation++;
    _pending = null;
    _freshness?.cancel();
    super.dispose();
  }
}
