import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_hooks/flutter_hooks.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:intl/intl.dart';
import 'package:lucide_icons_flutter/lucide_icons.dart';

import '../../shared/machines/computer.dart';
import '../../shared/machines/machines_api.dart';
import '../../shared/theme/theme.dart';
import '../../shared/widgets/buzz_loading_indicator.dart';
import 'computer_presentation.dart';

class ComputerDetailPage extends HookConsumerWidget {
  const ComputerDetailPage({
    super.key,
    required this.api,
    required this.id,
    required this.onBack,
    required this.visible,
  });
  final MachinesApi api;
  final String id;
  final VoidCallback onBack;
  final bool visible;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final refresh = useState(0);
    final future = useMemoized(() => api.get(id), [api, id, refresh.value]);
    // An earlier computer or identity must never remain visible while reading.
    final snapshot = useFuture(future, preserveState: false);
    final computer = snapshot.data;
    final now = useComputerObservationClock(
      ref,
      computer == null ? [] : [computer],
      visible,
    );
    useEffect(() {
      final listener = AppLifecycleListener(
        onResume: () {
          if (visible) refresh.value++;
        },
      );
      return listener.dispose;
    }, [visible]);
    final wasVisible = useRef(visible);
    useEffect(() {
      final becameVisible = visible && !wasVisible.value;
      wasVisible.value = visible;
      if (!becameVisible) return null;
      var disposed = false;
      scheduleMicrotask(() {
        if (!disposed) refresh.value++;
      });
      return () => disposed = true;
    }, [visible]);

    return Scaffold(
      backgroundColor: context.colors.surface,
      appBar: AppBar(
        leading: IconButton(
          tooltip: 'Back to Computers',
          onPressed: onBack,
          icon: const Icon(LucideIcons.arrowLeft),
        ),
        title: const Text('Computer details'),
        actions: [
          IconButton(
            tooltip: 'Refresh computer',
            onPressed: () => refresh.value++,
            icon: const Icon(LucideIcons.refreshCw),
          ),
        ],
      ),
      body: snapshot.connectionState != ConnectionState.done
          ? const Center(
              child: BuzzLoadingIndicator(semanticLabel: 'Loading computer'),
            )
          : snapshot.hasError
          ? ComputerReadError(
              error: snapshot.error!,
              onRetry: () => refresh.value++,
              detail: true,
            )
          : computer == null
          ? const SizedBox.shrink()
          : ListView(
              padding: const EdgeInsets.all(Grid.gutter),
              children: [
                Icon(
                  LucideIcons.monitor,
                  size: 48,
                  color: context.colors.primary,
                ),
                const SizedBox(height: Grid.md),
                Text(
                  computer.label,
                  style: context.textTheme.headlineSmall,
                  textAlign: TextAlign.center,
                ),
                const SizedBox(height: Grid.xs),
                Text(computer.statusAt(now), textAlign: TextAlign.center),
                const SizedBox(height: Grid.lg),
                Card(
                  margin: EdgeInsets.zero,
                  elevation: 0,
                  color: context.colors.surfaceContainerHigh,
                  child: Column(
                    children: [
                      ListTile(
                        subtitleTextStyle: context.textTheme.bodyMedium
                            ?.copyWith(color: context.colors.onSurfaceVariant),
                        title: const Text('Agent runtime'),
                        subtitle: Text(computer.runtimeLabel),
                        leading: const Icon(LucideIcons.bot),
                      ),
                      ListTile(
                        subtitleTextStyle: context.textTheme.bodyMedium
                            ?.copyWith(color: context.colors.onSurfaceVariant),
                        title: const Text('Last update'),
                        subtitle: Text(
                          computer.observedAt == null
                              ? 'No update received'
                              : DateFormat.yMMMd().add_jm().format(
                                  computer.observedAt!.toLocal(),
                                ),
                        ),
                        leading: const Icon(LucideIcons.clock),
                      ),
                    ],
                  ),
                ),
                const SizedBox(height: Grid.md),
                Text(
                  computer.reportedState == null
                      ? 'This computer has been added, but has not sent an update yet.'
                      : !computer.freshAt(now)
                      ? computer.expiresAt != null &&
                                !now.isBefore(computer.expiresAt!)
                            ? 'The last update has expired. Refresh to check for a new update.'
                            : 'A current status is unavailable. Refresh to check for a new update.'
                      : computer.reportedState ==
                            ComputerReportedState.unavailable
                      ? 'This computer reported that it is unavailable.'
                      : 'This is the latest status reported by this computer.',
                  style: context.textTheme.bodyMedium,
                ),
              ],
            ),
    );
  }
}
