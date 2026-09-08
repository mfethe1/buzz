import 'dart:collection';
import 'dart:convert';
import 'dart:ffi';
import 'dart:io';

import 'package:crypto/crypto.dart';

import 'golden_shot.dart';

/// The real renderer inputs, independent of which golden a test will compare.
/// No environment override may select an already-qualified fingerprint.
Map<String, String> readGoldenRendererFingerprint() {
  final sdk = flutterSdkRoot();
  final version =
      jsonDecode(File('$sdk/bin/cache/flutter.version.json').readAsStringSync())
          as Map<String, dynamic>;
  final fonts = SplayTreeMap<String, String>();
  for (final directory in [
    Directory('${mobilePackageRoot()}/assets/fonts'),
    Directory('$sdk/bin/cache/artifacts/material_fonts'),
    Directory('${iconPackageRoot()}/assets'),
  ]) {
    for (final file in directory.listSync(recursive: true).whereType<File>()) {
      final name = file.uri.pathSegments.last.toLowerCase();
      if (!name.endsWith('.ttf') && !name.endsWith('.otf')) continue;
      final hash = sha256.convert(file.readAsBytesSync()).toString();
      if (fonts.containsKey(name) && fonts[name] != hash) {
        throw StateError('Different golden font inputs share the name $name');
      }
      fonts[name] = hash;
    }
  }
  return {
    'platform': Platform.operatingSystem,
    'osVersion': Platform.operatingSystemVersion,
    'abi': Abi.current().toString(),
    'flutter': version['frameworkVersion'] as String,
    'framework': version['frameworkRevision'] as String,
    'engine': version['engineRevision'] as String,
    'testerSha256': sha256
        .convert(File(Platform.resolvedExecutable).readAsBytesSync())
        .toString(),
    'fontsSha256': sha256.convert(utf8.encode(jsonEncode(fonts))).toString(),
  };
}

/// An exact alternative profile or an unqualified canonical comparison.
class GoldenRendererSelection {
  const GoldenRendererSelection(this.directory, {required this.qualified});
  final String directory;
  final bool qualified;
}

/// Unknown renderers retain the existing strict canonical comparison, but may
/// neither select an alternative profile nor overwrite the canonical images.
GoldenRendererSelection selectGoldenRenderer(
  Map<String, String> actual,
  List<dynamic> profiles, {
  bool updating = false,
}) {
  final matches = profiles.where((entry) {
    if (entry is! Map<String, dynamic>) return false;
    final fingerprint = entry['fingerprint'];
    return fingerprint is Map<String, dynamic> &&
        fingerprint.length == actual.length &&
        actual.entries.every((e) => fingerprint[e.key] == e.value);
  }).toList();
  if (matches.isEmpty && !updating) {
    return const GoldenRendererSelection('', qualified: false);
  }
  if (matches.length != 1) {
    throw StateError(
      'Golden renderer is not uniquely qualified. Preserve the existing PNGs; '
      'review this renderer independently before adding a profile:\n'
      '${jsonEncode(actual)}',
    );
  }
  final directory = (matches.single as Map<String, dynamic>)['directory'];
  if (directory is! String ||
      (directory.isNotEmpty && !RegExp(r'^[a-z0-9-]+$').hasMatch(directory))) {
    throw StateError('Invalid golden renderer directory');
  }
  return GoldenRendererSelection(directory, qualified: true);
}
