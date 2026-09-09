import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_hooks/flutter_hooks.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:lucide_icons_flutter/lucide_icons.dart';

import '../../shared/machines/machines_api.dart';
import '../../shared/theme/theme.dart';
import '../../shared/widgets/buzz_loading_indicator.dart';
import 'computer_detail_page.dart';
import 'computer_list_query.dart';
import 'computer_presentation.dart';

/// Browses only the signing owner's enrolled computers in the selected community.
class ComputersPage extends HookConsumerWidget {
  const ComputersPage({super.key, required this.onBack, this.visible = true});
  final VoidCallback onBack;
  final bool visible;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final api = ref.watch(machinesApiProvider);
    final selection = useState<(MachinesApi, String)?>(null);
    final query = useMemoized(
      () => api == null
          ? null
          : ComputerListQuery((after) => api.list(after: after)),
      [api],
    );
    useEffect(() => query?.dispose, [query]);
    useEffect(() {
      if (!visible) return null;
      var disposed = false;
      scheduleMicrotask(() {
        if (!disposed) query?.refresh();
      });
      final listener = AppLifecycleListener(
        onResume: () {
          if (!disposed) unawaited(query?.refresh());
        },
      );
      return () {
        disposed = true;
        listener.dispose();
      };
    }, [query, visible]);
    final selected = selection.value;
    if (api != null && selected != null && identical(selected.$1, api)) {
      return ComputerDetailPage(
        key: ValueKey((api, selected.$2)),
        api: api,
        id: selected.$2,
        onBack: () => selection.value = null,
        visible: visible,
      );
    }
    return Scaffold(
      backgroundColor: context.colors.surface,
      appBar: AppBar(
        title: const Text('Computers'),
        leading: IconButton(
          tooltip: 'Back to Home',
          onPressed: onBack,
          icon: const Icon(LucideIcons.arrowLeft),
        ),
        actions: [
          IconButton(
            tooltip: 'Refresh computers',
            onPressed: query?.refresh,
            icon: const Icon(LucideIcons.refreshCw),
          ),
        ],
      ),
      body: query == null
          ? const Padding(
              padding: EdgeInsets.all(Grid.xl),
              child: Text(
                'Choose a community from Home to see your computers.',
              ),
            )
          : _ComputerList(
              query: query,
              visible: visible,
              onSelect: (id) {
                if (identical(ref.read(machinesApiProvider), api)) {
                  selection.value = (api!, id);
                }
              },
            ),
    );
  }
}

class _ComputerList extends HookConsumerWidget {
  const _ComputerList({
    required this.query,
    required this.visible,
    required this.onSelect,
  });
  final ComputerListQuery query;
  final bool visible;
  final ValueChanged<String> onSelect;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final state = useListenable(query).value;
    final now = useComputerObservationClock(ref, state.computers, visible);
    return RefreshIndicator(
      onRefresh: query.refresh,
      child: ListView(
        physics: const AlwaysScrollableScrollPhysics(),
        padding: const EdgeInsets.all(Grid.gutter),
        children: [
          for (final computer in state.computers)
            Card(
              margin: const EdgeInsets.only(bottom: Grid.xs),
              elevation: 0,
              color: context.colors.surfaceContainerHigh,
              shape: RoundedRectangleBorder(
                borderRadius: BorderRadius.circular(Radii.dialog),
              ),
              child: ListTile(
                subtitleTextStyle: context.textTheme.bodyMedium?.copyWith(
                  color: context.colors.onSurfaceVariant,
                ),
                key: ValueKey('computer-${computer.id}'),
                leading: Icon(
                  LucideIcons.monitor,
                  color: context.colors.primary,
                ),
                title: Text(computer.label),
                subtitle: Text(
                  '${computer.statusAt(now)}\n${computer.runtimeLabel}',
                ),
                isThreeLine: true,
                trailing: const Icon(LucideIcons.chevronRight),
                onTap: () => onSelect(computer.id),
              ),
            ),
          if (state.loading || state.loadingMore)
            const Padding(
              padding: EdgeInsets.all(Grid.xl),
              child: Center(
                child: BuzzLoadingIndicator(semanticLabel: 'Loading computers'),
              ),
            )
          else if (state.error != null)
            ComputerReadError(
              error: state.error!,
              onRetry: state.computers.isEmpty ? query.refresh : query.loadMore,
            )
          else if (state.nextCursor != null)
            Center(
              child: TextButton(
                onPressed: query.loadMore,
                child: const Text('Load more'),
              ),
            )
          else if (state.computers.isEmpty)
            const Padding(
              padding: EdgeInsets.all(Grid.xl),
              child: Text(
                'No computers have been added to this community yet.',
                textAlign: TextAlign.center,
              ),
            ),
          SizedBox(height: MediaQuery.paddingOf(context).bottom + Grid.xl),
        ],
      ),
    );
  }
}
