import 'dart:convert';
import 'dart:io';
import 'dart:typed_data';

import 'package:buzz/shared/relay/mp4_fast_start.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  late Directory tempDirectory;

  setUp(() async {
    tempDirectory = await Directory.systemTemp.createTemp(
      'buzz-faststart-test-',
    );
  });

  tearDown(() async {
    await tempDirectory.delete(recursive: true);
  });

  for (final offsetBox in ['stco', 'co64']) {
    test('moves moov before mdat and patches $offsetBox offsets', () async {
      final ftyp = _box('ftyp', [
        ...ascii.encode('isom'),
        0,
        0,
        0,
        0,
        ...ascii.encode('isom'),
      ]);
      final mdat = _box('mdat', [1, 2, 3, 4]);
      final originalMediaOffset = ftyp.length + 8;
      final offsetPayload = BytesBuilder()
        ..add([0, 0, 0, 0])
        ..add(_uint32(1))
        ..add(
          offsetBox == 'stco'
              ? _uint32(originalMediaOffset)
              : _uint64(originalMediaOffset),
        );
      final sampleTable = _box(offsetBox, offsetPayload.takeBytes());
      final moov = _nestedMoov(sampleTable);
      final sourceBytes = Uint8List.fromList([...ftyp, ...mdat, ...moov]);
      final source = File('${tempDirectory.path}/source.mp4');
      final destination = File('${tempDirectory.path}/output.mp4');
      await source.writeAsBytes(sourceBytes);

      await rewriteMp4ForFastStart(source, destination);

      final output = await destination.readAsBytes();
      expect(_topLevelTypes(output), ['ftyp', 'moov', 'mdat']);
      expect(
        output.sublist(ftyp.length + moov.length + 8),
        equals([1, 2, 3, 4]),
      );

      final typeOffset = _findAscii(output, offsetBox);
      expect(typeOffset, greaterThanOrEqualTo(4));
      final entryOffset = typeOffset + 12;
      final adjusted = offsetBox == 'stco'
          ? _readUint32(output, entryOffset)
          : _readUint64(output, entryOffset);
      expect(adjusted, originalMediaOffset + moov.length);
    });
  }

  test('co64 offsets outside the moved range retain all eight bytes', () async {
    final ftyp = _box('ftyp', ascii.encode('isom'));
    final mdat = _box('mdat', [1, 2, 3, 4]);
    // Includes the first inexact JS integer and high-bit unsigned values.
    // They do not refer to the moved payload and must remain byte-for-byte.
    const untouched = <int>[
      0,
      0x20,
      0,
      0,
      0,
      0,
      0,
      1,
      0x7f,
      0xff,
      0xff,
      0xff,
      0xff,
      0xff,
      0xff,
      0xff,
      0x80,
      0,
      0,
      0,
      0,
      0,
      0,
      1,
    ];
    final moov = _nestedMoov(
      _box('co64', [
        0,
        0,
        0,
        0,
        ..._uint32(4),
        ..._uint64(ftyp.length + 8),
        ...untouched,
      ]),
    );
    final source = File('${tempDirectory.path}/mixed-offsets.mp4');
    final destination = File('${tempDirectory.path}/output.mp4');
    await source.writeAsBytes([...ftyp, ...mdat, ...moov]);
    await rewriteMp4ForFastStart(source, destination);
    final output = await destination.readAsBytes();
    final entries = _findAscii(output, 'co64') + 12;
    expect(_readUint64(output, entries), ftyp.length + 8 + moov.length);
    expect(
      output.sublist(entries + 8, entries + 8 + untouched.length),
      untouched,
    );
    expect(_topLevelTypes(output), ['ftyp', 'moov', 'mdat']);
  });

  test(
    'rejects excessive nested box depth and deletes partial output',
    () async {
      final ftyp = _box('ftyp', [
        ...ascii.encode('isom'),
        0,
        0,
        0,
        0,
        ...ascii.encode('isom'),
      ]);
      final mdat = _box('mdat', [1]);
      var nested = _box('stco', [0, 0, 0, 0, ..._uint32(0)]);
      for (var index = 0; index < 34; index++) {
        nested = _box('stbl', nested);
      }
      final source = File('${tempDirectory.path}/deep.mp4');
      final destination = File('${tempDirectory.path}/output.mp4');
      await source.writeAsBytes([...ftyp, ...mdat, ..._box('moov', nested)]);

      await expectLater(
        rewriteMp4ForFastStart(source, destination),
        throwsA(isA<FormatException>()),
      );
      expect(await destination.exists(), isFalse);
    },
  );
}

Uint8List _nestedMoov(List<int> leaf) =>
    _box('moov', _box('trak', _box('mdia', _box('minf', _box('stbl', leaf)))));

Uint8List _box(String type, List<int> payload) => Uint8List.fromList([
  ..._uint32(payload.length + 8),
  ...ascii.encode(type),
  ...payload,
]);

Uint8List _uint32(int value) {
  final bytes = ByteData(4)..setUint32(0, value, Endian.big);
  return bytes.buffer.asUint8List();
}

Uint8List _uint64(int value) {
  final bytes = ByteData(8)..setUint64(0, value, Endian.big);
  return bytes.buffer.asUint8List();
}

int _readUint32(Uint8List bytes, int offset) =>
    ByteData.sublistView(bytes).getUint32(offset, Endian.big);

int _readUint64(Uint8List bytes, int offset) =>
    ByteData.sublistView(bytes).getUint64(offset, Endian.big);

List<String> _topLevelTypes(Uint8List bytes) {
  final types = <String>[];
  var offset = 0;
  while (offset < bytes.length) {
    final size = _readUint32(bytes, offset);
    types.add(ascii.decode(bytes.sublist(offset + 4, offset + 8)));
    offset += size;
  }
  return types;
}

int _findAscii(Uint8List bytes, String value) {
  final needle = ascii.encode(value);
  for (var offset = 0; offset + needle.length <= bytes.length; offset++) {
    var matches = true;
    for (var index = 0; index < needle.length; index++) {
      if (bytes[offset + index] != needle[index]) {
        matches = false;
        break;
      }
    }
    if (matches) return offset;
  }
  return -1;
}
