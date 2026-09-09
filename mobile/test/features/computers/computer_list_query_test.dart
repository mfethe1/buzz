import 'dart:async';
import 'package:buzz/features/computers/computer_list_query.dart';
import 'package:buzz/shared/machines/computer.dart';
import 'package:buzz/shared/machines/machines_api.dart';
import 'package:flutter_test/flutter_test.dart';

EnrolledComputer computer(String id, {int sequence = 0}) => EnrolledComputer(
  id: id,
  ownerPubkey: 'owner',
  coordinatorPubkey: 'coordinator',
  label: id,
  runtime: 'hermes',
  sequence: sequence,
  serverFresh: false,
);
void main() {
  test(
    'authorization loss on pagination clears cached private rows and cursor',
    () async {
      var reads = 0;
      final q = ComputerListQuery((_) async {
        if (reads++ == 0) {
          return ComputerPage(
            computers: [computer('private')],
            nextCursor: 'next',
          );
        }
        throw const ComputerApiException(403);
      });
      await q.refresh();
      await q.loadMore();
      expect(q.value.computers, isEmpty);
      expect(q.value.nextCursor, isNull);
      expect(q.value.error, isA<ComputerApiException>());
      q.dispose();
    },
  );
  test(
    'refresh discards pending page and its error, then reads a new first page',
    () async {
      for (final fail in [false, true]) {
        final pending = Completer<ComputerPage>();
        var calls = 0;
        final q = ComputerListQuery((cursor) async {
          calls++;
          if (calls == 1) {
            return ComputerPage(
              computers: [computer('old')],
              nextCursor: 'cursor',
            );
          }
          if (calls == 2) return pending.future;
          return ComputerPage(computers: [computer('new')]);
        });
        await q.refresh();
        final emitted = <ComputerListState>[];
        q.addListener(() => emitted.add(q.value));
        final page = q.loadMore();
        await Future<void>.delayed(Duration.zero);
        final refreshed = q.refresh();
        expect(q.value.computers, isEmpty);
        if (fail) {
          pending.completeError(StateError('old private error'));
        } else {
          pending.complete(ComputerPage(computers: [computer('private')]));
        }
        await page;
        await refreshed;
        expect(q.value.computers.map((v) => v.id), ['new']);
        expect(q.value.error, isNull);
        expect(calls, 3);
        expect(
          emitted.any(
            (state) =>
                state.computers.any((c) => c.id == 'private') ||
                state.error != null,
          ),
          isFalse,
        );
        q.dispose();
      }
    },
  );
  test('disposed identity cannot publish a late private page', () async {
    final pending = Completer<ComputerPage>();
    final q = ComputerListQuery((_) => pending.future);
    final load = q.refresh();
    await Future<void>.delayed(Duration.zero);
    q.dispose();
    pending.complete(ComputerPage(computers: [computer('private')]));
    await load;
    expect(q.value.computers, isEmpty);
  });
  test(
    'duplicate pages keep highest observation; cursor loop fails and retries',
    () async {
      var call = 0;
      final q = ComputerListQuery((cursor) async {
        call++;
        if (call == 1) {
          return ComputerPage(
            computers: [computer('one', sequence: 2)],
            nextCursor: 'a',
          );
        }
        if (call == 2) {
          return ComputerPage(computers: [computer('one')], nextCursor: 'a');
        }
        return ComputerPage(computers: [computer('one'), computer('two')]);
      });
      await q.refresh();
      await q.loadMore();
      expect(q.value.error, isA<FormatException>());
      expect(q.value.nextCursor, 'a');
      await q.loadMore();
      expect(q.value.computers.map((v) => v.id), ['one', 'two']);
      expect(q.value.computers.first.sequence, 2);
      q.dispose();
    },
  );
}
