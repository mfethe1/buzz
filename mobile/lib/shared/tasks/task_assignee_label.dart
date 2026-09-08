import 'package:flutter/material.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../mentions/agent_identity_provider.dart';

/// Presents the persisted assignee; a name is a label, never its address.
class TaskAssigneeLabel extends ConsumerWidget {
  const TaskAssigneeLabel({super.key, required this.pubkey});
  final String pubkey;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final names = ref.watch(agentDirectoryDisplayNamesProvider);
    final name = names[pubkey.toLowerCase()];
    final shortKey = String.fromCharCodes(pubkey.runes.take(8));
    final label = name == null
        ? shortKey
        : '${String.fromCharCodes(name.runes.take(80))} · $shortKey';
    return Text(
      'Assigned to $label',
      maxLines: 2,
      overflow: TextOverflow.ellipsis,
    );
  }
}
