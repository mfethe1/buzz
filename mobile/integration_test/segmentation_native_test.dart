import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:google_mlkit_selfie_segmentation/google_mlkit_selfie_segmentation.dart';
import 'package:image/image.dart' as image;
import 'package:integration_test/integration_test.dart';

void main() {
  IntegrationTestWidgetsFlutterBinding.ensureInitialized();

  for (final mode in [SegmenterMode.single, SegmenterMode.stream]) {
    testWidgets(
      'native segmentation processes two file images in ${mode.name} mode',
      (tester) async {
        expect(Platform.isIOS || Platform.isAndroid, isTrue);
        final masks = await tester.runAsync(() async {
          final directory = await Directory.systemTemp.createTemp(
            'buzz-native-segmentation-',
          );
          final segmenter = SelfieSegmenter(
            mode: mode,
            enableRawSizeMask: false,
          );
          final results = <SegmentationMask?>[];
          try {
            for (var frame = 0; frame < 2; frame++) {
              final input = image.Image(width: 64, height: 64, numChannels: 3);
              image.fill(
                input,
                color: image.ColorRgb8(64 + frame * 32, 96, 128),
              );
              final file = File('${directory.path}/frame-$frame.png');
              await file.writeAsBytes(image.encodePng(input));
              results.add(
                await segmenter
                    .processImage(InputImage.fromFilePath(file.path))
                    .timeout(const Duration(seconds: 60)),
              );
            }
            return results;
          } finally {
            try {
              await segmenter.close().timeout(const Duration(seconds: 10));
            } finally {
              await directory.delete(recursive: true);
            }
          }
        });

        // runAsync can return null after reporting an asynchronous error.
        // Assert completion and results outside that error-catching boundary.
        expect(masks, isNotNull);
        expect(masks, hasLength(2));
        for (final mask in masks!) {
          expect(mask, isNotNull);
          expect(mask!.width, 64);
          expect(mask.height, 64);
          expect(mask.confidences, hasLength(64 * 64));
          expect(
            mask.confidences.every(
              (value) => value.isFinite && value >= 0 && value <= 1,
            ),
            isTrue,
          );
        }
      },
    );
  }
}
