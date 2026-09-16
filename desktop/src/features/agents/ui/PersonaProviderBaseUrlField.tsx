import * as React from "react";

import { cn } from "@/shared/lib/cn";
import { Input } from "@/shared/ui/input";
import {
  PERSONA_FIELD_CONTROL_CLASS,
  PERSONA_FIELD_SHELL_CLASS,
} from "./agentConfigOptions";

/**
 * Top-level "Base URL" field for the OpenAI-compatible provider (#52).
 *
 * The harness reads `OPENAI_COMPAT_BASE_URL` from the agent env (default
 * https://api.openai.com/v1). Without this field the dialog persisted the key
 * and model but silently dropped the base URL, so local endpoints (Ollama,
 * vLLM) were ignored and every call went to api.openai.com and died on 401.
 *
 * Pure view over `envVars[OPENAI_COMPAT_BASE_URL]` — writes go through
 * `onValueChange`, which the parent routes to its env-var state. The Advanced
 * env-var row and this field are the same state, so sync is free.
 *
 * Shown only when `provider === "openai-compat"`; the inherited/placeholder
 * semantics mirror PersonaProviderApiKeyField but are informational — an empty
 * base URL is legal (it means "use the harness default").
 */
export const OPENAI_COMPAT_BASE_URL_ENV_VAR = "OPENAI_COMPAT_BASE_URL";

export const OPENAI_COMPAT_BASE_URL_PLACEHOLDER =
  "https://api.openai.com/v1 (default)";

/**
 * Canonical env-var write for the base URL (#52): empty clears the var (back
 * to harness default), non-empty sets it. Shared by every config surface so
 * the round-trip semantics are identical everywhere.
 */
export function applyOpenaiCompatBaseUrlChange(
  setEnvVars: React.Dispatch<React.SetStateAction<Record<string, string>>>,
): (next: string) => void {
  return (next) =>
    setEnvVars((prev) =>
      next.trim().length === 0
        ? Object.fromEntries(
            Object.entries(prev).filter(
              ([key]) => key !== OPENAI_COMPAT_BASE_URL_ENV_VAR,
            ),
          )
        : { ...prev, [OPENAI_COMPAT_BASE_URL_ENV_VAR]: next },
    );
}

/**
 * Props for `EditAgentProviderModelFields`' base-URL field (#52), so config
 * dialogs don't grow: spread with `{...openaiCompatBaseUrlProps(...)}`.
 */
export type OpenaiCompatBaseUrlConfig = {
  fieldVisible: boolean;
  value: string;
  inheritedLabel: string;
  onValueChange: (next: string) => void;
};

export function buildOpenaiCompatBaseUrlConfig(
  envVars: Record<string, string>,
  inheritedEnvVars: Record<string, string> | undefined,
  effectiveProvider: string,
  setEnvVars: React.Dispatch<React.SetStateAction<Record<string, string>>>,
): OpenaiCompatBaseUrlConfig {
  return {
    fieldVisible: effectiveProvider === "openai-compat",
    value: envVars[OPENAI_COMPAT_BASE_URL_ENV_VAR] ?? "",
    inheritedLabel: inheritedEnvVars?.[OPENAI_COMPAT_BASE_URL_ENV_VAR] ?? "",
    onValueChange: applyOpenaiCompatBaseUrlChange(setEnvVars),
  };
}

export function PersonaProviderBaseUrlField({
  className,
  disabled,
  id = "persona-openai-compat-base-url",
  inheritedLabel,
  isInherited,
  onValueChange,
  value,
}: {
  className?: string;
  disabled: boolean;
  id?: string;
  /** Where the effective-but-not-locally-set base URL comes from, if known. */
  inheritedLabel: string;
  /** True when a non-empty base URL is satisfied by an inherited layer. */
  isInherited: boolean;
  onValueChange: (value: string) => void;
  value: string;
}) {
  const [focused, setFocused] = React.useState(false);
  const hintId = `${id}-env-hint`;
  return (
    <div className={cn("space-y-1.5", className)}>
      <label className="text-sm font-medium text-foreground" htmlFor={id}>
        Base URL
        <span className="ml-1.5 text-xs font-normal text-muted-foreground">
          OpenAI-compatible endpoint
        </span>
      </label>
      <div
        className={cn(
          "flex min-h-11 items-center px-3",
          PERSONA_FIELD_SHELL_CLASS,
        )}
      >
        <Input
          aria-describedby={hintId}
          autoCapitalize="off"
          autoCorrect="off"
          className={cn("h-8 px-0 py-0 leading-6", PERSONA_FIELD_CONTROL_CLASS)}
          disabled={disabled}
          id={id}
          inputMode="url"
          onBlur={() => setFocused(false)}
          onChange={(event) => onValueChange(event.target.value)}
          onFocus={() => setFocused(true)}
          placeholder={
            isInherited && value.length === 0
              ? inheritedLabel || OPENAI_COMPAT_BASE_URL_PLACEHOLDER
              : OPENAI_COMPAT_BASE_URL_PLACEHOLDER
          }
          spellCheck={false}
          type="text"
          value={value}
        />
      </div>
      {focused ? (
        <p className="font-mono text-2xs text-muted-foreground/70" id={hintId}>
          {OPENAI_COMPAT_BASE_URL_ENV_VAR}
        </p>
      ) : null}
    </div>
  );
}
