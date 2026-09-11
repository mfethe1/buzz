import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../mentions/agent_identity_provider.dart';
import '../relay/relay.dart';

/// Task assignment choices from registered agents with an actual bot role in
/// the chosen conversation. This is UI eligibility, never an execution grant.
class TaskAssigneeDirectory {
  const TaskAssigneeDirectory({required this.actorPubkey, required this.query});

  final String? actorPubkey;
  final Future<List<NostrEvent>> Function(List<NostrFilter>) query;

  /// Reads current membership on every call, including immediately before a
  /// creation. Workspace tasks can be created unassigned.
  Future<List<AgentDirectoryEntry>> load(String? channelId) async {
    if (channelId == null) return const [];
    if (actorPubkey == null) throw StateError('missing signing identity');
    final events = await query([
      NostrFilters.agentProfiles(),
      NostrFilters.channelMembers(channelId),
    ]);
    final membership = events
        .where(
          (event) =>
              event.kind == 39002 &&
              event.tags.any(
                (tag) =>
                    tag.length >= 2 && tag[0] == 'd' && tag[1] == channelId,
              ),
        )
        .toList();
    if (membership.length != 1) {
      throw StateError('unavailable conversation membership');
    }
    final members = membersFromEvent(membership.single);
    if (!members.any(
      (member) => member.pubkey.toLowerCase() == actorPubkey?.toLowerCase(),
    )) {
      throw StateError('conversation membership changed');
    }
    final bots = {
      for (final member in members)
        if (member.role == 'bot') member.pubkey.toLowerCase(),
    };
    final agents = <String, AgentDirectoryEntry>{};
    for (final event in events.where((event) => event.kind == 10100)) {
      final entry = AgentDirectoryEntry.fromEvent(event);
      if (RegExp(r'^[0-9a-f]{64}$').hasMatch(entry.pubkey) &&
          bots.contains(entry.pubkey)) {
        agents.putIfAbsent(entry.pubkey, () => entry);
      }
    }
    final result = agents.values.toList()
      ..sort(
        (a, b) =>
            (a.displayName ?? a.pubkey).compareTo(b.displayName ?? b.pubkey),
      );
    return List.unmodifiable(result);
  }
}

/// Reuses the signed HTTP query path and fences every read to the captured
/// community/identity. An old form cannot query through a new session scope.
final taskAssigneeDirectoryProvider = Provider<TaskAssigneeDirectory>((ref) {
  final config = ref.watch(relayConfigProvider);
  final session = ref.read(relaySessionProvider.notifier);
  var disposed = false;
  ref.onDispose(() => disposed = true);
  void checkScope() {
    if (disposed || !identical(ref.read(relayConfigProvider), config)) {
      throw StateError('task workspace changed');
    }
  }

  return TaskAssigneeDirectory(
    actorPubkey: pubkeyFromNsec(config.nsec),
    query: (filters) async {
      checkScope();
      final events = await session.queryRelay(filters);
      checkScope();
      return events;
    },
  );
});
