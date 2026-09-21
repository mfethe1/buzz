import * as React from "react";

import { cn } from "@/shared/lib/cn";

/**
 * Transcript of a voice note, published as a separate signed `kind:40009`
 * event and resolved onto the note by `voiceNoteTranscript.mjs`.
 *
 * Collapsed by default: a transcript is a reading aid, not a replacement for
 * the audio, and expanding every one would bury short voice notes under walls
 * of text. The full text is still in the DOM when expanded so browser find and
 * screen readers reach it.
 */
export function VoiceNoteTranscript({
  className,
  text,
}: {
  className?: string;
  text: string;
}) {
  const [expanded, setExpanded] = React.useState(false);
  const bodyId = React.useId();

  return (
    <div className={cn("mt-1 flex flex-col gap-1", className)}>
      <button
        aria-controls={bodyId}
        aria-expanded={expanded}
        className="self-start text-muted-foreground text-xs underline-offset-2 hover:underline"
        data-testid="voice-note-transcript-toggle"
        onClick={() => setExpanded((open) => !open)}
        type="button"
      >
        {expanded ? "Hide transcript" : "Show transcript"}
      </button>
      {expanded ? (
        <p
          className="whitespace-pre-wrap text-muted-foreground text-sm"
          data-testid="voice-note-transcript-text"
          id={bodyId}
        >
          {text}
        </p>
      ) : null}
    </div>
  );
}
