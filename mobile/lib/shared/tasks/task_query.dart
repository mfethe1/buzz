import 'dart:async';

import 'package:flutter/widgets.dart';
import 'package:flutter_hooks/flutter_hooks.dart';

/// Keeps one request in flight, with at most one follow-up for invalidations
/// received during it. A new scope disposes the previous result immediately.
TaskQuery<T> useTaskQuery<T>(
  Future<T> Function() load, {
  required List<Object?> scope,
  required Object signal,
}) {
  final query = useMemoized(() => TaskQuery(load), scope);
  useEffect(() => query.dispose, [query]);
  useListenable(query);
  useEffect(() {
    var cancelled = false;
    scheduleMicrotask(() {
      if (!cancelled) query.refresh();
    });
    return () => cancelled = true;
  }, [query, signal]);
  return query;
}

class TaskQuery<T> extends ValueNotifier<AsyncSnapshot<T>> {
  TaskQuery(this._load) : super(const AsyncSnapshot.waiting());

  final Future<T> Function() _load;
  bool _disposed = false;
  bool _running = false;
  bool _requested = false;

  void refresh() {
    if (_disposed) return;
    _requested = true;
    if (!_running) unawaited(_drain());
  }

  Future<void> _drain() async {
    _running = true;
    while (_requested && !_disposed) {
      _requested = false;
      value = value.inState(ConnectionState.waiting);
      try {
        final result = await _load();
        if (!_disposed) {
          value = AsyncSnapshot.withData(ConnectionState.done, result);
        }
      } on Object catch (error, stack) {
        if (!_disposed) {
          value = AsyncSnapshot.withError(ConnectionState.done, error, stack);
        }
      }
    }
    _running = false;
  }

  @override
  void dispose() {
    _disposed = true;
    super.dispose();
  }
}
