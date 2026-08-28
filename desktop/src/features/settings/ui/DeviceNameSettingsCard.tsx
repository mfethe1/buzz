import { useMutation, useQueryClient } from "@tanstack/react-query";
import * as React from "react";
import { toast } from "sonner";

import {
  deviceIdentityQueryKey,
  useDeviceIdentityQuery,
} from "@/features/agents/hooks";
import {
  getDeviceNameSuggestion,
  resetDeviceLabel,
  setDeviceLabel,
} from "@/shared/api/tauriDeviceIdentity";
import { Button } from "@/shared/ui/button";
import { Input } from "@/shared/ui/input";
import { saveIsDisabled, showsResetControl } from "./deviceNameCardLogic";
import { SettingsOptionGroup, SettingsOptionRow } from "./SettingsOptionGroup";

/** Backend cap on the device label; mirrored here so the field stops typing at it. */
const DEVICE_LABEL_MAX_LENGTH = 32;

/**
 * Shows and renames this install's device label.
 *
 * The label is published on each local agent's kind:30177 event so the same
 * account's other devices can tell duplicate agent names apart, which is why
 * the subcopy spells out that it leaves the machine.
 */
export function DeviceNameSettingsCard() {
  const queryClient = useQueryClient();
  const deviceIdentity = useDeviceIdentityQuery();
  const savedLabel = deviceIdentity.data?.deviceLabel ?? "";
  const [draftLabel, setDraftLabel] = React.useState(savedLabel);
  const [editedLabel, setEditedLabel] = React.useState(false);

  // Seed the field from the query the first time it resolves, but never stomp
  // on what the user is currently typing.
  React.useEffect(() => {
    if (!editedLabel) {
      setDraftLabel(savedLabel);
    }
  }, [editedLabel, savedLabel]);

  const renameDevice = useMutation({
    mutationFn: (label: string) => setDeviceLabel(label),
    onSuccess: (identity) => {
      setEditedLabel(false);
      setDraftLabel(identity.deviceLabel);
      queryClient.setQueryData(deviceIdentityQueryKey, identity);
      toast.success("Device name updated");
    },
    onError: (error: unknown) => {
      toast.error(
        error instanceof Error ? error.message : "Could not rename this device",
      );
    },
  });

  // REG-11: the one-click way back from a published name. FM1 honesty rule:
  // the copy says "stop showing" (forward pseudonymisation), never "remove"
  // or "erase" — the device id is unchanged and previously published events
  // stay fetchable at their coordinates.
  const resetDevice = useMutation({
    mutationFn: () => resetDeviceLabel(),
    onSuccess: (identity) => {
      setEditedLabel(false);
      setDraftLabel(identity.deviceLabel);
      // Publish the identity the command returned rather than invalidating and
      // waiting for a refetch: the Reset control's own visibility is derived
      // from this query, so a lagging cache leaves it on screen and clickable
      // after the reset already landed.
      queryClient.setQueryData(deviceIdentityQueryKey, identity);
      toast.success("Device name reset to anonymous");
    },
    onError: (error: unknown) => {
      toast.error(
        error instanceof Error
          ? error.message
          : "Could not reset this device's name",
      );
    },
  });

  // The OS host name is only ever *offered*. A device's name starts opaque
  // (`device-xxxxxxxx`) precisely because host names routinely carry a real
  // person's name and this label is published world-readable — so the owner
  // opts in here, seeing the warning above before anything leaves the machine.
  const [suggestion, setSuggestion] = React.useState<string | null>(null);
  React.useEffect(() => {
    let cancelled = false;
    void getDeviceNameSuggestion()
      .then((value) => {
        if (!cancelled) {
          setSuggestion(value);
        }
      })
      .catch(() => {
        // Advisory only — a device with no usable host name simply gets no
        // suggestion, which is not worth surfacing as an error.
      });
    return () => {
      cancelled = true;
    };
  }, []);

  const trimmedLabel = draftLabel.trim();
  const showSuggestion =
    suggestion !== null &&
    suggestion !== savedLabel &&
    suggestion !== trimmedLabel;

  // REG-11: only offer the way back when this device is actually off its
  // anonymous default — see `showsResetControl`.
  const showsReset = showsResetControl(deviceIdentity.data);
  const saveDisabled = saveIsDisabled({
    trimmedLabel,
    savedLabel,
    identityLoaded: Boolean(deviceIdentity.data),
    renamePending: renameDevice.isPending,
    resetPending: resetDevice.isPending,
  });

  return (
    <div className="min-w-0 space-y-3">
      <SettingsOptionGroup data-testid="device-name-card" title="This device">
        <SettingsOptionRow className="flex-col items-stretch gap-3 sm:flex-row sm:items-center">
          <div className="min-w-0">
            <label className="text-sm font-medium" htmlFor="device-name-input">
              Device name
            </label>
            <p
              className="text-sm font-normal text-muted-foreground/70"
              data-settings-subcopy
            >
              Shown next to this device's agents when you view them from your
              other devices. Published with your agents, so avoid personal
              details.
            </p>
            {showSuggestion ? (
              <button
                className="mt-1 text-xs text-muted-foreground underline underline-offset-2 hover:text-foreground"
                data-testid="device-name-use-hostname"
                onClick={() => {
                  setEditedLabel(true);
                  setDraftLabel(suggestion);
                }}
                type="button"
              >
                Use this computer's name ({suggestion})
              </button>
            ) : null}
          </div>
          <div className="flex shrink-0 items-center gap-2">
            <Input
              className="w-48"
              data-testid="device-name-input"
              disabled={
                !deviceIdentity.data ||
                renameDevice.isPending ||
                resetDevice.isPending
              }
              id="device-name-input"
              maxLength={DEVICE_LABEL_MAX_LENGTH}
              onChange={(event) => {
                setEditedLabel(true);
                setDraftLabel(event.target.value);
              }}
              value={draftLabel}
            />
            <Button
              data-testid="device-name-save"
              disabled={saveDisabled}
              onClick={() => {
                renameDevice.mutate(trimmedLabel);
              }}
              size="sm"
              type="button"
              variant="outline"
            >
              Save
            </Button>
            {showsReset ? (
              <Button
                data-testid="device-name-reset"
                disabled={resetDevice.isPending || renameDevice.isPending}
                onClick={() => {
                  resetDevice.mutate();
                }}
                size="sm"
                title="Stop showing a real name on your agents. New views see the anonymous device-xxxxxxxx default; the device id is unchanged and past events stay fetchable, so this is pseudonymisation, not erasure."
                type="button"
                variant="ghost"
              >
                Reset
              </Button>
            ) : null}
          </div>
        </SettingsOptionRow>
      </SettingsOptionGroup>
    </div>
  );
}
