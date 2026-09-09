part of '../channels_page.dart';

class _ChannelsBody extends StatelessWidget {
  final List<Channel>? channels;
  final VoidCallback? onOpenWork;
  final VoidCallback? onOpenComputers;
  final AsyncValue<List<Channel>> channelsAsync;
  final bool showError;
  final SessionStatus sessionStatus;
  final bool showConnectionSkeleton;
  final String? currentPubkey;
  final double topSectionHeight;
  final bool usesPinnedGradient;
  final ScrollController scrollController;
  final Future<void> Function() onRefresh;
  final Future<void> Function(Channel channel) onSelectChannel;

  const _ChannelsBody({
    this.onOpenWork,
    this.onOpenComputers,
    required this.channels,
    required this.channelsAsync,
    required this.showError,
    required this.sessionStatus,
    required this.showConnectionSkeleton,
    required this.currentPubkey,
    required this.topSectionHeight,
    required this.usesPinnedGradient,
    required this.scrollController,
    required this.onRefresh,
    required this.onSelectChannel,
  });

  @override
  Widget build(BuildContext context) {
    final barHeight = topSectionHeight;
    final loadedChannels = channels;
    final loading =
        showConnectionSkeleton || (loadedChannels == null && !showError);
    final hasShortcuts = onOpenWork != null || onOpenComputers != null;
    final shortcuts = _HomeShortcuts(
      onOpenWork: onOpenWork,
      onOpenComputers: onOpenComputers,
    );
    final content = showError && channelsAsync.hasError
        ? Padding(
            padding: EdgeInsets.only(top: barHeight),
            child: _ErrorView(error: channelsAsync.error!, onRetry: onRefresh),
          )
        : loadedChannels == null
        ? const SizedBox.shrink()
        : BeeRefreshIndicator(
            edgeOffset: barHeight,
            onRefresh: onRefresh,
            child: CustomScrollView(
              controller: scrollController,
              // Transparent list gaps must remain hit-testable so a new drag
              // can interrupt ballistic scrolling. The app bar is painted
              // later and retains its community and profile controls.
              hitTestBehavior: HitTestBehavior.translucent,
              slivers: [
                SliverToBoxAdapter(child: SizedBox(height: barHeight)),
                if (hasShortcuts) SliverToBoxAdapter(child: shortcuts),
                if (usesPinnedGradient)
                  _SliverChannelsList(
                    channels: loadedChannels,
                    currentPubkey: currentPubkey,
                    onSelectChannel: onSelectChannel,
                  )
                else
                  DecoratedSliver(
                    decoration: BoxDecoration(
                      color: context.colors.surface,
                      borderRadius: const BorderRadius.vertical(
                        top: Radius.circular(Radii.dialog),
                      ),
                    ),
                    sliver: _SliverChannelsList(
                      channels: loadedChannels,
                      currentPubkey: currentPubkey,
                      onSelectChannel: onSelectChannel,
                    ),
                  ),
              ],
            ),
          );

    return SkeletonReveal(
      loading: loading && !hasShortcuts,
      shimmerEnabled: sessionStatus != SessionStatus.disconnected,
      skeleton: _ChannelsSkeleton(
        channels: loadedChannels,
        topInset: barHeight,
        status: sessionStatus,
      ),
      content: hasShortcuts && (loading || showError)
          ? BeeRefreshIndicator(
              edgeOffset: barHeight,
              onRefresh: onRefresh,
              child: CustomScrollView(
                controller: scrollController,
                physics: const AlwaysScrollableScrollPhysics(),
                slivers: [
                  SliverToBoxAdapter(child: SizedBox(height: barHeight)),
                  SliverToBoxAdapter(child: shortcuts),
                  SliverFillRemaining(
                    hasScrollBody: false,
                    child: showError && channelsAsync.hasError
                        ? _ErrorView(
                            error: channelsAsync.error!,
                            onRetry: onRefresh,
                          )
                        : const Center(
                            child: BuzzLoadingIndicator(
                              semanticLabel: 'Loading conversations',
                            ),
                          ),
                  ),
                ],
              ),
            )
          : content,
    );
  }
}

class _SliverChannelsList extends HookConsumerWidget {
  final List<Channel> channels;
  final String? currentPubkey;
  final Future<void> Function(Channel channel) onSelectChannel;

  const _SliverChannelsList({
    required this.channels,
    required this.currentPubkey,
    required this.onSelectChannel,
  });

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final readState = ref.watch(readStateProvider);
    final sectionsState = ref.watch(channelSectionsProvider);
    final mutesState = ref.watch(channelMutesProvider);
    final mutedChannelIds = {
      for (final entry in mutesState.store.channels.entries)
        if (entry.value.muted) entry.key,
    };
    final starsState = ref.watch(channelStarsProvider);
    final starredChannelIds = {
      for (final entry in starsState.store.channels.entries)
        if (entry.value.starred) entry.key,
    };
    final visibleChannels = channels
        .where((channel) => channel.isMember && !channel.isArchived)
        .toList();
    final streamChannels = visibleChannels
        .where((channel) => channel.isStream)
        .toList();
    final dmChannels = sortDmChannelsByDisplayLabel(
      visibleChannels.where((channel) => channel.isDm),
      currentPubkey: currentPubkey,
    );

    final starredExpanded = useState(true);
    final channelsExpanded = useState(true);
    final dmsExpanded = useState(true);
    final sortState = ref.watch(channelSortProvider);
    final initialSeedComplete = useState(false);
    final seededPubkey = useRef<String?>(null);
    final seedCompleteForPubkey =
        seededPubkey.value == readState.pubkey && initialSeedComplete.value;

    useEffect(() {
      if (!readState.isReady) {
        return null;
      }

      return deferReadStateUpdate(context, () {
        if (seededPubkey.value != readState.pubkey) {
          seededPubkey.value = readState.pubkey;
          initialSeedComplete.value = false;
        }

        if (initialSeedComplete.value) {
          return;
        }

        final notifier = ref.read(readStateProvider.notifier);
        for (final channel in visibleChannels) {
          if (readState.effectiveTimestamp(channel.id) != null) {
            continue;
          }

          final lastMessageAt = dateTimeToUnixSeconds(channel.lastMessageAt);
          if (lastMessageAt != null) {
            notifier.seedContextRead(channel.id, lastMessageAt);
          }
        }
        initialSeedComplete.value = true;
      });
    }, [readState.isReady, readState.pubkey, visibleChannels]);

    final unreadState = _computeUnreadChannelState(
      channels: visibleChannels,
      readState: readState,
      channelsNotifier: ref.read(channelsProvider.notifier),
    );
    final unreadChannelIds = {
      for (final channelId in unreadState.ids)
        if (seedCompleteForPubkey ||
            readState.effectiveTimestamp(channelId) != null)
          channelId,
    };
    // Build sorted user-defined sections and compute which stream channels
    // belong to each section. Channels not assigned to any valid section fall
    // through to the built-in "Channels" list.
    final userSections = sectionsState.store.sections.toList()
      ..sort((a, b) => a.order.compareTo(b.order));
    final sectionAssignments = sectionsState.store.assignments;
    final validSectionIds = {for (final s in userSections) s.id};
    final assignedChannelIds = {
      for (final entry in sectionAssignments.entries)
        if (validSectionIds.contains(entry.value)) entry.key,
    };
    // Starred is exclusive: a starred channel lives only in the Starred section,
    // not in its custom section or the default Channels list.
    final starredStreamChannels = sortChannelsForList(
      streamChannels.where((c) => starredChannelIds.contains(c.id)).toList(),
      sortState.sortModeFor('starred'),
    );
    final ungroupedStreamChannels = sortChannelsForList(
      streamChannels
          .where(
            (c) =>
                !assignedChannelIds.contains(c.id) &&
                !starredChannelIds.contains(c.id),
          )
          .toList(),
      sortState.sortModeFor('channels'),
    );
    // DMs default to the display-label alphabetical order (labels can differ
    // from channel names); Recent mode reorders by last message time.
    final sortedDmChannels =
        sortState.sortModeFor('dms') == ChannelSortMode.recent
        ? sortChannelsForList(dmChannels, ChannelSortMode.recent)
        : dmChannels;

    final liveSectionIds = [for (final s in userSections) s.id];
    void setSortMode(String groupKey, ChannelSortMode mode) {
      ref
          .read(channelSortProvider.notifier)
          .setSortModeFor(groupKey, mode, liveSectionIds: liveSectionIds);
    }

    final sectionExpandedStates = useState<Map<String, bool>>({});

    bool sectionExpanded(String sectionId) =>
        sectionExpandedStates.value[sectionId] ?? true;

    void toggleSection(String sectionId) {
      sectionExpandedStates.value = {
        ...sectionExpandedStates.value,
        sectionId: !sectionExpanded(sectionId),
      };
    }

    return SliverPadding(
      padding: EdgeInsets.only(
        top: Grid.xxs,
        bottom: MediaQuery.paddingOf(context).bottom,
      ),
      sliver: SliverList.list(
        children: [
          if (visibleChannels.isEmpty)
            const _EmptyState()
          else ...[
            // Starred channels (exclusive — pinned above all sections).
            if (starredStreamChannels.isNotEmpty)
              _ChannelSection(
                title: 'Starred',
                icon: LucideIcons.star,
                showTopDivider: false,
                expanded: starredExpanded.value,
                onToggle: () => starredExpanded.value = !starredExpanded.value,
                channels: starredStreamChannels,
                unreadChannelIds: unreadChannelIds,
                mutedChannelIds: mutedChannelIds,
                currentPubkey: currentPubkey,
                emptyLabel: '',
                sortMode: sortState.sortModeFor('starred'),
                onSortModeChange: (mode) => setSortMode('starred', mode),
                onSelectChannel: onSelectChannel,
              ),
            // User-defined sections for stream channels, in user-defined order.
            for (final section in userSections)
              _CustomChannelSection(
                section: section,
                channels: sortChannelsForList(
                  streamChannels
                      .where(
                        (c) =>
                            sectionAssignments[c.id] == section.id &&
                            !starredChannelIds.contains(c.id),
                      )
                      .toList(),
                  sortState.sortModeFor(sectionSortGroupKey(section.id)),
                ),
                unreadChannelIds: unreadChannelIds,
                mutedChannelIds: mutedChannelIds,
                currentPubkey: currentPubkey,
                expanded: sectionExpanded(section.id),
                isFirst: userSections.first.id == section.id,
                isLast: userSections.last.id == section.id,
                showTopDivider:
                    starredStreamChannels.isNotEmpty ||
                    userSections.first.id != section.id,
                onToggle: () => toggleSection(section.id),
                onRename: () async {
                  final name = await showBuzzDialog<String>(
                    context: context,
                    builder: (_) => _SectionNameDialog(
                      title: 'Rename Section',
                      confirmLabel: 'Rename',
                      initialValue: section.name,
                    ),
                  );
                  if (name != null && name.isNotEmpty) {
                    ref
                        .read(channelSectionsProvider.notifier)
                        .renameSection(section.id, name);
                  }
                },
                onDelete: () async {
                  final confirmed = await showBuzzDialog<bool>(
                    context: context,
                    builder: (_) => AlertDialog(
                      title: Text('Delete "${section.name}"?'),
                      content: const Text(
                        'Channels in this section will move back to the main list.',
                      ),
                      actions: [
                        TextButton(
                          onPressed: () => Navigator.pop(context, false),
                          child: const Text('Cancel'),
                        ),
                        TextButton(
                          onPressed: () => Navigator.pop(context, true),
                          child: Text(
                            'Delete',
                            style: TextStyle(color: context.colors.error),
                          ),
                        ),
                      ],
                    ),
                  );
                  if (confirmed == true) {
                    ref
                        .read(channelSectionsProvider.notifier)
                        .deleteSection(section.id);
                  }
                },
                onMoveUp: () => ref
                    .read(channelSectionsProvider.notifier)
                    .moveSectionUp(section.id),
                onMoveDown: () => ref
                    .read(channelSectionsProvider.notifier)
                    .moveSectionDown(section.id),
                sortMode: sortState.sortModeFor(
                  sectionSortGroupKey(section.id),
                ),
                onSortModeChange: (mode) =>
                    setSortMode(sectionSortGroupKey(section.id), mode),
                onSelectChannel: onSelectChannel,
                onMarkChannelRead: (channel) {
                  final ts = dateTimeToUnixSeconds(channel.lastMessageAt);
                  if (ts != null) {
                    ref
                        .read(readStateProvider.notifier)
                        .markContextRead(
                          channel.id,
                          ts,
                          clearForcedMessages: true,
                        );
                    ref
                        .read(channelsProvider.notifier)
                        .clearObservedUnreadCoveredByRead(channel.id, ts);
                  }
                },
              ),
            _ChannelSection(
              title: 'Channels',
              icon: LucideIcons.hash,
              showTopDivider:
                  starredStreamChannels.isNotEmpty || userSections.isNotEmpty,
              expanded: channelsExpanded.value,
              onToggle: () => channelsExpanded.value = !channelsExpanded.value,
              channels: ungroupedStreamChannels,
              unreadChannelIds: unreadChannelIds,
              mutedChannelIds: mutedChannelIds,
              currentPubkey: currentPubkey,
              emptyLabel: 'No stream channels yet',
              sortMode: sortState.sortModeFor('channels'),
              onSortModeChange: (mode) => setSortMode('channels', mode),
              onSelectChannel: onSelectChannel,
            ),
            _ChannelSection(
              title: 'DMs',
              icon: LucideIcons.messagesSquare,
              showTopDivider: true,
              expanded: dmsExpanded.value,
              onToggle: () => dmsExpanded.value = !dmsExpanded.value,
              channels: sortedDmChannels,
              unreadChannelIds: unreadChannelIds,
              mutedChannelIds: mutedChannelIds,
              currentPubkey: currentPubkey,
              emptyLabel: 'No direct messages yet',
              sortMode: sortState.sortModeFor('dms'),
              onSortModeChange: (mode) => setSortMode('dms', mode),
              onSelectChannel: onSelectChannel,
            ),
          ],
        ],
      ),
    );
  }
}

class _HomeShortcut extends StatelessWidget {
  const _HomeShortcut({
    super.key,
    required this.title,
    required this.subtitle,
    required this.icon,
    required this.onTap,
  });
  final String title;
  final String subtitle;
  final IconData icon;
  final VoidCallback onTap;
  @override
  Widget build(BuildContext context) => Card(
    margin: EdgeInsets.zero,
    elevation: 0,
    color: context.colors.surfaceContainerHigh,
    shape: RoundedRectangleBorder(
      borderRadius: BorderRadius.circular(Radii.dialog),
    ),
    child: InkWell(
      onTap: onTap,
      borderRadius: BorderRadius.circular(Radii.dialog),
      child: Padding(
        padding: const EdgeInsets.all(Grid.xs),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Icon(icon, color: context.colors.primary),
            const SizedBox(height: Grid.sm),
            Text(title, style: context.textTheme.titleMedium),
            const SizedBox(height: Grid.xxs),
            Text(subtitle, style: context.textTheme.bodySmall),
          ],
        ),
      ),
    ),
  );
}

/// Keep navigation usable while conversations are loading or unavailable.
class _HomeShortcuts extends StatelessWidget {
  const _HomeShortcuts({this.onOpenWork, this.onOpenComputers});
  final VoidCallback? onOpenWork;
  final VoidCallback? onOpenComputers;
  @override
  Widget build(BuildContext context) {
    final cards = [
      if (onOpenWork != null)
        _HomeShortcut(
          key: const ValueKey('home-work-entry'),
          title: 'Work',
          subtitle: 'View and assign tasks',
          icon: LucideIcons.clipboardList,
          onTap: onOpenWork!,
        ),
      if (onOpenComputers != null)
        _HomeShortcut(
          key: const ValueKey('home-computers-entry'),
          title: 'Computers',
          subtitle: 'View your computers',
          icon: LucideIcons.monitor,
          onTap: onOpenComputers!,
        ),
    ];
    return Padding(
      padding: const EdgeInsets.fromLTRB(
        Grid.gutter,
        Grid.xxs,
        Grid.gutter,
        Grid.xs,
      ),
      child: LayoutBuilder(
        builder: (context, constraints) {
          final stack =
              constraints.maxWidth < 300 ||
              MediaQuery.textScalerOf(context).scale(1) > 1.3;
          if (stack) {
            return Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                for (var i = 0; i < cards.length; i++) ...[
                  if (i > 0) const SizedBox(height: Grid.twelve),
                  cards[i],
                ],
              ],
            );
          }
          return IntrinsicHeight(
            child: Row(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                for (var i = 0; i < cards.length; i++) ...[
                  if (i > 0) const SizedBox(width: Grid.twelve),
                  Expanded(child: cards[i]),
                ],
              ],
            ),
          );
        },
      ),
    );
  }
}
