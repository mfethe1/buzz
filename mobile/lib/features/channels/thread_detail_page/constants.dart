part of '../thread_detail_page.dart';

// Landing-highlight timings for a thread opened via deep link / jump-to-reply.
const _landingHighlightDuration = Duration(seconds: 3);
const _landingHighlightDelay = Duration(milliseconds: 50);
const _landingHighlightTransitionDuration = Duration(milliseconds: 300);
const _landingHighlightOpacity = 0.12;

const _threadTailScrollTolerance = 0.5;

// Keep the direct-position correction finite in case the viewport cannot
// expose its tail (for example, continuously changing media dimensions).
const _latestTailCorrectionLimit = 8;
