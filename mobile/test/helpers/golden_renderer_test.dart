import 'dart:convert';
import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'golden_renderer.dart';
import 'golden_shot.dart';

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

  group('recorded Linux renderer qualification', () {
    const hosted = {
      'platform': 'linux',
      'osVersion':
          'Linux 6.17.0-1022-azure #22-Ubuntu SMP Mon Jul 27 17:24:03 UTC 2026',
      'abi': 'linux_x64',
      'flutter': '3.41.7',
      'framework': 'cc0734ac716fbb8b90f3f9db8020958b1553afa7',
      'engine': '59aa584fdf100e6c78c785d8a5b565d1de4b48ab',
      'testerSha256':
          '4542c5cec954f0aa5d08d6012c10e1e79762dd2e79ddb5d6dfd24b1d831b5171',
      'fontsSha256':
          '6b7db69026d649261d441120392bffae8bbccf1d61f368d8752b4ff2f780847a',
    };
    late List<dynamic> recorded;
    setUp(() {
      recorded =
          jsonDecode(
                File(
                  '${mobilePackageRoot()}/test/features/channels/agent_activity/goldens/renderer_profiles.json',
                ).readAsStringSync(),
              )
              as List<dynamic>;
    });

    for (final osVersion in [
      hosted['osVersion']!,
      'Linux 6.8.0-117-generic #117-Ubuntu SMP PREEMPT_DYNAMIC Thu May  7 17:26:37 UTC 2026',
    ]) {
      test('selects the recorded exact Linux renderer $osVersion', () {
        final selection = selectGoldenRenderer({
          ...hosted,
          'osVersion': osVersion,
        }, recorded);
        expect(selection.qualified, isTrue);
        expect(selection.directory, 'linux-x64-flutter-3-41-7');
      });
    }

    test('changed Linux inputs cannot select or update an alternative', () {
      for (final field in hosted.keys) {
        final unknown = {...hosted, field: '${hosted[field]}-unobserved'};
        final selection = selectGoldenRenderer(unknown, recorded);
        expect(selection.qualified, isFalse, reason: field);
        expect(selection.directory, '', reason: field);
        expect(
          () => selectGoldenRenderer(unknown, recorded, updating: true),
          throwsStateError,
          reason: field,
        );
      }
    });

    test('incomplete Linux fingerprint cannot qualify an update', () {
      final incomplete = {...hosted}..remove('engine');
      expect(selectGoldenRenderer(incomplete, recorded).qualified, isFalse);
      expect(
        () => selectGoldenRenderer(incomplete, recorded, updating: true),
        throwsStateError,
      );
    });
  });
}
