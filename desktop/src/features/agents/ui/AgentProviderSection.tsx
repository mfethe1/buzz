import * as React from "react";

import { setEnvVarKey } from "./agentEnvVarWrites";
import type { PersonaDropdownOption } from "./agentConfigOptions";
import { EditAgentProviderModelFields } from "./EditAgentProviderModelFields";
import {
  buildOpenaiCompatBaseUrlConfig,
  type OpenaiCompatBaseUrlConfig,
} from "./PersonaProviderBaseUrlField";

/**
 * The LLM provider / API-key / base-URL / model section of the edit-agent
 * dialog (#52). Extracted so AgentInstanceEditDialog stays under the
 * file-size ratchet while the base-URL field adds props.
 */
export function AgentProviderSection({
  apiKeyInheritedLabel,
  apiKeyIsInherited,
  apiKeyIsRequired,
  apiKeyValue,
  disabled,
  effectiveProvider,
  globalEnvVars,
  envVars,
  isCustomProviderEditing,
  llmProviderFieldVisible,
  model,
  modelDiscoveryLoading,
  modelDropdownOptions,
  modelRequired,
  modelSelectValue,
  modelStatusMessage,
  onModelDropdownChange,
  onProviderDropdownChange,
  provider,
  providerDropdownOptions,
  providerRequired,
  providerSelectValue,
  setEnvVars,
  setModel,
  setProvider,
  showCustomModelInput,
  topLevelSecretEnvVar,
}: {
  apiKeyInheritedLabel: string;
  apiKeyIsInherited: boolean;
  apiKeyIsRequired: boolean;
  apiKeyValue: string;
  disabled: boolean;
  effectiveProvider: string;
  envVars: Record<string, string>;
  globalEnvVars: Record<string, string> | undefined;
  isCustomProviderEditing: boolean;
  llmProviderFieldVisible: boolean;
  model: string;
  modelDiscoveryLoading: boolean;
  modelDropdownOptions: PersonaDropdownOption[];
  modelRequired: boolean;
  modelSelectValue: string;
  modelStatusMessage: string | null;
  onModelDropdownChange: (value: string) => void;
  onProviderDropdownChange: (value: string) => void;
  provider: string;
  providerDropdownOptions: PersonaDropdownOption[];
  providerRequired: boolean;
  providerSelectValue: string;
  setEnvVars: React.Dispatch<React.SetStateAction<Record<string, string>>>;
  setModel: (value: string) => void;
  setProvider: (value: string) => void;
  showCustomModelInput: boolean;
  topLevelSecretEnvVar: string | null;
}) {
  const baseUrlConfig: OpenaiCompatBaseUrlConfig =
    buildOpenaiCompatBaseUrlConfig(
      envVars,
      globalEnvVars,
      effectiveProvider,
      setEnvVars,
    );
  return (
    <EditAgentProviderModelFields
      apiKeyInheritedLabel={apiKeyInheritedLabel}
      apiKeyIsInherited={apiKeyIsInherited}
      apiKeyIsRequired={apiKeyIsRequired}
      apiKeyValue={apiKeyValue}
      disabled={disabled}
      effectiveProvider={effectiveProvider}
      isCustomProviderEditing={isCustomProviderEditing}
      llmProviderFieldVisible={llmProviderFieldVisible}
      model={model}
      modelDiscoveryLoading={modelDiscoveryLoading}
      modelDropdownOptions={modelDropdownOptions}
      modelRequired={modelRequired}
      modelSelectValue={modelSelectValue}
      modelStatusMessage={modelStatusMessage}
      onApiKeyChange={setEnvVarKey(setEnvVars, topLevelSecretEnvVar)}
      onModelDropdownChange={onModelDropdownChange}
      onProviderChange={setProvider}
      onProviderDropdownChange={onProviderDropdownChange}
      openaiCompatBaseUrl={baseUrlConfig}
      provider={provider}
      providerDropdownOptions={providerDropdownOptions}
      providerRequired={providerRequired}
      providerSelectValue={providerSelectValue}
      showCustomModelInput={showCustomModelInput}
      topLevelSecretEnvVar={topLevelSecretEnvVar}
      onModelChange={setModel}
    />
  );
}
