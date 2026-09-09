import 'dart:async';

import 'package:flutter/foundation.dart';

import '../../shared/machines/computer.dart';
import '../../shared/machines/machines_api.dart';

/// Read-only page state for one identity and one computer owner.
@immutable
class ComputerListState {
  /// A loading state starts without pages from a previous community or refresh.
  const ComputerListState({
    this.computers = const [],
    this.nextCursor,
    this.loading = true,
    this.loadingMore = false,
    this.error,
  });

  final List<EnrolledComputer> computers;
  final String? nextCursor;
  final bool loading;
  final bool loadingMore;
  final Object? error;
}

/// Serializes page reads and coalesces invalidations into one fresh first page.
/// An identity/community change creates a new query and disposes the old one.
class ComputerListQuery extends ValueNotifier<ComputerListState> {
  ComputerListQuery(this._load) : super(const ComputerListState());

  final Future<ComputerPage> Function(String? after) _load;
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
    value = const ComputerListState();
    return _active ??= Future<void>.microtask(() => _run(append: false));
  }

  /// A denied detail read revokes this cached list too, including pending pages.
  void invalidateAccess(ComputerApiException error) {
    if (_disposed) return;
    _generation++;
    _refreshRequested = false;
    _seenCursors.clear();
    value = ComputerListState(loading: false, error: error);
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
      final after = append ? previous.nextCursor : null;
      if (append) {
        value = ComputerListState(
          computers: previous.computers,
          nextCursor: after,
          loading: false,
          loadingMore: true,
        );
      }
      try {
        final page = await _load(after);
        if (!_disposed && generation == _generation) {
          final next = page.nextCursor;
          if (next != null && (next == after || _seenCursors.contains(next))) {
            throw const FormatException('computer pagination did not advance');
          }
          final computers = <String, EnrolledComputer>{
            if (append)
              for (final computer in previous.computers) computer.id: computer,
          };
          for (final computer in page.computers) {
            final existing = computers[computer.id];
            if (existing == null || computer.sequence >= existing.sequence) {
              computers[computer.id] = computer;
            }
          }
          if (after != null) _seenCursors.add(after);
          value = ComputerListState(
            computers: List.unmodifiable(computers.values),
            nextCursor: next,
            loading: false,
          );
        }
      } on Object catch (error) {
        if (!_disposed && generation == _generation) {
          final accessChanged =
              error is ComputerApiException &&
              (error.statusCode == 401 || error.statusCode == 403);
          value = ComputerListState(
            computers: append && !accessChanged ? previous.computers : const [],
            nextCursor: append && !accessChanged ? after : null,
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
