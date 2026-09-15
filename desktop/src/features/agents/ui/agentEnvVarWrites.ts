import type * as React from "react";

/**
 * Shared env-var writes for agent config dialogs (#52). Kept out of the
 * dialogs themselves so the file-size ratchet stays satisfied.
 */
export function setEnvVarKey(
  setEnvVars: React.Dispatch<React.SetStateAction<Record<string, string>>>,
  envVar: string | null | undefined,
): (next: string) => void {
  return (next) =>
    setEnvVars((prev) => ({ ...prev, [envVar as string]: next }));
}
