import 'dart:convert';

import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:http/http.dart' as http;

import '../community/community_provider.dart';
import '../relay/relay.dart';
import 'computer.dart';

/// Read-only owner API. No development-auth fallback or execution operation.
class MachinesApi {
  MachinesApi({
    required http.Client httpClient,
    required String baseUrl,
    required String nsec,
  }) : _http = httpClient,
       _baseUrl = baseUrl,
       _nsec = nsec;

  final http.Client _http;
  final String _baseUrl;
  final String _nsec;

  Future<ComputerPage> list({String? after, int limit = 20}) async {
    if (limit < 1 || limit > 100 || (after != null && !isComputerId(after))) {
      throw ArgumentError('Invalid computer page');
    }
    final object = await _get('/api/machines', {
      'limit': '$limit',
      'after': ?after,
    });
    final rows = object['machines'];
    final next = object['next_cursor'];
    if (rows is! List ||
        (next != null && (next is! String || !isComputerId(next)))) {
      throw const FormatException('Invalid computer page');
    }
    return ComputerPage(
      computers: List.unmodifiable(rows.map(_computer)),
      nextCursor: next as String?,
    );
  }

  Future<EnrolledComputer> get(String id) async {
    if (!isComputerId(id)) throw ArgumentError('Invalid computer ID');
    final computer = _computer(await _get('/api/machines/$id'));
    if (computer.id != id) {
      throw const FormatException('Computer identity changed');
    }
    return computer;
  }

  EnrolledComputer _computer(Object? raw) {
    if (raw is! Map<String, dynamic>) {
      throw const FormatException('Invalid computer');
    }
    final computer = EnrolledComputer.fromJson(raw);
    if (computer.ownerPubkey != pubkeyFromNsec(_nsec)) {
      throw const FormatException(
        'Computer owner does not match this identity',
      );
    }
    return computer;
  }

  Future<Map<String, dynamic>> _get(
    String path, [
    Map<String, String>? query,
  ]) async {
    final uri = Uri.parse(
      _baseUrl,
    ).resolve(path).replace(queryParameters: query);
    final request = http.Request('GET', uri)
      ..headers['Authorization'] = buildNip98AuthHeader(
        method: 'GET',
        url: uri.toString(),
        bodyBytes: const [],
        nsec: _nsec,
      );
    final response = await _http
        .send(request)
        .then(http.Response.fromStream)
        .timeout(const Duration(seconds: 15));
    if (response.statusCode < 200 || response.statusCode >= 300) {
      throw ComputerApiException(response.statusCode);
    }
    final decoded = jsonDecode(response.body);
    if (decoded is! Map<String, dynamic>) {
      throw const FormatException('Invalid computer response');
    }
    return decoded;
  }
}

class ComputerApiException implements Exception {
  const ComputerApiException(this.statusCode);
  final int statusCode;
}

final machinesHttpClientProvider = Provider<http.Client>((ref) {
  final client = http.Client();
  ref.onDispose(client.close);
  return client;
});

/// A pending community transition clears private computer data immediately.
final machinesApiProvider = Provider<MachinesApi?>((ref) {
  final selected = ref.watch(activeCommunityProvider);
  if (selected.isLoading || selected.hasError || selected.value == null) {
    return null;
  }
  final config = ref.watch(relayConfigProvider);
  final nsec = config.nsec;
  if (nsec == null || nsec.isEmpty) return null;
  return MachinesApi(
    httpClient: ref.watch(machinesHttpClientProvider),
    baseUrl: config.baseUrl,
    nsec: nsec,
  );
});
