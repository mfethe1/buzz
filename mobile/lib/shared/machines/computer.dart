/// An owner-visible enrolled computer and its latest bounded observation.
class EnrolledComputer {
  const EnrolledComputer({
    required this.id,
    required this.ownerPubkey,
    required this.coordinatorPubkey,
    required this.label,
    required this.runtime,
    required this.sequence,
    required this.serverFresh,
    this.reportedState,
    this.observedAt,
    this.receivedAt,
    this.expiresAt,
  });

  final String id;
  final String ownerPubkey;
  final String coordinatorPubkey;
  final String label;
  final String runtime;
  final int sequence;
  final bool serverFresh;
  final ComputerReportedState? reportedState;
  final DateTime? observedAt;
  final DateTime? receivedAt;
  final DateTime? expiresAt;

  /// A cached report cannot remain fresh after its server-supplied deadline.
  bool freshAt(DateTime now) =>
      serverFresh && expiresAt != null && now.isBefore(expiresAt!);

  String get runtimeLabel => switch (runtime) {
    'hermes' => 'Hermes',
    'openclaw' => 'OpenClaw',
    'codex' => 'Codex',
    'claude-code' => 'Claude Code',
    _ => throw StateError('Unsupported computer runtime'),
  };

  String statusAt(DateTime now) => reportedState == null
      ? 'No update yet'
      : !freshAt(now)
      ? expiresAt != null && !now.isBefore(expiresAt!)
            ? 'Update expired'
            : 'Status unavailable'
      : switch (reportedState!) {
          ComputerReportedState.ready => 'Reported ready',
          ComputerReportedState.busy => 'Reported busy',
          ComputerReportedState.unavailable => 'Unavailable',
        };

  factory EnrolledComputer.fromJson(Map<String, dynamic> json) {
    String text(String key) {
      final value = json[key];
      if (value is! String || value.trim().isEmpty) {
        throw FormatException('Invalid computer $key');
      }
      return value;
    }

    DateTime? timestamp(String key) {
      final value = json[key];
      if (value == null) return null;
      if (value is! String ||
          !RegExp(r'(Z|[+-]\d{2}:\d{2})$').hasMatch(value)) {
        throw FormatException('Invalid computer $key');
      }
      return DateTime.parse(value).toUtc();
    }

    final id = text('machine_id');
    final owner = text('owner_pubkey');
    final coordinator = text('coordinator_pubkey');
    final label = text('label');
    final runtime = text('runtime');
    final sequence = json['observation_sequence'];
    final fresh = json['fresh'];
    final rawState = json['reported_state'];
    final state = rawState == null
        ? null
        : ComputerReportedState.values
              .where((v) => v.name == rawState)
              .singleOrNull;
    final observed = timestamp('observed_at');
    final received = timestamp('received_at');
    final expires = timestamp('expires_at');
    if (!isComputerId(id) ||
        !RegExp(r'^[0-9a-f]{64}$').hasMatch(owner) ||
        !RegExp(r'^[0-9a-f]{64}$').hasMatch(coordinator) ||
        owner == coordinator ||
        label.length > 80 ||
        label.codeUnits.any((v) => v < 32 || v == 127) ||
        !const [
          'hermes',
          'openclaw',
          'codex',
          'claude-code',
        ].contains(runtime) ||
        sequence is! int ||
        sequence < 0 ||
        sequence > 9007199254740991 ||
        fresh is! bool ||
        (rawState != null && state == null)) {
      throw const FormatException('Invalid computer response');
    }
    if (sequence == 0) {
      if (fresh ||
          state != null ||
          observed != null ||
          received != null ||
          expires != null) {
        throw const FormatException('Unexpected computer observation');
      }
    } else if (state == null ||
        observed == null ||
        received == null ||
        expires == null ||
        expires.isAfter(observed.add(const Duration(seconds: 120))) ||
        expires.isAfter(received.add(const Duration(seconds: 120)))) {
      throw const FormatException(
        'Incomplete or unbounded computer observation',
      );
    }
    return EnrolledComputer(
      id: id,
      ownerPubkey: owner,
      coordinatorPubkey: coordinator,
      label: label,
      runtime: runtime,
      sequence: sequence,
      serverFresh: fresh,
      reportedState: state,
      observedAt: observed,
      receivedAt: received,
      expiresAt: expires,
    );
  }
}

enum ComputerReportedState { ready, busy, unavailable }

bool isComputerId(String value) => RegExp(
  r'^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$',
).hasMatch(value);

class ComputerPage {
  const ComputerPage({required this.computers, this.nextCursor});
  final List<EnrolledComputer> computers;
  final String? nextCursor;
}
