import 'package:flutter/foundation.dart';

/// A conversation the current identity is allowed to use for a task.
@immutable
class TaskChannel {
  /// Comes from the application's current membership-scoped channel list.
  const TaskChannel({required this.id, required this.name});

  /// Relay channel identifier.
  final String id;

  /// Display name; not an authorization or addressing key.
  final String name;
}
