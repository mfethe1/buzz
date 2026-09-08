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
    final selection = selectGoldenRenderer(fingerprint, profiles);
    expect(selection.directory, 'macos-26-4-1-25e253');
    expect(selection.qualified, isTrue);
    expect(
      selectGoldenRenderer(fingerprint, profiles, updating: true).directory,
      'macos-26-4-1-25e253',
    );
    expect(
      selectGoldenRenderer(fingerprint, [
        {'directory': '', 'fingerprint': fingerprint},
      ]).directory,
      '',
    );
  });
  for (final field in fingerprint.keys) {
    test('unknown $field compares canonical images but cannot update them', () {
      final actual = {...fingerprint, field: 'unknown'};
      final selection = selectGoldenRenderer(actual, profiles);
      expect(selection.directory, '');
      expect(selection.qualified, isFalse);
      expect(
        () => selectGoldenRenderer(actual, profiles, updating: true),
        throwsStateError,
      );
    });
  }
  test(
    'unknown Linux runner retains the canonical comparison without qualification',
    () {
      final selection = selectGoldenRenderer({
        ...fingerprint,
        'platform': 'linux',
        'abi': 'linux_x64',
        'osVersion': 'CI kernel',
      }, profiles);
      expect(selection.directory, '');
      expect(selection.qualified, isFalse);
    },
  );
  test('rejects incomplete updates and ambiguous profile matches', () {
    expect(
      () => selectGoldenRenderer(
        {...fingerprint}..remove('engine'),
        profiles,
        updating: true,
      ),
      throwsStateError,
    );
    expect(
      () => selectGoldenRenderer(fingerprint, [...profiles, ...profiles]),
      throwsStateError,
    );
  });
  test('rejects profile directory traversal', () {
    expect(
      () => selectGoldenRenderer(fingerprint, [
        {'directory': '../unreviewed', 'fingerprint': fingerprint},
      ]),
      throwsStateError,
    );
  });
}
