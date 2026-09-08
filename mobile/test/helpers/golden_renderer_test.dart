import 'package:flutter_test/flutter_test.dart';
import 'golden_renderer.dart';

void main() {
  const fingerprint = {
    'platform': 'macos',
    'osVersion': 'Version 26.4.1 (Build 25E253)',
    'abi': 'macos_arm64',
    'flutter': '3.41.7',
    'framework': 'framework-sha',
    'engine': 'engine-sha',
    'testerSha256': 'binary-sha',
    'fontsSha256': 'font-sha',
  };
  final profiles = [
    {'directory': 'macos-26-4-1-25e253', 'fingerprint': fingerprint},
  ];
  test('selects the exact independently qualified renderer', () {
    expect(
      qualifiedGoldenDirectory(fingerprint, profiles),
      'macos-26-4-1-25e253',
    );
    expect(
      qualifiedGoldenDirectory(fingerprint, [
        {'directory': '', 'fingerprint': fingerprint},
      ]),
      '',
    );
  });
  for (final field in fingerprint.keys) {
    test(
      'rejects an unqualified $field even when every other field matches',
      () {
        expect(
          () => qualifiedGoldenDirectory({
            ...fingerprint,
            field: 'unknown',
          }, profiles),
          throwsStateError,
        );
      },
    );
  }
  test('rejects incomplete and ambiguous profile matches', () {
    expect(
      () => qualifiedGoldenDirectory(
        {...fingerprint}..remove('engine'),
        profiles,
      ),
      throwsStateError,
    );
    expect(
      () => qualifiedGoldenDirectory(fingerprint, [...profiles, ...profiles]),
      throwsStateError,
    );
  });
  test('rejects profile directory traversal', () {
    expect(
      () => qualifiedGoldenDirectory(fingerprint, [
        {'directory': '../unreviewed', 'fingerprint': fingerprint},
      ]),
      throwsStateError,
    );
  });
}
